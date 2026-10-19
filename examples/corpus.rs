//! Real-world corpus check: runs the parsers and the formatters over every
//! file in a directory (default `test/corpus`, filled by
//! `tools/corpus/fetch.sh`) and compares against oxttl (RDF) and spargebra
//! (SPARQL).
//!
//!     cargo run --release --example corpus [-- DIR] [--only TEXT]
//!
//! `--only TEXT` runs only the cases whose name contains TEXT (a SPARQL case
//! is named `DIR/path/file.ttl sh:select`) and writes
//! `target/corpus/report-only.md` instead of the full report.
//!
//! Per RDF file (Turtle, TriG, N-Triples, N-Quads) it reports:
//! - parser: our triples against oxttl's (count, ground triples, graph names),
//!   and whether both agree the file is valid. In N-Triples and N-Quads both
//!   must reject exactly the same lines (real-world dumps have invalid lines)
//!   and agree on the quads of the others;
//! - formatter: the output parses to the same quads (ours and oxttl's), keeps
//!   every comment, and formatting it again changes nothing;
//! - robustness: panics and files that take longer than the time limit;
//! - cost: parse and format time.
//!
//! RDF/XML files (`.rdf`) are converted to N-Triples first. Every Turtle file
//! under 5 MB is also serialized by oxttl into one named graph, as TriG
//! (`NAME.gen.trig`) and as N-Quads (`NAME.gen.nq`), since there is little
//! public TriG or N-Quads to download. An N-Triples or N-Quads file with
//! invalid lines is also checked without them (`NAME.valid.nt` / `.nq`), so that the
//! formatter, which refuses a file with syntax errors, sees the real data.
//!
//! Every subdirectory is searched for SPARQL examples in the SIB
//! `sparql-examples` layout: each `sh:select`, `sh:ask`, `sh:construct`,
//! `sh:describe`, `sh:update` and `spex:describe` (the repository's DESCRIBE
//! queries) literal of a `.ttl` file is a SPARQL case, one report row per
//! example folder.
//! When the example names `sh:prefixes`, the `sh:declare`d prefixes of that
//! node are passed as the document's external prefixes, read as the
//! repository's own tooling reads them: `examples/prefixes.ttl`, the
//! example's folder's `prefixes.ttl` and the example concatenated into one
//! Turtle document. Per case:
//! - verdict: no error diagnostic exactly when spargebra parses it with the
//!   same prefixes and base (as a query, or as an update for `sh:update`),
//!   and the document is an update exactly for `sh:update`;
//! - lexer: no error token in a query spargebra accepts;
//! - formatter: a valid query formats, to the same algebra (blank node labels
//!   and spargebra's aggregate variables renumbered), keeping every comment,
//!   and formatting the output again changes nothing; an invalid one is
//!   refused;
//! - robustness and cost as above.
//!
//! Writes `target/corpus/report.md` and one `target/corpus/details/NAME.txt`
//! per file (or SPARQL folder, each failing case with its text) with a
//! finding or a note (lines or queries both parsers reject). Exits with 1
//! when any case has a finding.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use turtle_lsp::features::format::{Options, format};
use turtle_lsp::sparql::{self, Document, Level, TokenKind};
use turtle_lsp::syntax::lexer::{Kind, tokenize};
use turtle_lsp::syntax::{GraphName, Node, PrefixMap, Severity, Syntax, parse_with_base};

const TIME_LIMIT: Duration = Duration::from_secs(300);
const GEN_MAX_BYTES: usize = 5 << 20;
const SH: &str = "http://www.w3.org/ns/shacl#";
/// The predicates whose literals are SPARQL: (name in case names, IRI, is an
/// update). The SIB repository writes DESCRIBE queries with `spex:describe`.
const QUERY_PREDICATES: &[(&str, &str, bool)] = &[
    ("sh:select", "http://www.w3.org/ns/shacl#select", false),
    ("sh:ask", "http://www.w3.org/ns/shacl#ask", false),
    (
        "sh:construct",
        "http://www.w3.org/ns/shacl#construct",
        false,
    ),
    ("sh:describe", "http://www.w3.org/ns/shacl#describe", false),
    ("sh:update", "http://www.w3.org/ns/shacl#update", true),
    (
        "spex:describe",
        "https://purl.expasy.org/sparql-examples/ontology#describe",
        false,
    ),
];

struct Case {
    name: String,
    syntax: Syntax,
    text: String,
    base: String,
}

#[derive(Default)]
struct Row {
    name: String,
    bytes: usize,
    triples: usize,
    parse_ms: f64,
    oxttl_ms: f64,
    format_ms: f64,
    /// "same", or what differs.
    parser: String,
    /// "same", "refused (syntax errors)", "-", or what differs.
    formatter: String,
    findings: Vec<String>,
    /// Worth reading but not wrong (lines both parsers reject).
    notes: Vec<String>,
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let (mut dir, mut only) = (None, None);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--only" {
            only = Some(args.next().expect("--only needs a value"));
        } else if let Some(v) = a.strip_prefix("--only=") {
            only = Some(v.to_string());
        } else {
            dir = Some(PathBuf::from(a));
        }
    }
    let dir = dir.unwrap_or_else(|| root.join("test/corpus"));
    let wanted = |name: &str| only.as_deref().is_none_or(|o| name.contains(o));
    let out = root.join("target/corpus");
    let details = out.join("details");
    if only.is_none() {
        let _ = fs::remove_dir_all(&details);
    }
    fs::create_dir_all(&details).unwrap();
    let write_details = |file: &str, findings: &[String], notes: &[String]| {
        let path = details.join(format!("{file}.txt"));
        let all: Vec<&String> = findings.iter().chain(notes).collect();
        if all.is_empty() {
            let _ = fs::remove_file(path);
        } else {
            let text: Vec<&str> = all.iter().map(|s| s.as_str()).collect();
            fs::write(path, text.join("\n\n")).unwrap();
        }
    };

    let mut entries: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}; run tools/corpus/fetch.sh", dir.display()))
        .flatten()
        .map(|e| e.path())
        .collect();
    entries.sort();

    let mut rows = Vec::new();
    for path in entries.iter().filter(|p| p.is_file()) {
        for case in cases(path) {
            let name = case.name.clone();
            if !wanted(&name) {
                continue;
            }
            eprint!("{name:<32} ");
            let row = run_with_limit(case);
            eprintln!(
                "{:>9} triples  parse {:>8.1} ms  format {:>8.1} ms  {}",
                row.triples,
                row.parse_ms,
                row.format_ms,
                if row.findings.is_empty() {
                    "ok"
                } else {
                    "FINDING"
                }
            );
            write_details(&name, &row.findings, &row.notes);
            rows.push(row);
        }
    }

    let mut groups: BTreeMap<String, Vec<SparqlResult>> = BTreeMap::new();
    for sub in entries.iter().filter(|p| p.is_dir()) {
        let (cases, problems) = sparql_cases(sub);
        for (group, finding) in problems {
            if wanted(&group) {
                groups.entry(group).or_default().push(SparqlResult {
                    findings: vec![finding],
                    ..Default::default()
                });
            }
        }
        for case in cases.into_iter().filter(|c| wanted(&c.name)) {
            let (group, name, text, bytes) = (
                case.group.clone(),
                case.name.clone(),
                case.text.clone(),
                case.text.len(),
            );
            let r = limited(move || check_sparql(&case)).unwrap_or_else(|e| SparqlResult {
                name,
                bytes,
                findings: vec![format!("{e}\n{text}")],
                ..Default::default()
            });
            groups.entry(group).or_default().push(r);
        }
    }
    let sparql_rows: Vec<SparqlRow> = groups
        .iter()
        .map(|(group, results)| {
            let row = SparqlRow::of(group, results);
            eprintln!(
                "{group:<32} {:>5} queries  {} agree  formatter {} safe / {} refused / {} failed  {}",
                row.cases,
                row.agree,
                row.safe,
                row.refused,
                row.failed,
                if row.findings == 0 { "ok" } else { "FINDING" }
            );
            let findings: Vec<String> = results.iter().flat_map(|r| r.findings.clone()).collect();
            let notes: Vec<String> = results.iter().flat_map(|r| r.notes.clone()).collect();
            write_details(&format!("sparql-{}", group.replace('/', "-")), &findings, &notes);
            row
        })
        .collect();

    let report = report(&rows, &sparql_rows);
    let file = if only.is_some() {
        "report-only.md"
    } else {
        "report.md"
    };
    fs::write(out.join(file), &report).unwrap();
    println!("{report}");
    println!("Wrote {}", out.join(file).display());
    if rows.iter().any(|r| !r.findings.is_empty()) || sparql_rows.iter().any(|r| r.findings > 0) {
        std::process::exit(1);
    }
}

/// The cases a downloaded file yields: itself (RDF/XML as N-Triples), plus
/// generated TriG and N-Quads copies of a small enough Turtle file, plus the
/// valid lines of an N-Triples or N-Quads file with invalid ones.
fn cases(path: &Path) -> Vec<Case> {
    let stem = path.file_stem().unwrap().to_string_lossy().to_string();
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let base = format!("http://corpus.example/{stem}");
    let bytes = fs::read(path).unwrap();
    let mut out = Vec::new();
    match ext.as_str() {
        "rdf" => match rdfxml_to_ntriples(&bytes, &base) {
            Ok(text) => out.push(Case {
                name: format!("{stem}.rdf→nt"),
                syntax: Syntax::NTriples,
                text,
                base,
            }),
            Err(e) => eprintln!("{stem}.rdf: not converted: {e}"),
        },
        "ttl" | "nt" | "nq" | "trig" => {
            let syntax = Syntax::from_extension(&format!("x.{ext}")).unwrap();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let mut generated = Vec::new();
            if syntax == Syntax::Turtle && text.len() <= GEN_MAX_BYTES {
                for (gen_ext, gen_syntax) in [("trig", Syntax::TriG), ("nq", Syntax::NQuads)] {
                    match turtle_to_dataset(&text, &base, &stem, gen_syntax) {
                        Ok(copy) => generated.push(Case {
                            name: format!("{stem}.gen.{gen_ext}"),
                            syntax: gen_syntax,
                            text: copy,
                            base: base.clone(),
                        }),
                        Err(e) => eprintln!("{stem}: no {gen_ext} copy: {e}"),
                    }
                }
            }
            if syntax.is_line_based() {
                let (_, errors) = oxttl_line_based(&text, syntax);
                if !errors.is_empty() {
                    let lines = Lines::new(&text);
                    let bad: BTreeSet<usize> = errors.iter().map(|(o, _)| lines.line(*o)).collect();
                    let valid: String = text
                        .split_inclusive('\n')
                        .enumerate()
                        .filter(|(i, _)| !bad.contains(&(i + 1)))
                        .map(|(_, l)| l)
                        .collect();
                    generated.push(Case {
                        name: format!("{stem}.valid.{ext}"),
                        syntax,
                        text: valid,
                        base: base.clone(),
                    });
                }
            }
            out.push(Case {
                name: format!("{stem}.{ext}"),
                syntax,
                text,
                base,
            });
            out.extend(generated);
        }
        _ => {}
    }
    out
}

fn rdfxml_to_ntriples(bytes: &[u8], base: &str) -> Result<String, String> {
    let mut s = String::new();
    let parser = oxrdfxml::RdfXmlParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?;
    for t in parser.for_slice(bytes) {
        let t = t.map_err(|e| e.to_string())?;
        writeln!(s, "{t} .").unwrap();
    }
    Ok(s)
}

/// The Turtle file as TriG or N-Quads, all of it in one named graph.
fn turtle_to_dataset(text: &str, base: &str, stem: &str, syntax: Syntax) -> Result<String, String> {
    let mut parser = oxttl::TurtleParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?
        .for_slice(text);
    let mut triples = Vec::new();
    for t in parser.by_ref() {
        triples.push(t.map_err(|e| e.to_string())?);
    }
    let g = oxrdf::NamedNode::new(format!("http://corpus.example/graph/{stem}")).unwrap();
    let quads = triples.into_iter().map(|t| t.in_graph(g.clone()));
    let bytes = if syntax == Syntax::NQuads {
        let mut w = oxttl::NQuadsSerializer::new().for_writer(Vec::new());
        for q in quads {
            w.serialize_quad(q.as_ref()).map_err(|e| e.to_string())?;
        }
        w.finish()
    } else {
        let mut ser = oxttl::TriGSerializer::new();
        for (name, iri) in parser.prefixes() {
            ser = ser.with_prefix(name, iri).map_err(|e| e.to_string())?;
        }
        let mut w = ser.for_writer(Vec::new());
        for q in quads {
            w.serialize_quad(q.as_ref()).map_err(|e| e.to_string())?;
        }
        w.finish().map_err(|e| e.to_string())?
    };
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Runs `f` on its own thread so a hang or a panic is an error, not the end
/// of the run.
fn limited<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let r = catch_unwind(AssertUnwindSafe(f));
            let _ = tx.send(r);
        })
        .unwrap();
    match rx.recv_timeout(TIME_LIMIT) {
        Ok(Ok(r)) => Ok(r),
        Ok(Err(p)) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            Err(format!("PANIC: {msg}"))
        }
        Err(_) => Err(format!(
            "TIMEOUT: still running after {} s",
            TIME_LIMIT.as_secs()
        )),
    }
}

/// Runs [`check`] under [`limited`]: a hang or a panic is one finding.
fn run_with_limit(case: Case) -> Row {
    let (name, bytes) = (case.name.clone(), case.text.len());
    limited(move || check(&case)).unwrap_or_else(|what| Row {
        name,
        bytes,
        parser: "-".into(),
        formatter: "-".into(),
        findings: vec![what],
        ..Default::default()
    })
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn check(case: &Case) -> Row {
    let Case {
        name,
        syntax,
        text,
        base,
    } = case;
    let (syntax, base) = (*syntax, Some(base.as_str()));
    let mut row = Row {
        name: name.clone(),
        bytes: text.len(),
        ..Default::default()
    };

    let t = Instant::now();
    let parsed = parse_with_base(text, &PrefixMap::default(), syntax, base);
    row.parse_ms = ms(t);
    row.triples = parsed.triples.len();
    let errors: Vec<String> = parsed
        .errors
        .iter()
        .filter(|e| e.severity == Severity::Error)
        .take(10)
        .map(|e| format!("{} at {}", e.message, line_col(text, e.span.start)))
        .collect();

    // 1. Parser against oxttl.
    let ox = if syntax.is_line_based() {
        compare_lines(text, syntax, &parsed, &mut row)
    } else {
        let t = Instant::now();
        let ox = oxttl_quads(text, syntax, base.unwrap());
        row.oxttl_ms = ms(t);
        row.parser = match (&ox, errors.is_empty()) {
            (Ok(quads), true) => match compare(&parsed.triples.iter().collect::<Vec<_>>(), quads) {
                None => "same".into(),
                Some(diff) => {
                    row.findings
                        .push(format!("PARSER DIFFERS from oxttl\n{diff}"));
                    "differs".into()
                }
            },
            (Err(e), false) => {
                // Both reject: fine, but worth seeing in the details.
                row.findings.push(format!(
                    "BOTH REJECT (input is invalid?)\noxttl: {e}\nours: {}",
                    errors.join("\n      ")
                ));
                "both reject".into()
            }
            (Ok(_), false) => {
                row.findings.push(format!(
                    "WE REJECT a file oxttl accepts\n{}",
                    errors.join("\n")
                ));
                "we reject".into()
            }
            (Err(e), true) => {
                row.findings
                    .push(format!("WE ACCEPT a file oxttl rejects\noxttl: {e}"));
                "we accept".into()
            }
        };
        ox
    };

    // 2. Formatter, on a parse without a base IRI, as the server parses (the
    // formatter's own safety check re-reads its output without one).
    let parsed = parse_with_base(text, &PrefixMap::default(), syntax, None);
    let opts = Options::default();
    let t = Instant::now();
    let out = format(text, &parsed, &PrefixMap::default(), &opts);
    row.format_ms = ms(t);
    let Some(out) = out else {
        row.formatter = if errors.is_empty() {
            row.findings
                .push("FORMATTER REFUSED a file without syntax errors".into());
            "refused".into()
        } else {
            "refused (syntax errors)".into()
        };
        return row;
    };
    let reparsed = parse_with_base(&out, &PrefixMap::default(), syntax, None);
    let mut problems = Vec::new();
    if reparsed
        .errors
        .iter()
        .any(|e| e.severity == Severity::Error)
    {
        problems.push(format!(
            "output has syntax errors: {}",
            reparsed
                .errors
                .iter()
                .take(3)
                .map(|e| format!("{} at {}", e.message, line_col(&out, e.span.start)))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if ours(&parsed) != ours(&reparsed) {
        problems.push(first_difference(
            "our quads change",
            &ours(&parsed),
            &ours(&reparsed),
        ));
    }
    if let Ok(before) = &ox {
        match oxttl_quads(&out, syntax, base.unwrap()) {
            Ok(after) => {
                let (b, a) = (canonical(before), canonical(&after));
                if b != a {
                    problems.push(first_difference("oxttl quads change", &b, &a));
                }
            }
            Err(e) => problems.push(format!("oxttl rejects the output: {e}")),
        }
    }
    if comments(text) != comments(&out) {
        problems.push(first_difference(
            "comments change",
            &comments(text),
            &comments(&out),
        ));
    }
    match format(&out, &reparsed, &PrefixMap::default(), &opts) {
        Some(again) if again == out => {}
        Some(again) => problems.push(format!(
            "not idempotent: second pass differs at {}",
            line_col(&out, first_diff_offset(&out, &again))
        )),
        None => problems.push("refused its own output".into()),
    }
    if problems.is_empty() {
        row.formatter = "same".into();
    } else {
        row.formatter = "differs".into();
        row.findings
            .push(format!("FORMATTER\n{}", problems.join("\n")));
    }
    row
}

/// Line-by-line comparison (N-Triples, N-Quads): both parsers reject the same
/// lines, and the quads of the other lines are the same. Returns oxttl's
/// quads, or its first error when it rejects any line.
fn compare_lines(
    text: &str,
    syntax: Syntax,
    parsed: &turtle_lsp::syntax::Parsed,
    row: &mut Row,
) -> Result<Vec<oxrdf::Quad>, String> {
    let t = Instant::now();
    let (quads, ox_errors) = oxttl_line_based(text, syntax);
    row.oxttl_ms = ms(t);
    let lines = Lines::new(text);
    // Line number -> the first message about it.
    let mut ours: BTreeMap<usize, String> = BTreeMap::new();
    for e in parsed
        .errors
        .iter()
        .filter(|e| e.severity == Severity::Error)
    {
        ours.entry(lines.line(e.span.start as usize))
            .or_insert_with(|| e.message.clone());
    }
    let mut theirs: BTreeMap<usize, String> = BTreeMap::new();
    for (offset, message) in &ox_errors {
        theirs
            .entry(lines.line(*offset))
            .or_insert_with(|| message.clone());
    }
    let show = |n: usize| {
        let l = lines.text(text, n);
        let cut = l.char_indices().nth(200).map_or(l.len(), |(i, _)| i);
        format!("  line {n}: {}", &l[..cut])
    };
    let only_ours: Vec<usize> = ours
        .keys()
        .filter(|l| !theirs.contains_key(l))
        .copied()
        .collect();
    let only_theirs: Vec<usize> = theirs
        .keys()
        .filter(|l| !ours.contains_key(l))
        .copied()
        .collect();
    // Our parser keeps what it can of a line it reports; compare the others.
    let kept: Vec<&turtle_lsp::syntax::Triple> = parsed
        .triples
        .iter()
        .filter(|t| !ours.contains_key(&lines.line(t.p_span.start as usize)))
        .collect();
    let diff = compare(&kept, &quads);
    if only_ours.is_empty() && only_theirs.is_empty() {
        match diff {
            None if theirs.is_empty() => row.parser = "same".into(),
            None => {
                row.parser = format!("same (invalid lines: {})", theirs.len());
                let mut note = format!(
                    "BOTH REJECT the same lines ({}; the input has invalid lines); the first 50:",
                    theirs.len()
                );
                for (n, message) in theirs.iter().take(50) {
                    write!(
                        note,
                        "\n{}\n    oxttl: {message}\n    ours:  {}",
                        show(*n),
                        ours[n]
                    )
                    .unwrap();
                }
                row.notes.push(note);
            }
            Some(diff) => {
                row.findings.push(format!(
                    "PARSER DIFFERS from oxttl on the valid lines\n{diff}"
                ));
                row.parser = "differs".into();
            }
        }
    } else {
        let mut finding = format!(
            "INVALID LINES DIFFER: we reject {} lines oxttl accepts, oxttl rejects {} lines we accept ({} rejected by both)",
            only_ours.len(),
            only_theirs.len(),
            ours.len() - only_ours.len()
        );
        for n in only_ours.iter().take(10) {
            write!(finding, "\n{}\n    ours: {}", show(*n), ours[n]).unwrap();
        }
        for n in only_theirs.iter().take(10) {
            write!(finding, "\n{}\n    oxttl: {}", show(*n), theirs[n]).unwrap();
        }
        for (n, message) in theirs.iter().filter(|(n, _)| ours.contains_key(n)).take(10) {
            write!(
                finding,
                "\n{} (both reject)\n    oxttl: {message}\n    ours:  {}",
                show(*n),
                ours[n]
            )
            .unwrap();
        }
        if let Some(diff) = diff {
            write!(finding, "\n{diff}").unwrap();
        }
        row.findings.push(finding);
        row.parser = "invalid lines differ".into();
    }
    match ox_errors.first() {
        None => Ok(quads),
        Some((offset, message)) => Err(format!("{message} on line {}", lines.line(*offset))),
    }
}

/// oxttl's quads of an N-Triples or N-Quads file, reading on after an
/// invalid line, and the byte offset and message of each error.
fn oxttl_line_based(src: &str, syntax: Syntax) -> (Vec<oxrdf::Quad>, Vec<(usize, String)>) {
    let mut quads = Vec::new();
    let mut errors = Vec::new();
    let mut push = |r: Result<oxrdf::Quad, oxttl::TurtleSyntaxError>| match r {
        Ok(q) => quads.push(q),
        Err(e) => errors.push((e.location().start.offset as usize, e.message().to_string())),
    };
    if syntax == Syntax::NQuads {
        for q in oxttl::NQuadsParser::new().for_slice(src) {
            push(q);
        }
    } else {
        for t in oxttl::NTriplesParser::new().for_slice(src) {
            push(t.map(|t| t.in_graph(oxrdf::GraphName::DefaultGraph)));
        }
    }
    (quads, errors)
}

/// Line numbers (from 1) of byte offsets.
struct Lines(Vec<usize>);

impl Lines {
    fn new(text: &str) -> Lines {
        let mut starts = vec![0];
        starts.extend(text.match_indices('\n').map(|(i, _)| i + 1));
        Lines(starts)
    }

    fn line(&self, offset: usize) -> usize {
        self.0.partition_point(|&s| s <= offset)
    }

    fn text<'a>(&self, src: &'a str, line: usize) -> &'a str {
        let start = self.0[line - 1];
        let end = self.0.get(line).copied().unwrap_or(src.len());
        src[start..end].trim_end()
    }
}

// ---- SPARQL ----

struct SparqlCase {
    /// `sib-sparql-examples/UniProt`.
    group: String,
    /// `sib-sparql-examples/UniProt/1_select.ttl sh:select`.
    name: String,
    text: String,
    /// `sh:update`, else a query.
    update: bool,
    /// The external prefixes (from `sh:prefixes`).
    prefixes: Vec<(String, String)>,
    base: String,
}

#[derive(Default)]
struct SparqlResult {
    /// Empty for a note about a file that yields no case.
    name: String,
    bytes: usize,
    parse_ms: f64,
    spargebra_ms: f64,
    format_ms: f64,
    /// "both accept", "both reject", "we reject", "we accept"; empty when the
    /// case did not finish.
    verdict: &'static str,
    error_token: bool,
    /// "safe", "refused", "failed", "refused (syntax errors)"; empty when the
    /// case did not finish.
    formatter: &'static str,
    findings: Vec<String>,
    notes: Vec<String>,
}

/// Every SPARQL case in a SIB `sparql-examples`-style directory, and notes
/// about files that could not be read.
fn sparql_cases(dir: &Path) -> (Vec<SparqlCase>, Vec<(String, String)>) {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|e| e == "ttl") {
                out.push(p);
            }
        }
    }
    let top = dir.file_name().unwrap().to_string_lossy().to_string();
    let mut files = Vec::new();
    walk(dir, &mut files);
    files.sort();
    let shared_path = dir.join("examples/prefixes.ttl");
    let shared = fs::read_to_string(&shared_path).unwrap_or_default();
    let sh = |l: &str| oxrdf::NamedNode::new_unchecked(format!("{SH}{l}"));
    let mut cases = Vec::new();
    let mut notes = Vec::new();
    for file in files {
        let rel = file.strip_prefix(dir).unwrap();
        let rel_s = rel.to_string_lossy().replace('\\', "/");
        let folder = rel.parent().unwrap_or(Path::new(""));
        let folder = folder.strip_prefix("examples").unwrap_or(folder);
        let folder = folder.to_string_lossy().replace('\\', "/");
        let group = if folder.is_empty() {
            top.clone()
        } else {
            format!("{top}/{folder}")
        };
        let text = String::from_utf8_lossy(&fs::read(&file).unwrap()).into_owned();
        let base = format!("http://corpus.example/{top}/{rel_s}");
        let graph = match turtle_graph(&text, &base) {
            Ok(g) => g,
            Err(e) => {
                notes.push((group, format!("NOT READ as Turtle: {rel_s}: {e}")));
                continue;
            }
        };
        // (predicate, update, subject, query), in a stable order.
        let mut found = Vec::new();
        for (label, iri, update) in QUERY_PREDICATES {
            let predicate = oxrdf::NamedNode::new_unchecked(*iri);
            for t in graph.triples_for_predicate(&predicate) {
                match t.object {
                    oxrdf::TermRef::Literal(l) => found.push((
                        (*label, predicate.clone()),
                        *update,
                        t.subject.into_owned(),
                        l.value().to_string(),
                    )),
                    o => notes.push((group.clone(), format!("{rel_s}: {label} {o} is no literal"))),
                }
            }
        }
        if found.is_empty() {
            continue;
        }
        found.sort_by(|a, b| (a.0.0, a.2.to_string(), &a.3).cmp(&(b.0.0, b.2.to_string(), &b.3)));
        // The prefixes as the repository's tooling sees them: the shared
        // prefixes file, the folder's and the example read as one document.
        let wants_prefixes = found.iter().any(|f| {
            graph
                .object_for_subject_predicate(&f.2, &sh("prefixes"))
                .is_some()
        });
        let joined = if wants_prefixes {
            let own = file.with_file_name("prefixes.ttl");
            let folder_prefixes = if own != shared_path && own != file {
                fs::read_to_string(&own).unwrap_or_default()
            } else {
                String::new()
            };
            match turtle_graph(&format!("{shared}\n{folder_prefixes}\n{text}"), &base) {
                Ok(g) => Some(g),
                Err(e) => {
                    notes.push((
                        group.clone(),
                        format!(
                            "{rel_s}: prefixes not read (the example is checked without them): {e}"
                        ),
                    ));
                    None
                }
            }
        } else {
            None
        };
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for ((label, predicate), update, subject, query) in &found {
            let n = seen.entry(label).or_default();
            *n += 1;
            let mut name = format!("{top}/{rel_s} {label}");
            if *n > 1 {
                write!(name, " #{n}").unwrap();
            }
            let mut prefixes = BTreeMap::new();
            if let Some(g) = &joined {
                // Find the example's node again: by its IRI, or by its query
                // (blank node labels need not survive the second parse).
                let lit = oxrdf::Literal::new_simple_literal(query.as_str());
                let nodes: Vec<oxrdf::NamedOrBlankNodeRef> = match subject {
                    oxrdf::NamedOrBlankNode::NamedNode(n) => vec![n.into()],
                    _ => g.subjects_for_predicate_object(predicate, &lit).collect(),
                };
                for node in nodes {
                    for decls in g.objects_for_subject_predicate(node, &sh("prefixes")) {
                        let decls = match decls {
                            oxrdf::TermRef::NamedNode(n) => oxrdf::NamedOrBlankNodeRef::from(n),
                            oxrdf::TermRef::BlankNode(b) => b.into(),
                            _ => continue,
                        };
                        for d in g.objects_for_subject_predicate(decls, &sh("declare")) {
                            let d = match d {
                                oxrdf::TermRef::NamedNode(n) => oxrdf::NamedOrBlankNodeRef::from(n),
                                oxrdf::TermRef::BlankNode(b) => b.into(),
                                _ => continue,
                            };
                            let value = |p: &str| -> Vec<String> {
                                g.objects_for_subject_predicate(d, &sh(p))
                                    .map(|o| match o {
                                        oxrdf::TermRef::Literal(l) => l.value().to_string(),
                                        oxrdf::TermRef::NamedNode(n) => n.as_str().to_string(),
                                        o => o.to_string(),
                                    })
                                    .collect()
                            };
                            let (p, ns) = (value("prefix"), value("namespace"));
                            if let ([p], [ns]) = (p.as_slice(), ns.as_slice()) {
                                prefixes.insert(p.clone(), ns.clone());
                            }
                        }
                    }
                }
            }
            cases.push(SparqlCase {
                group: group.clone(),
                name,
                text: query.clone(),
                update: *update,
                prefixes: prefixes.into_iter().collect(),
                base: base.clone(),
            });
        }
    }
    (cases, notes)
}

fn turtle_graph(text: &str, base: &str) -> Result<oxrdf::Graph, String> {
    let mut g = oxrdf::Graph::new();
    for t in oxttl::TurtleParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?
        .for_slice(text)
    {
        g.insert(&t.map_err(|e| e.to_string())?);
    }
    Ok(g)
}

fn check_sparql(case: &SparqlCase) -> SparqlResult {
    let text = case.text.as_str();
    let mut r = SparqlResult {
        name: case.name.clone(),
        bytes: text.len(),
        ..Default::default()
    };
    let mut prefixes = PrefixMap::default();
    for (p, ns) in &case.prefixes {
        prefixes.insert(p.clone(), ns.clone());
    }
    let base = Some(case.base.as_str());

    let t = Instant::now();
    let doc = Document::parse(text, &prefixes, base);
    r.parse_ms = ms(t);
    let errors: Vec<String> = doc
        .diagnostics()
        .iter()
        .filter(|d| d.level == Level::Error)
        .take(5)
        .map(|d| {
            format!(
                "{} [{}] at {}",
                d.message,
                d.code,
                line_col(text, d.span.start)
            )
        })
        .collect();
    let t = Instant::now();
    let before = algebra(text, &case.prefixes, &case.base, case.update);
    r.spargebra_ms = ms(t);

    let mut problems = Vec::new();
    if doc.is_update() != case.update {
        problems.push(format!(
            "KIND: read as {}",
            if doc.is_update() {
                "an update"
            } else {
                "a query"
            }
        ));
    }
    // 1. Verdict against spargebra.
    r.verdict = match (&before, errors.is_empty()) {
        (Ok(_), true) => "both accept",
        (Err(e), false) => {
            r.notes.push(format!(
                "BOTH REJECT {}\nspargebra: {e}\nours: {}\n--- query\n{text}",
                case.name,
                errors.join("\n      ")
            ));
            "both reject"
        }
        (Ok(_), false) => {
            problems.push(format!(
                "WE REJECT a query spargebra accepts\nours: {}",
                errors.join("\n      ")
            ));
            "we reject"
        }
        (Err(e), true) => {
            problems.push(format!(
                "WE ACCEPT a query spargebra rejects\nspargebra: {e}"
            ));
            "we accept"
        }
    };
    // 2. The lexer on a valid query.
    if before.is_ok()
        && let Some(tok) = doc.tokens().iter().find(|t| t.kind == TokenKind::Error)
    {
        r.error_token = true;
        problems.push(format!(
            "ERROR TOKEN {:?} at {}",
            tok.text(text),
            line_col(text, tok.span.start)
        ));
    }
    // 3. Formatter.
    let opts = Options::default();
    let t = Instant::now();
    let out = sparql::format(text, &prefixes, base, &opts);
    r.format_ms = ms(t);
    r.formatter = match (out, errors.is_empty()) {
        (Err(_), false) => "refused (syntax errors)",
        (Err(e), true) => {
            problems.push(format!("FORMATTER REFUSED a query without errors: {e}"));
            "refused"
        }
        (Ok(out), false) => {
            problems.push(format!(
                "FORMATTER FORMATTED a query with errors\n--- output\n{out}"
            ));
            "failed"
        }
        (Ok(out), true) => {
            let mut fp = Vec::new();
            if let Ok(before) = &before {
                match algebra(&out, &case.prefixes, &case.base, case.update) {
                    Ok(after) if &after == before => {}
                    Ok(after) => fp.push(format!(
                        "algebra changes\n  before: {before}\n  after:  {after}"
                    )),
                    Err(e) => fp.push(format!("spargebra rejects the output: {e}")),
                }
            }
            if sparql_comments(text) != sparql_comments(&out) {
                fp.push(first_difference(
                    "comments change",
                    &sparql_comments(text),
                    &sparql_comments(&out),
                ));
            }
            match sparql::format(&out, &prefixes, base, &opts) {
                Ok(again) if again == out => {}
                Ok(again) => fp.push(format!(
                    "not idempotent: second pass differs at {}",
                    line_col(&out, first_diff_offset(&out, &again))
                )),
                Err(e) => fp.push(format!("refused its own output: {e}")),
            }
            if fp.is_empty() {
                "safe"
            } else {
                problems.push(format!("FORMATTER\n{}\n--- output\n{out}", fp.join("\n")));
                "failed"
            }
        }
    };
    if !problems.is_empty() {
        let external = if case.prefixes.is_empty() {
            String::new()
        } else {
            format!(" ({} external prefixes)", case.prefixes.len())
        };
        r.findings.push(format!(
            "{}{external}\n{}\n--- query\n{text}",
            case.name,
            problems.join("\n")
        ));
    }
    r
}

/// spargebra's algebra of a query or update, with generated names
/// renumbered ([`canonical_algebra`]).
fn algebra(
    text: &str,
    prefixes: &[(String, String)],
    base: &str,
    update: bool,
) -> Result<String, String> {
    let mut parser = spargebra::SparqlParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?;
    for (p, ns) in prefixes {
        parser = parser
            .with_prefix(p.as_str(), ns.as_str())
            .map_err(|e| format!("prefix {p}: {e}"))?;
    }
    // As the server gives it: local names with several dots escaped, which
    // spargebra 0.4 rejects although the grammar allows them.
    let text = turtle_lsp::sparql::validate::text_for_spargebra(
        text,
        &turtle_lsp::sparql::lexer::tokenize(text),
    );
    let s = if update {
        parser.parse_update(&text).map(|u| u.to_string())
    } else {
        parser.parse_query(&text).map(|q| q.to_string())
    };
    s.map(|s| canonical_algebra(&s)).map_err(|e| e.to_string())
}

/// The algebra as text with generated names renumbered in order of first
/// appearance: blank node labels (fresh on every parse) and the hex-named
/// variables spargebra makes up for aggregates (as `tests/w3c_sparql.rs`).
fn canonical_algebra(algebra: &str) -> String {
    let mut ids: BTreeMap<String, usize> = BTreeMap::new();
    let mut out = String::with_capacity(algebra.len());
    let mut rest = algebra;
    loop {
        let blank = rest.find("_:");
        // spargebra writes a random u128 in hex, without leading zeros.
        let hex_len = |name: &str| {
            let n = name.bytes().take_while(u8::is_ascii_hexdigit).count();
            let ends = !name[n..].starts_with(|c: char| c.is_alphanumeric() || c == '_');
            ((20..=32).contains(&n) && ends).then_some(n)
        };
        let var = rest
            .match_indices('?')
            .find_map(|(i, _)| hex_len(&rest[i + 1..]).map(|n| (i, n)));
        let (i, sigil, len) = match (blank, var) {
            (Some(b), Some((v, n))) if v < b => (v, "?", n),
            (Some(b), _) => {
                let tail = &rest[b + 2..];
                let len = tail
                    .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '-'))
                    .unwrap_or(tail.len());
                (b, "_:", len)
            }
            (None, Some((v, n))) => (v, "?", n),
            (None, None) => break,
        };
        out.push_str(&rest[..i]);
        let name = &rest[i + sigil.len()..i + sigil.len() + len];
        let n = ids.len();
        let id = *ids.entry(format!("{sigil}{name}")).or_insert(n);
        write!(out, "{sigil}g{id}").unwrap();
        rest = &rest[i + sigil.len() + len..];
    }
    out.push_str(rest);
    out
}

fn sparql_comments(src: &str) -> Vec<String> {
    let mut v: Vec<String> = sparql::lexer::tokenize(src)
        .iter()
        .filter(|t| t.kind == TokenKind::Comment)
        .map(|t| t.text(src).trim_end().to_string())
        .collect();
    v.sort();
    v
}

/// One report row: the SPARQL cases of one example folder.
#[derive(Default)]
struct SparqlRow {
    group: String,
    cases: usize,
    bytes: usize,
    parse_ms: f64,
    parse_max_ms: f64,
    spargebra_ms: f64,
    format_ms: f64,
    format_max_ms: f64,
    agree: usize,
    both_reject: usize,
    we_reject: usize,
    we_accept: usize,
    error_tokens: usize,
    safe: usize,
    refused: usize,
    failed: usize,
    refused_invalid: usize,
    unfinished: usize,
    /// Cases (or files) with a finding.
    findings: usize,
}

impl SparqlRow {
    fn of(group: &str, results: &[SparqlResult]) -> SparqlRow {
        let mut row = SparqlRow {
            group: group.to_string(),
            ..Default::default()
        };
        for r in results {
            row.findings += usize::from(!r.findings.is_empty());
            if r.name.is_empty() {
                continue;
            }
            row.add(r);
        }
        row
    }

    fn add(&mut self, r: &SparqlResult) {
        self.cases += 1;
        self.bytes += r.bytes;
        self.parse_ms += r.parse_ms;
        self.parse_max_ms = self.parse_max_ms.max(r.parse_ms);
        self.spargebra_ms += r.spargebra_ms;
        self.format_ms += r.format_ms;
        self.format_max_ms = self.format_max_ms.max(r.format_ms);
        match r.verdict {
            "both accept" => self.agree += 1,
            "both reject" => {
                self.agree += 1;
                self.both_reject += 1;
            }
            "we reject" => self.we_reject += 1,
            "we accept" => self.we_accept += 1,
            _ => self.unfinished += 1,
        }
        self.error_tokens += usize::from(r.error_token);
        match r.formatter {
            "safe" => self.safe += 1,
            "refused" => self.refused += 1,
            "failed" => self.failed += 1,
            "refused (syntax errors)" => self.refused_invalid += 1,
            _ => {}
        }
    }

    fn total(rows: &[SparqlRow]) -> SparqlRow {
        let mut t = SparqlRow {
            group: "**total**".into(),
            ..Default::default()
        };
        for r in rows {
            t.cases += r.cases;
            t.bytes += r.bytes;
            t.parse_ms += r.parse_ms;
            t.parse_max_ms = t.parse_max_ms.max(r.parse_max_ms);
            t.spargebra_ms += r.spargebra_ms;
            t.format_ms += r.format_ms;
            t.format_max_ms = t.format_max_ms.max(r.format_max_ms);
            t.agree += r.agree;
            t.both_reject += r.both_reject;
            t.we_reject += r.we_reject;
            t.we_accept += r.we_accept;
            t.error_tokens += r.error_tokens;
            t.safe += r.safe;
            t.refused += r.refused;
            t.failed += r.failed;
            t.refused_invalid += r.refused_invalid;
            t.unfinished += r.unfinished;
            t.findings += r.findings;
        }
        t
    }
}

fn line_col(text: &str, offset: u32) -> String {
    let before = &text[..(offset as usize).min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.len() - before.rfind('\n').map_or(0, |i| i + 1) + 1;
    format!("{line}:{col}")
}

fn first_diff_offset(a: &str, b: &str) -> u32 {
    a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count() as u32
}

fn first_difference(what: &str, a: &[String], b: &[String]) -> String {
    let sa: BTreeSet<&String> = a.iter().collect();
    let sb: BTreeSet<&String> = b.iter().collect();
    let gone: Vec<_> = sa.difference(&sb).take(3).collect();
    let new: Vec<_> = sb.difference(&sa).take(3).collect();
    format!(
        "{what} ({} → {}): missing {gone:?} new {new:?}",
        a.len(),
        b.len()
    )
}

fn oxttl_quads(src: &str, syntax: Syntax, base: &str) -> Result<Vec<oxrdf::Quad>, String> {
    let e = |e: oxttl::TurtleSyntaxError| e.to_string();
    let dg = oxrdf::GraphName::DefaultGraph;
    match syntax {
        Syntax::Turtle => oxttl::TurtleParser::new()
            .with_base_iri(base)
            .unwrap()
            .for_slice(src)
            .map(|t| t.map(|t| t.in_graph(dg.clone())).map_err(e))
            .collect(),
        Syntax::TriG => oxttl::TriGParser::new()
            .with_base_iri(base)
            .unwrap()
            .for_slice(src)
            .map(|q| q.map_err(e))
            .collect(),
        Syntax::NTriples => oxttl::NTriplesParser::new()
            .for_slice(src)
            .map(|t| t.map(|t| t.in_graph(dg.clone())).map_err(e))
            .collect(),
        Syntax::NQuads => oxttl::NQuadsParser::new()
            .for_slice(src)
            .map(|q| q.map_err(e))
            .collect(),
    }
}

// ---- parser comparison (as tests/oxttl_oracle.rs: ground triples as sets,
// counts and graph names in full) ----

fn node(n: &Node) -> Option<String> {
    match n {
        Node::Iri(i) => Some(format!("<{}>", &**i)),
        Node::Literal {
            value,
            datatype,
            lang,
        } => Some(match lang {
            Some(l) => format!("{value:?}@{}", &**l),
            None => format!("{value:?}^^<{}>", &**datatype),
        }),
        Node::Blank(_) | Node::Triple(_) => None,
    }
}

fn ox_term(t: &oxrdf::Term) -> Option<String> {
    use oxrdf::Term;
    match t {
        Term::NamedNode(n) => Some(format!("<{}>", n.as_str())),
        Term::Literal(l) => Some(match (l.language(), l.direction()) {
            (Some(lang), Some(dir)) => format!("{:?}@{lang}--{dir}", l.value()),
            (Some(lang), None) => format!("{:?}@{lang}", l.value()),
            _ => format!("{:?}^^<{}>", l.value(), l.datatype().as_str()),
        }),
        _ => None,
    }
}

fn graph(g: Option<GraphName>) -> String {
    match g {
        None => "DEFAULT".into(),
        Some(GraphName::Iri(i)) => format!("<{}>", &*i),
        Some(GraphName::Blank(_)) => "blank".into(),
    }
}

fn ox_graph(g: &oxrdf::GraphName) -> String {
    match g {
        oxrdf::GraphName::DefaultGraph => "DEFAULT".into(),
        oxrdf::GraphName::NamedNode(n) => format!("<{}>", n.as_str()),
        oxrdf::GraphName::BlankNode(_) => "blank".into(),
    }
}

fn compare(ours: &[&turtle_lsp::syntax::Triple], ox: &[oxrdf::Quad]) -> Option<String> {
    let mut diff = String::new();
    if ours.len() != ox.len() {
        writeln!(diff, "triple count: ours {} oxttl {}", ours.len(), ox.len()).unwrap();
    }
    let a: BTreeSet<String> = ours
        .iter()
        .filter_map(|t| {
            Some(format!(
                "{} <{}> {} {}",
                node(&t.s)?,
                &*t.p,
                node(&t.o)?,
                graph(t.graph)
            ))
        })
        .collect();
    let b: BTreeSet<String> = ox
        .iter()
        .filter_map(|q| {
            let s = match &q.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => format!("<{}>", n.as_str()),
                _ => return None,
            };
            Some(format!(
                "{s} <{}> {} {}",
                q.predicate.as_str(),
                ox_term(&q.object)?,
                ox_graph(&q.graph_name)
            ))
        })
        .collect();
    let only_ours: Vec<_> = a.difference(&b).take(5).collect();
    let only_ox: Vec<_> = b.difference(&a).take(5).collect();
    if !only_ours.is_empty() || !only_ox.is_empty() {
        writeln!(diff, "only ours: {only_ours:#?}\nonly oxttl: {only_ox:#?}").unwrap();
    }
    let mut ga: Vec<String> = ours.iter().map(|t| graph(t.graph)).collect();
    let mut gb: Vec<String> = ox.iter().map(|q| ox_graph(&q.graph_name)).collect();
    ga.sort();
    gb.sort();
    if ga != gb {
        writeln!(diff, "graph names differ").unwrap();
    }
    (!diff.is_empty()).then_some(diff)
}

// ---- formatter comparison (as tests/format.rs: blank nodes numbered in
// order of appearance, which formatting keeps) ----

fn ours(p: &turtle_lsp::syntax::Parsed) -> Vec<String> {
    let mut v: Vec<String> = p
        .triples
        .iter()
        .map(|t| format!("{:?} {:?} {:?} {:?}", t.s, t.p, t.o, t.graph))
        .collect();
    v.sort();
    v
}

fn canonical(quads: &[oxrdf::Quad]) -> Vec<String> {
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<String> = quads
        .iter()
        .map(|q| {
            let s = match &q.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => n.to_string(),
                oxrdf::NamedOrBlankNode::BlankNode(b) => bnode(&mut ids, b.as_str()),
            };
            let o = term(&mut ids, &q.object);
            let g = match &q.graph_name {
                oxrdf::GraphName::DefaultGraph => String::new(),
                oxrdf::GraphName::NamedNode(n) => format!(" {n}"),
                oxrdf::GraphName::BlankNode(b) => format!(" {}", bnode(&mut ids, b.as_str())),
            };
            format!("{s} {} {o}{g} .", q.predicate)
        })
        .collect();
    out.sort();
    out
}

fn bnode(ids: &mut HashMap<String, usize>, id: &str) -> String {
    let n = ids.len();
    format!("_:b{}", ids.entry(id.to_string()).or_insert(n))
}

fn term(ids: &mut HashMap<String, usize>, t: &oxrdf::Term) -> String {
    use oxrdf::Term;
    match t {
        Term::NamedNode(n) => n.to_string(),
        Term::BlankNode(b) => bnode(ids, b.as_str()),
        Term::Literal(l) => l.to_string(),
        Term::Triple(tr) => {
            let s = match &tr.subject {
                oxrdf::NamedOrBlankNode::NamedNode(n) => n.to_string(),
                oxrdf::NamedOrBlankNode::BlankNode(b) => bnode(ids, b.as_str()),
            };
            format!("<<( {s} {} {} )>>", tr.predicate, term(ids, &tr.object))
        }
    }
}

fn comments(src: &str) -> Vec<String> {
    let mut v: Vec<String> = tokenize(src)
        .iter()
        .filter(|t| t.kind == Kind::Comment)
        .map(|t| t.text(src).trim_end().to_string())
        .collect();
    v.sort();
    v
}

fn report(rows: &[Row], sparql: &[SparqlRow]) -> String {
    let mut s = String::from("# Corpus report\n\n");
    let bad = rows.iter().filter(|r| !r.findings.is_empty()).count();
    let triples: usize = rows.iter().map(|r| r.triples).sum();
    let bytes: usize = rows.iter().map(|r| r.bytes).sum();
    writeln!(
        s,
        "{} cases, {:.1} MB, {} triples. {} with findings (details in `target/corpus/details/`).\n",
        rows.len(),
        bytes as f64 / 1e6,
        triples,
        bad
    )
    .unwrap();
    s.push_str("| File | Size | Triples | Parse ms | oxttl ms | Format ms | Parser vs oxttl | Formatter |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---|---|\n");
    for r in rows {
        writeln!(
            s,
            "| {} | {} | {} | {:.1} | {:.1} | {:.1} | {} | {} |",
            r.name,
            size(r.bytes),
            r.triples,
            r.parse_ms,
            r.oxttl_ms,
            r.format_ms,
            r.parser,
            r.formatter
        )
        .unwrap();
    }
    if sparql.is_empty() {
        return s;
    }
    let total = SparqlRow::total(sparql);
    writeln!(
        s,
        "\n## SPARQL\n\n{} queries and updates in {} folders, {:.1} MB. {} with findings. \
         Verdict: no error diagnostic exactly when spargebra parses it. Formatter: safe \
         (same algebra, comments kept, idempotent) / refused / failed on valid queries; \
         invalid ones must be refused.\n",
        total.cases,
        sparql.len(),
        total.bytes as f64 / 1e6,
        total.findings
    )
    .unwrap();
    s.push_str("| Folder | Queries | Size | Parse ms (total / max) | spargebra ms | Format ms (total / max) | Verdict vs spargebra | Error tokens | Formatter safe / refused / failed | Invalid refused | Findings |\n");
    s.push_str("|---|---:|---:|---:|---:|---:|---|---:|---|---:|---:|\n");
    for r in sparql.iter().chain([&total]) {
        let mut verdict = format!("{}/{} agree", r.agree, r.cases);
        if r.both_reject > 0 {
            write!(verdict, " ({} both reject)", r.both_reject).unwrap();
        }
        if r.we_reject > 0 {
            write!(verdict, ", {} we reject", r.we_reject).unwrap();
        }
        if r.we_accept > 0 {
            write!(verdict, ", {} we accept", r.we_accept).unwrap();
        }
        if r.unfinished > 0 {
            write!(verdict, ", {} unfinished", r.unfinished).unwrap();
        }
        writeln!(
            s,
            "| {} | {} | {} | {:.1} / {:.1} | {:.1} | {:.1} / {:.1} | {} | {} | {} / {} / {} | {} | {} |",
            r.group,
            r.cases,
            size(r.bytes),
            r.parse_ms,
            r.parse_max_ms,
            r.spargebra_ms,
            r.format_ms,
            r.format_max_ms,
            verdict,
            r.error_tokens,
            r.safe,
            r.refused,
            r.failed,
            r.refused_invalid,
            r.findings
        )
        .unwrap();
    }
    s
}

fn size(b: usize) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", b.div_ceil(1024))
    }
}
