//! Real-world corpus check: runs the parser and the formatter over every file
//! in a directory (default `test/corpus`, filled by `tools/corpus/fetch.sh`)
//! and compares against oxttl.
//!
//!     cargo run --release --example corpus [-- DIR]
//!
//! Per file it reports:
//! - parser: our triples against oxttl's (count, ground triples, graph names),
//!   and whether both agree the file is valid;
//! - formatter: the output parses to the same quads (ours and oxttl's), keeps
//!   every comment, and formatting it again changes nothing;
//! - robustness: panics and files that take longer than the time limit;
//! - cost: parse and format time.
//!
//! RDF/XML files (`.rdf`) are converted to N-Triples first. Every Turtle file
//! under 5 MB is also serialized to TriG by oxttl (one named graph) and checked
//! as `NAME.gen.trig`, since there is little public TriG to download.
//!
//! Writes `target/corpus/report.md` and one `target/corpus/details/NAME.txt`
//! per file with a finding. Exits with 1 when any file has a finding.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use turtle_lsp::features::format::{Options, format};
use turtle_lsp::syntax::lexer::{Kind, tokenize};
use turtle_lsp::syntax::{GraphName, Node, PrefixMap, Severity, Syntax, parse_with_base};

const TIME_LIMIT: Duration = Duration::from_secs(300);
const TRIG_MAX_BYTES: usize = 5 << 20;

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
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("test/corpus"));
    let out = root.join("target/corpus");
    let _ = fs::remove_dir_all(out.join("details"));
    fs::create_dir_all(out.join("details")).unwrap();

    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{}: {e}; run tools/corpus/fetch.sh", dir.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();

    let mut rows = Vec::new();
    for path in files {
        for case in cases(&path) {
            let name = case.name.clone();
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
            if !row.findings.is_empty() {
                fs::write(
                    out.join("details").join(format!("{name}.txt")),
                    row.findings.join("\n\n"),
                )
                .unwrap();
            }
            rows.push(row);
        }
    }
    let report = report(&rows);
    fs::write(out.join("report.md"), &report).unwrap();
    println!("{report}");
    println!("Wrote {}", out.join("report.md").display());
    if rows.iter().any(|r| !r.findings.is_empty()) {
        std::process::exit(1);
    }
}

/// The cases a downloaded file yields: itself (RDF/XML as N-Triples), plus a
/// generated TriG copy of a small enough Turtle file.
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
        "ttl" | "nt" | "trig" => {
            let syntax = Syntax::from_extension(&format!("x.{ext}")).unwrap();
            let text = String::from_utf8_lossy(&bytes).into_owned();
            if syntax == Syntax::Turtle && text.len() <= TRIG_MAX_BYTES {
                match turtle_to_trig(&text, &base, &stem) {
                    Ok(trig) => out.push(Case {
                        name: format!("{stem}.gen.trig"),
                        syntax: Syntax::TriG,
                        text: trig,
                        base: base.clone(),
                    }),
                    Err(e) => eprintln!("{stem}: no TriG copy: {e}"),
                }
            }
            out.insert(
                0,
                Case {
                    name: format!("{stem}.{ext}"),
                    syntax,
                    text,
                    base,
                },
            );
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

fn turtle_to_trig(text: &str, base: &str, stem: &str) -> Result<String, String> {
    let mut parser = oxttl::TurtleParser::new()
        .with_base_iri(base)
        .map_err(|e| e.to_string())?
        .for_slice(text);
    let mut triples = Vec::new();
    for t in parser.by_ref() {
        triples.push(t.map_err(|e| e.to_string())?);
    }
    let mut ser = oxttl::TriGSerializer::new();
    for (name, iri) in parser.prefixes() {
        ser = ser.with_prefix(name, iri).map_err(|e| e.to_string())?;
    }
    let mut w = ser.for_writer(Vec::new());
    let g = oxrdf::NamedNode::new(format!("http://corpus.example/graph/{stem}")).unwrap();
    for t in triples {
        w.serialize_quad(t.in_graph(g.clone()).as_ref())
            .map_err(|e| e.to_string())?;
    }
    String::from_utf8(w.finish().map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Runs [`check`] on its own thread so a hang or a panic is one finding, not
/// the end of the run.
fn run_with_limit(case: Case) -> Row {
    let (name, bytes) = (case.name.clone(), case.text.len());
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(move || {
            let r = catch_unwind(AssertUnwindSafe(|| check(&case)));
            let _ = tx.send(r);
        })
        .unwrap();
    let failed = |what: String| Row {
        name: name.clone(),
        bytes,
        parser: "-".into(),
        formatter: "-".into(),
        findings: vec![what],
        ..Default::default()
    };
    match rx.recv_timeout(TIME_LIMIT) {
        Ok(Ok(row)) => row,
        Ok(Err(p)) => {
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            failed(format!("PANIC: {msg}"))
        }
        Err(_) => failed(format!(
            "TIMEOUT: still running after {} s",
            TIME_LIMIT.as_secs()
        )),
    }
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

    let t = Instant::now();
    let ox = oxttl_quads(text, syntax, base.unwrap());
    row.oxttl_ms = ms(t);

    // 1. Parser against oxttl.
    row.parser = match (&ox, errors.is_empty()) {
        (Ok(quads), true) => match compare(&parsed, quads) {
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

fn compare(ours: &turtle_lsp::syntax::Parsed, ox: &[oxrdf::Quad]) -> Option<String> {
    let mut diff = String::new();
    if ours.triples.len() != ox.len() {
        writeln!(
            diff,
            "triple count: ours {} oxttl {}",
            ours.triples.len(),
            ox.len()
        )
        .unwrap();
    }
    let a: BTreeSet<String> = ours
        .triples
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
    let mut ga: Vec<String> = ours.triples.iter().map(|t| graph(t.graph)).collect();
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

fn report(rows: &[Row]) -> String {
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
    s
}

fn size(b: usize) -> String {
    if b >= 1 << 20 {
        format!("{:.1} MB", b as f64 / (1 << 20) as f64)
    } else {
        format!("{} KB", b.div_ceil(1024))
    }
}
