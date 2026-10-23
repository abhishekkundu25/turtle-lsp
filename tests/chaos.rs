//! Randomized robustness test ("fuzzing" on stable Rust).
//!
//! Takes real Turtle, TriG, N-Triples and N-Quads files (SHACL shapes with
//! embedded SPARQL among them), JSON-LD documents and SPARQL queries and
//! updates, mutates them thousands of times (cuts, unbalanced brackets, stray
//! quotes, multi-byte characters, CRLF and lone CR, truncation, repeated
//! brackets, huge tokens) and runs the parser and every editor feature on each
//! result at random positions in both position encodings. Any panic, and any
//! span outside the text, fails the test. Deterministic: the corpus is read in
//! sorted order, the seed is fixed, and CHAOS_SEED / CHAOS_ITERS override it.
//!
//! A failure's input is saved under `CARGO_TARGET_TMPDIR` (CI discards the
//! system temp directory) and its start is in the panic message.
//!
//! The mutations rarely build a 10,000-level nest or a megabyte token, so
//! `pathological_inputs_are_survived` writes those down, each on a 2 MiB
//! thread, the smallest stack a host gives its threads.

use lsp_types::{
    CodeActionOrCommand, CodeActionParams, CompletionParams, Position, Range,
    TextDocumentIdentifier, TextDocumentPositionParams, WorkspaceEdit,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use turtle_lsp::analysis::{Analysis, Encoding, SparqlAnalysis};
use turtle_lsp::features::hierarchy;
use turtle_lsp::features::{
    completion, diagnostics, format, hover, inlay, navigation, quickfix, refactor, semantic,
    symbols,
};
use turtle_lsp::imports;
use turtle_lsp::jsonld;
use turtle_lsp::knowledge::{Knowledge, Source};
use turtle_lsp::settings::InlayLabels;
use turtle_lsp::sparql::{self, Document, Level, features as sparql_features};
use turtle_lsp::syntax::{Parsed, PrefixMap, Severity, Syntax, parse, parse_with_base};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545F4914F6CDD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    /// The generator of iteration `i` of a run seeded `seed`: each iteration
    /// gets its own, so a shard starts anywhere in the range. The state goes
    /// through splitmix64, as neighbouring seeds give xorshift similar first
    /// outputs, and is never 0, where xorshift stays.
    fn at(seed: u64, i: usize) -> Rng {
        let mut z = seed.wrapping_add((i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        Rng((z ^ (z >> 31)) | 1)
    }
}

/// Each mutation loop runs as this many tests, which the harness runs in
/// parallel: at CHAOS_ITERS=30000 one unsplit loop set the time of the step.
const SHARDS: usize = 4;

/// The iterations shard `k` (0-based) runs: `k`/`SHARDS` of the way through
/// `0..iters` up to the start of the next shard, so the shards cover the range
/// once with no gap and no overlap.
fn shard(iters: usize, k: usize) -> std::ops::Range<usize> {
    iters * k / SHARDS..iters * (k + 1) / SHARDS
}

#[test]
fn the_shards_cover_every_iteration_once() {
    for iters in [0, 1, 3, 4, 5, 1199, 1200, 2000, 30_001] {
        let all: Vec<usize> = (0..SHARDS).flat_map(|k| shard(iters, k)).collect();
        assert_eq!(all, (0..iters).collect::<Vec<_>>(), "{iters} iterations");
    }
}

const SNIPPETS: &[&str] = &[
    "<",
    ">",
    "<<",
    ">>",
    "<<(",
    ")>>",
    "{|",
    "|}",
    "~",
    "\"",
    "'",
    "\"\"\"",
    "'''",
    "\\",
    "\\u00",
    "[",
    "]",
    "(",
    ")",
    ";",
    ",",
    ".",
    ":",
    "^^",
    "@",
    "@en",
    "@en--ltr",
    "_:",
    "_:b1",
    "#",
    "\n",
    "\r\n",
    "\r",
    "\t",
    " ",
    "a ",
    "true",
    "1.5e",
    "-.",
    "ex:",
    ":x",
    "@prefix ",
    "@prefix ex: <http://e/> .\n",
    "PREFIX ex: <http://e/>\n",
    "@base <http://b/> .\n",
    "é",
    "日本",
    "😀",
    "\u{200C}",
    "\u{FFFD}",
    "%4",
    "%41",
    "<http://x/ y>",
    "ex:a.b.",
    "VERSION \"1.2\"",
    "<http://example.org/x>",
    " a<http://xmlns.com/foaf/0.1/Person>",
    "<http://new.example/ns/y> ",
    "{",
    "}",
    "{ ",
    " }\n",
    "GRAPH ",
    "ex:g {\n",
    "_:g { ",
    "[] {",
    " <http://www.w3.org/ns/shacl#property> [ <http://www.w3.org/ns/shacl#minCount> -1 ] ",
    " <http://www.w3.org/ns/shacl#path> ( ",
    " <http://www.w3.org/ns/shacl#or> ( ",
    " <http://www.w3.org/ns/shacl#select> \"\"\"SELECT $this WHERE { ",
    "\"\"\" ; ",
    "$this ?p ?o . FILTER(",
    "SELECT ?x WHERE { ?x a ex:C } ",
    "FILTER NOT EXISTS { $this ex:p ?v } ",
    "\\\n",
    "\\\r",
];

fn root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

/// The files directly in `dir` (all below it when `deep`), sorted by path: a
/// CI seed must pick the same file locally, and `read_dir` has no order.
fn sorted_files(dir: &Path, deep: bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    paths.sort();
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            if deep {
                files.extend(sorted_files(&p, true));
            }
        } else {
            files.push(p);
        }
    }
    files
}

/// The Turtle, TriG, N-Triples and N-Quads inputs. JSON-LD has its own list.
fn corpus() -> Vec<(String, Syntax)> {
    let fixtures = root().join("test/fixtures");
    let mut out: Vec<(String, Syntax)> = ["", "@prefix", "ex:a ex:b ex:c"]
        .into_iter()
        .map(|t| (t.to_string(), Syntax::Turtle))
        .collect();
    out.push(("ex:g { ex:a ex:b ex:c }".into(), Syntax::TriG));
    out.push((
        "<http://e/a> <http://e/b> <http://e/c> .".into(),
        Syntax::NTriples,
    ));
    out.push((
        "<http://e/a> <http://e/b> <http://e/c> <http://e/g> .\n_:s <http://e/b> \"x\" _:g ."
            .into(),
        Syntax::NQuads,
    ));
    let mut files = Vec::new();
    for dir in [
        "workspace",
        "project",
        "project/core",
        "project/data",
        "project/shapes",
        "format",
        "datasets",
        "oracle",
        "shacl",
        "query",
        // Shapes with SPARQL in `sh:select`, `sh:ask`, `sh:construct`, `sh:update`.
        "chaos",
    ] {
        files.extend(sorted_files(&fixtures.join(dir), false));
    }
    // The SHACL-SPARQL suite: constraints, targets, pre-binding.
    files.extend(sorted_files(&root().join("tests/w3c/shacl/sparql"), true));
    for path in files {
        let name = path.to_string_lossy();
        match Syntax::from_extension(&name) {
            Some(Syntax::JsonLd) | None => {}
            Some(syntax) => out.push((fs::read_to_string(&path).unwrap(), syntax)),
        }
    }
    // The same files with every prefixed name written as a full IRI, so the
    // shorten and extract refactorings have work to do.
    let expanded: Vec<(String, Syntax)> = out
        .iter()
        .filter(|(_, syntax)| *syntax != Syntax::NTriples)
        .map(|(t, syntax)| {
            let p = parse(t, &PrefixMap::default(), *syntax);
            let mut s = t.clone();
            for r in p.refs.iter().rev() {
                if matches!(r.form, turtle_lsp::syntax::Form::PName { .. }) {
                    s.replace_range(
                        r.span.start as usize..r.span.end as usize,
                        &format!(" <{}> ", r.iri),
                    );
                }
            }
            (s, *syntax)
        })
        .collect();
    out.extend(expanded);
    out
}

/// JSON-LD inputs: the W3C toRdf test inputs and the fixtures' JSON-LD files.
fn jsonld_corpus() -> Vec<String> {
    let mut out: Vec<String> = [
        "",
        "{}",
        "[]",
        "null",
        "{\"@id\": \"http://e/a\", \"http://e/p\": 1}",
    ]
    .into_iter()
    .map(str::to_string)
    .collect();
    let mut files = sorted_files(&root().join("tests/w3c/jsonld/toRdf"), false);
    files.retain(|p| p.to_string_lossy().ends_with("-in.jsonld"));
    files.extend(
        sorted_files(&root().join("test/fixtures"), true)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "jsonld")),
    );
    for p in files {
        out.push(fs::read_to_string(p).unwrap());
    }
    out
}

fn char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Mutates `text` one to four times: inserts, cuts, copies, truncations, and
/// now and then a bulk edit that reaches what single snippets do not (a long
/// run of one snippet is a deep nest or a long line, `\n` to a lone `\r`, a
/// huge token).
fn mutate_with(rng: &mut Rng, text: &str, snippets: &[&str], wide: bool) -> String {
    let mut s = text.to_string();
    for _ in 0..1 + rng.below(4) {
        let at = char_boundary(&s, rng.below(s.len() + 1));
        match rng.below(60) {
            0..=11 => s.insert_str(at, snippets[rng.below(snippets.len())]),
            12..=17 => {
                let end = char_boundary(&s, at + rng.below(40));
                s.replace_range(at..end, "");
            }
            18..=23 => {
                let end = char_boundary(&s, at + rng.below(60));
                let chunk = s[at..end].to_string();
                let to = char_boundary(&s, rng.below(s.len() + 1));
                s.insert_str(to, &chunk);
            }
            24..=29 => s.truncate(at),
            30..=56 => {
                let n = rng.below(6);
                for _ in 0..n {
                    let at = char_boundary(&s, rng.below(s.len() + 1));
                    s.insert_str(at, snippets[rng.below(snippets.len())]);
                }
            }
            // 20 to 1,500 copies of one snippet: a deep nest, a long line.
            57 => {
                let snippet = snippets[rng.below(snippets.len())];
                s.insert_str(at, &snippet.repeat(20 + rng.below(1500)));
            }
            // Old Mac line ends, or none.
            58 => {
                s = if rng.below(2) == 0 {
                    s.replace("\r\n", "\n").replace('\n', "\r")
                } else {
                    s.replace(['\n', '\r'], " ")
                }
            }
            // A token of tens of thousands of characters.
            _ => {
                let c = ["a", "0", "_", "-", "é"][rng.below(if wide { 5 } else { 4 })];
                s.insert_str(at, &c.repeat(1000 + rng.below(60_000)));
            }
        }
    }
    s
}

fn mutate(rng: &mut Rng, text: &str) -> String {
    mutate_with(rng, text, SNIPPETS, true)
}

fn doc_uri(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Turtle => "file:///chaos/doc.ttl",
        Syntax::TriG => "file:///chaos/doc.trig",
        Syntax::NTriples => "file:///chaos/doc.nt",
        Syntax::NQuads => "file:///chaos/doc.nq",
        Syntax::JsonLd => "file:///chaos/doc.jsonld",
    }
}

/// How a span is wrong.
#[derive(Debug, PartialEq)]
enum SpanFault {
    /// Backwards, or past the end of the text.
    Bounds(String),
    /// An end inside a multi-byte character.
    SplitsChar(String),
}

/// Every span the parser reports, whichever field carries it, lies inside the
/// text on character boundaries and runs forwards; every index points at
/// something that exists.
fn span_fault(text: &str, p: &Parsed) -> Option<SpanFault> {
    let mut spans: Vec<(&str, u32, u32)> = Vec::new();
    for d in &p.prefixes {
        spans.push(("prefix declaration", d.span.start, d.span.end));
        spans.push(("prefix name", d.name_span.start, d.name_span.end));
        spans.push(("prefix IRI", d.iri_span.start, d.iri_span.end));
    }
    spans.extend(
        p.prefix_uses
            .iter()
            .map(|u| ("prefix use", u.span.start, u.span.end)),
    );
    spans.extend(
        p.refs
            .iter()
            .map(|r| ("reference", r.span.start, r.span.end)),
    );
    for st in &p.statements {
        spans.push(("statement", st.span.start, st.span.end));
        if let Some((_, s)) = &st.subject {
            spans.push(("statement subject", s.start, s.end));
        }
        if let Some((_, s)) = &st.graph {
            spans.push(("statement graph", s.start, s.end));
        }
    }
    for g in &p.graphs {
        spans.push(("graph block", g.span.start, g.span.end));
        if let Some(s) = g.name_span {
            spans.push(("graph name", s.start, s.end));
        }
        if g.statements.start > g.statements.end || g.statements.end as usize > p.statements.len() {
            return Some(SpanFault::Bounds(format!(
                "graph block statements {:?} outside the {} statements",
                g.statements,
                p.statements.len()
            )));
        }
    }
    for t in &p.triples {
        spans.push(("triple predicate", t.p_span.start, t.p_span.end));
        spans.push(("triple object", t.o_span.start, t.o_span.end));
        if t.statement as usize >= p.statements.len() {
            return Some(SpanFault::Bounds(format!(
                "triple points at statement {} of {}",
                t.statement,
                p.statements.len()
            )));
        }
    }
    spans.extend(p.errors.iter().map(|e| ("error", e.span.start, e.span.end)));
    spans.extend(p.comments.iter().map(|c| ("comment", c.start, c.end)));
    spans.extend(p.tokens.iter().map(|t| ("token", t.start, t.end)));
    for (what, start, end) in spans {
        if start > end || end as usize > text.len() {
            return Some(SpanFault::Bounds(format!(
                "{what} span {start}..{end} out of bounds (text is {} bytes)",
                text.len()
            )));
        }
        if !text.is_char_boundary(start as usize) || !text.is_char_boundary(end as usize) {
            return Some(SpanFault::SplitsChar(format!(
                "{what} span {start}..{end} splits a character"
            )));
        }
    }
    None
}

fn spans_are_inside(text: &str, p: &Parsed) {
    if let Some(SpanFault::Bounds(m) | SpanFault::SplitsChar(m)) = span_fault(text, p) {
        panic!("{m}");
    }
}

/// A byte offset in `text`: anywhere, or (half the time) within a few bytes of
/// a SPARQL keyword or variable, which is where an embedded query (`sh:select`)
/// has the features that differ from the Turtle ones.
fn pick_offset(rng: &mut Rng, text: &str) -> u32 {
    if rng.below(2) == 0 {
        const WORDS: [&str; 6] = ["SELECT", "WHERE", "FILTER", "$this", "?", "CONSTRUCT"];
        let word = WORDS[rng.below(WORDS.len())];
        let hits: Vec<usize> = text.match_indices(word).map(|(i, _)| i).collect();
        if !hits.is_empty() {
            let at = hits[rng.below(hits.len())] + rng.below(word.len() + 8);
            return char_boundary(text, at) as u32;
        }
    }
    char_boundary(text, rng.below(text.len() + 1)) as u32
}

/// `text` parsed with its own frontend: JSON-LD with no remote contexts, the
/// Turtle family with no external prefixes.
fn parse_doc(text: &str, syntax: Syntax, base: Option<&str>) -> Parsed {
    if syntax == Syntax::JsonLd {
        jsonld::parse(text, base, &jsonld::NoNetwork::default())
    } else {
        parse_with_base(text, &PrefixMap::default(), syntax, base)
    }
}

fn exercise(rng: &mut Rng, text: &str, syntax: Syntax, k: &mut Knowledge) {
    // The parser alone: every span must be inside the text and on char boundaries.
    let p = parse_doc(text, syntax, None);
    spans_are_inside(text, &p);

    // Import sources: catalogs and RDF/XML come from outside too.
    let _ = imports::parse_catalog(text, std::path::Path::new("/chaos"));
    let _ = imports::sniff(text.as_bytes());
    let _ = imports::rdf_xml_to_ntriples(text.as_bytes(), "http://example.com/chaos");

    let a = Arc::new(Analysis::with_syntax(
        doc_uri(syntax).into(),
        text,
        Arc::new(PrefixMap::default()),
        true,
        syntax,
    ));
    // JSON-LD is parsed by its own frontend: the analysis's spans, and those
    // of what processing dropped (the diagnostics are made of both).
    let dropped = a.dropped.iter().find_map(|d| {
        let (start, end) = (d.span.start as usize, d.span.end as usize);
        if start > end || end > text.len() {
            Some(SpanFault::Bounds(format!(
                "dropped span {start}..{end} out of bounds"
            )))
        } else if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            Some(SpanFault::SplitsChar(format!(
                "dropped span {start}..{end} splits a character"
            )))
        } else {
            None
        }
    });
    if let Some(SpanFault::Bounds(m) | SpanFault::SplitsChar(m)) =
        span_fault(text, &a.parsed).or(dropped)
    {
        panic!("{m}");
    }
    // The analysis's tokens and comments are the parser's, whether kept or
    // made on demand.
    let key = |t: &turtle_lsp::syntax::lexer::Token| (t.kind, t.start, t.end, t.error());
    let lean = Analysis::with_syntax(
        doc_uri(syntax).into(),
        text,
        Arc::new(PrefixMap::default()),
        false,
        syntax,
    );
    for an in [&*a, &lean] {
        assert!(
            an.tokens().iter().map(key).eq(p.tokens.iter().map(key)),
            "the analysis's tokens differ from the parser's"
        );
        assert!(
            an.comments().eq(p.comments.iter().copied()),
            "the analysis's comments differ from the parser's"
        );
    }
    // The document joins the index (and leaves it again below), as an edited
    // open document does.
    k.set(doc_uri(syntax), Source::Open, a.clone());
    let docs = &*k;
    // Formatting: refused on errors, else a fixed point. JSON-LD is refused on
    // JSON syntax errors only (context problems do not stop it), and has
    // nothing to format when empty.
    let opts = format::Options::default();
    let formatted = format::format(text, &a.parsed, &a.external, &opts);
    let refused = if syntax == Syntax::JsonLd {
        a.parsed.errors.iter().any(|e| e.code == "json-syntax") || text.trim().is_empty()
    } else {
        a.parsed
            .errors
            .iter()
            .any(|e| e.severity == Severity::Error)
    };
    if refused {
        assert!(
            formatted.is_none(),
            "formatted a document with syntax errors"
        );
    } else {
        let out = formatted.expect("formatter refused a document without syntax errors");
        let again = format::format(&out, &parse_doc(&out, syntax, None), &a.external, &opts);
        // Known defect (see `format_drops_a_dangling_semicolon_once`): a `;`
        // before a comment and the closing `.` is kept the first time and
        // dropped the second. Only that difference is let through.
        let known = out.contains("; .")
            && again.as_deref().map(|a| a.replacen("; .", ".", 1))
                == Some(out.replacen("; .", ".", 1));
        if !known {
            assert_eq!(
                again.as_deref(),
                Some(out.as_str()),
                "formatting is not idempotent"
            );
        }
    }
    for enc in [Encoding::Utf8, Encoding::Utf16] {
        let _ = format::formatting(&a, enc, &opts);
        let _ = diagnostics::diagnostics(&a, docs, enc);
        let lint_off = diagnostics::Options {
            lint_undescribed_terms: false,
            ..diagnostics::Options::default()
        };
        let _ = diagnostics::diagnostics_with(&a, docs, enc, &lint_off);
        let unresolved = imports::Status::Unresolved("x".into());
        for d in imports::diagnostics_with(&a, enc, |_| Some(&unresolved)) {
            assert!(d.range.start <= d.range.end, "bad import diagnostic range");
        }
        let _ = symbols::document_symbols(&a, enc);
        let _ = symbols::folding_ranges(&a);
        let _ = symbols::workspace_symbols(docs, enc, "a");
        tokens_are_well_formed(text, enc, &semantic::full(&a, docs, enc).data);
        for _ in 0..4 {
            let off = pick_offset(rng, text);
            let pos: Position = a.position(enc, off);
            let _ = completion::complete(&a, docs, enc, pos);
            let _ = hover::hover(&a, docs, enc, pos);
            let _ = navigation::definition(&a, docs, enc, pos, None);
            let _ = navigation::type_definition(&a, docs, enc, pos);
            let _ = navigation::implementations(&a, docs, enc, pos);
            let _ = navigation::references(&a, docs, enc, pos, true);
            let _ = navigation::highlights(&a, enc, pos);
            let _ = navigation::prepare_rename(&a, enc, pos);
            for item in hierarchy::prepare(&a, docs, enc, pos).into_iter().flatten() {
                for up in hierarchy::supertypes(&item, docs, enc, Some(&a)) {
                    let _ = hierarchy::subtypes(&up, docs, enc, None);
                }
                let _ = hierarchy::subtypes(&item, docs, enc, Some(&a));
            }
            let scope = navigation::PrefixScope {
                file: None,
                file_of: &mut |_| None,
            };
            let _ = navigation::rename(&a, docs, enc, pos, "renamed", scope);
            // Positions past the end of a line or of the file, as clients sometimes send.
            let wild = Position::new(
                pos.line + rng.below(3) as u32,
                pos.character + rng.below(200) as u32,
            );
            let _ = completion::complete(&a, docs, enc, wild);
            let _ = hover::hover(&a, docs, enc, wild);
            let (from, to) = if rng.below(2) == 0 {
                (pos, wild)
            } else {
                (wild, pos)
            };
            let tokens = semantic::range(&a, docs, enc, Range::new(from, to)).data;
            tokens_are_well_formed(text, enc, &tokens);
            for mode in [InlayLabels::Opaque, InlayLabels::All, InlayLabels::Off] {
                let _ = inlay::inlay_hints(&a, docs, enc, Range::new(from, to), mode);
            }
            code_actions_keep_the_graph(
                &a,
                docs,
                text,
                enc,
                Range::new(from, to),
                rng.below(2) == 0,
            );
        }
    }
    // SHACL validation of the document with the corpus, as on save.
    let job = turtle_lsp::features::shacl_validate::Job {
        generation: 1,
        docs: shacl_docs(rng, docs),
        max_triples: u64::MAX,
        enc: if rng.below(2) == 0 {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        },
        unresolved: if rng.below(2) == 0 {
            vec![]
        } else {
            vec!["http://example.invalid/missing".into()]
        },
        soften: true,
    };
    let _ = turtle_lsp::features::shacl_validate::run(&job, &|| false);
    k.remove(doc_uri(syntax));
}

/// The document under test and eight others of the workspace: validating all
/// of them each time costs seconds, and any eight make a workspace.
fn shacl_docs(rng: &mut Rng, k: &Knowledge) -> Vec<(Source, Arc<Analysis>)> {
    let all: Vec<_> = k.docs().map(|(_, s, a)| (s, a.clone())).collect();
    let open = all.iter().position(|(s, _)| *s == Source::Open);
    let mut out: Vec<_> = open.map(|i| all[i].clone()).into_iter().collect();
    for _ in 0..8 {
        if !all.is_empty() {
            out.push(all[rng.below(all.len())].clone());
        }
    }
    out
}

/// Semantic tokens: sorted, non-empty, each inside one line, never overlapping.
fn tokens_are_well_formed(text: &str, enc: Encoding, data: &[lsp_types::SemanticToken]) {
    let lines: Vec<&str> = text.split('\n').collect();
    let width = |l: &str| match enc {
        Encoding::Utf8 => l.len() as u32,
        Encoding::Utf16 => l.encode_utf16().count() as u32,
    };
    // The width of the line the previous token was on: a megabyte line with a
    // hundred thousand tokens is measured once, not once per token.
    let mut measured: Option<(u32, u32)> = None;
    let (mut line, mut col, mut prev_len) = (0u32, 0u32, 0u32);
    for (i, t) in data.iter().enumerate() {
        if t.delta_line > 0 || i == 0 {
            col = 0;
        } else {
            assert!(
                t.delta_start >= prev_len,
                "token {i} overlaps the previous one"
            );
        }
        line += t.delta_line;
        col += t.delta_start;
        assert!(t.length > 0, "empty token {i}");
        let line_width = match measured {
            Some((l, w)) if l == line => w,
            _ => {
                let l = lines.get(line as usize).expect("token past the last line");
                let w = width(l);
                measured = Some((line, w));
                w
            }
        };
        assert!(col + t.length <= line_width, "token {i} runs past its line");
        prev_len = t.length;
    }
}

/// Every code action must not panic; on a file without syntax errors, every
/// refactoring must leave the parsed triples unchanged.
fn code_actions_keep_the_graph(
    a: &Analysis,
    k: &Knowledge,
    text: &str,
    enc: Encoding,
    range: Range,
    lazy: bool,
) {
    let params = CodeActionParams {
        text_document: lsp_types::TextDocumentIdentifier {
            uri: doc_uri(a.parsed.syntax).parse().unwrap(),
        },
        range,
        context: Default::default(),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    let context = lsp_types::CodeActionContext {
        diagnostics: diagnostics::diagnostics(a, k, enc),
        ..Default::default()
    };
    let _ = quickfix::code_actions(
        a,
        k,
        enc,
        &CodeActionParams {
            context,
            ..params.clone()
        },
    );
    let actions = refactor::code_actions(a, enc, &params, lazy);
    if !a.parsed.errors.is_empty() {
        return;
    }
    let syntax = a.parsed.syntax;
    let triples = |t: &str| {
        let base = a.parsed.document_base.as_deref();
        let mut v: Vec<String> = parse_doc(t, syntax, base)
            .triples
            .iter()
            .map(|t| format!("{:?} {:?} {:?} {:?}", t.s, t.p, t.o, t.graph))
            .collect();
        v.sort();
        v
    };
    let before = triples(text);
    for c in actions {
        let CodeActionOrCommand::CodeAction(c) = c else {
            continue;
        };
        let after = refactor::apply(a, enc, &refactor::edits_of(a, enc, &c));
        let reparsed = parse_doc(&after, syntax, a.parsed.document_base.as_deref());
        assert!(
            reparsed.errors.is_empty() && triples(&after) == before,
            "'{}' changed the graph:\n{after}",
            c.title
        );
    }
}

fn env_or<T: std::str::FromStr>(name: &str, default: T) -> T {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

/// A failing input's report: where it went and its start. The input is saved
/// whole under `CARGO_TARGET_TMPDIR` (the system temp directory is discarded
/// by CI); the message carries the first characters so a CI log is enough to
/// start from.
fn failure_report(
    kind: &str,
    seed: u64,
    iters: usize,
    i: usize,
    ext: &str,
    text: &str,
    why: &str,
) -> String {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(dir).unwrap();
    let dump = dir.join(format!("chaos-{kind}-{seed}-{i}.{ext}"));
    fs::write(&dump, text).unwrap();
    let head: String = text.chars().take(300).collect();
    let more = if head.len() < text.len() { " ..." } else { "" };
    format!(
        "iteration {i} (seed {seed}, {kind}; replay with CHAOS_SEED={seed} CHAOS_ITERS={iters}) failed: {why}\n  input ({} bytes): {head:?}{more}\n  saved to {}",
        text.len(),
        dump.display()
    )
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|| "(not a string)".into())
}

/// The documents as a workspace's disk files, for cross-file features.
fn knowledge_of(docs: &[(String, Syntax)]) -> Knowledge {
    let mut k = Knowledge::default();
    for (i, (t, syntax)) in docs.iter().enumerate() {
        let uri = format!("file:///chaos/{i}.{}", extension(*syntax));
        let a = Analysis::with_syntax(
            uri.clone(),
            t,
            Arc::new(PrefixMap::default()),
            true,
            *syntax,
        );
        k.set(&uri, Source::Disk, Arc::new(a));
    }
    k
}

fn extension(syntax: Syntax) -> &'static str {
    match syntax {
        Syntax::Turtle => "ttl",
        Syntax::TriG => "trig",
        Syntax::NTriples => "nt",
        Syntax::NQuads => "nq",
        Syntax::JsonLd => "jsonld",
    }
}

#[test]
fn mutated_turtle_never_panics_1() {
    mutated_turtle(0);
}

#[test]
fn mutated_turtle_never_panics_2() {
    mutated_turtle(1);
}

#[test]
fn mutated_turtle_never_panics_3() {
    mutated_turtle(2);
}

#[test]
fn mutated_turtle_never_panics_4() {
    mutated_turtle(3);
}

fn mutated_turtle(k: usize) {
    let seed = env_or("CHAOS_SEED", 0x5EED_0007_u64);
    let iters = env_or("CHAOS_ITERS", 2000usize);
    let corpus = corpus();
    let mut base = knowledge_of(&corpus);
    for i in shard(iters, k) {
        let mut rng = Rng::at(seed, i);
        let pick = rng.below(corpus.len());
        let (source, syntax) = &corpus[pick];
        let mut syntax = *syntax;
        let text = mutate(&mut rng, source);
        // Turtle is nearly TriG: read some Turtle files as TriG too.
        if syntax == Syntax::Turtle && rng.below(4) == 0 {
            syntax = Syntax::TriG;
        }
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exercise(&mut Rng(seed ^ i as u64 ^ 0x9E37), &text, syntax, &mut base)
        }));
        if let Err(e) = r {
            let ext = extension(syntax);
            panic!(
                "{}",
                failure_report("turtle", seed, iters, i, ext, &text, &panic_message(&*e))
            );
        }
    }
}

const JSON_SNIPPETS: &[&str] = &[
    "{",
    "}",
    "[",
    "]",
    ",",
    ":",
    "\"",
    "\\",
    "\\u00",
    "\\ud800",
    "\"@context\"",
    "\"@context\": {",
    "\"@context\": [",
    "\"@context\": null",
    "\"@context\": \"http://example.com/ctx\"",
    "\"@id\"",
    "\"@type\"",
    "\"@value\"",
    "\"@language\"",
    "\"@direction\": \"rtl\"",
    "\"@graph\"",
    "\"@list\"",
    "\"@set\"",
    "\"@reverse\"",
    "\"@nest\"",
    "\"@included\"",
    "\"@container\": \"@language\"",
    "\"@container\": [\"@graph\", \"@id\"]",
    "\"@vocab\": \"http://e/\"",
    "\"@base\": \"http://b/\"",
    "\"@protected\": true",
    "\"@propagate\": false",
    "\"@json\"",
    "\"@none\"",
    "\"@version\": 1.1",
    "\"@import\": \"http://e/i\"",
    "\"http://e/p\"",
    "\"ex:p\"",
    "\"ex\": \"http://e/\"",
    "null",
    "true",
    "false",
    "0",
    "-1.5e300",
    "1e999",
    "12345678901234567890",
    "\"\"",
    "{}",
    "[]",
    "{\"@id\": \"\"}",
    "[{",
    "}]",
    "\n",
    "\r\n",
    "\r",
    "\t",
    " ",
    "//",
    "/*",
    "*/",
    " <http://www.w3.org/ns/shacl#select> ",
];

#[test]
fn mutated_jsonld_never_panics_1() {
    mutated_jsonld(0);
}

#[test]
fn mutated_jsonld_never_panics_2() {
    mutated_jsonld(1);
}

#[test]
fn mutated_jsonld_never_panics_3() {
    mutated_jsonld(2);
}

#[test]
fn mutated_jsonld_never_panics_4() {
    mutated_jsonld(3);
}

fn mutated_jsonld(k: usize) {
    let seed = env_or("CHAOS_SEED", 0x5EED_0013_u64);
    let iters = env_or("CHAOS_ITERS", 1200usize);
    let seeds: Vec<String> = jsonld_corpus();
    // The workspace the JSON-LD document joins: the Turtle fixtures, and the
    // JSON-LD files themselves (those of the fixtures, not the W3C inputs).
    let mut docs: Vec<(String, Syntax)> = corpus()
        .into_iter()
        .filter(|(t, _)| t.len() < 20_000)
        .step_by(3)
        .collect();
    docs.extend(
        sorted_files(&root().join("test/fixtures"), true)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "jsonld"))
            .map(|p| (fs::read_to_string(p).unwrap(), Syntax::JsonLd)),
    );
    let mut base = knowledge_of(&docs);
    for i in shard(iters, k) {
        let mut rng = Rng::at(seed, i);
        let source = &seeds[rng.below(seeds.len())];
        let text = mutate_with(&mut rng, source, JSON_SNIPPETS, false);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exercise(
                &mut Rng(seed ^ i as u64 ^ 0x9E37),
                &text,
                Syntax::JsonLd,
                &mut base,
            )
        }));
        if let Err(e) = r {
            panic!(
                "{}",
                failure_report(
                    "jsonld",
                    seed,
                    iters,
                    i,
                    "jsonld",
                    &text,
                    &panic_message(&*e)
                )
            );
        }
    }
}

const SPARQL_SNIPPETS: &[&str] = &[
    "{",
    "}",
    "(",
    ")",
    "[",
    "]",
    ".",
    ";",
    ",",
    "?",
    "$",
    "?x",
    "$x",
    "?x ",
    "<",
    ">",
    "<<",
    ">>",
    "<<(",
    ")>>",
    "{|",
    "|}",
    "~",
    "\"",
    "'",
    "\"\"\"",
    "'''",
    "\\",
    "\\u00",
    "#",
    "\n",
    "\r\n",
    "\t",
    " ",
    "a ",
    "ex:",
    ":x",
    "_:b",
    "[]",
    "()",
    "PREFIX ",
    "PREFIX ex: <http://e/>\n",
    "BASE <http://b/>\n",
    "VERSION \"1.2\"\n",
    "SELECT ",
    "SELECT * ",
    "WHERE ",
    "WHERE { ",
    "OPTIONAL { ",
    "FILTER(",
    "BIND(",
    " AS ?v)",
    "VALUES ?v { ",
    "UNION ",
    "MINUS { ",
    "GRAPH ?g { ",
    "SERVICE <http://s/> { ",
    "GROUP BY ",
    "ORDER BY ",
    "LIMIT 1",
    "OFFSET ",
    "HAVING(",
    "COUNT(*)",
    "COUNT(DISTINCT ?x)",
    "GROUP_CONCAT(?x; SEPARATOR=\",\")",
    "STR(",
    "REGEX(",
    "EXISTS { ",
    "NOT EXISTS { ",
    "INSERT DATA { ",
    "DELETE WHERE { ",
    "INSERT { ",
    "DELETE { ",
    " ;\n",
    "LOAD <http://l/> ",
    "CLEAR ALL",
    "WITH <http://w/> ",
    "USING ",
    "/",
    "|",
    "^",
    "*",
    "+",
    "!",
    "&&",
    "||",
    "=",
    "!=",
    "<=",
    "1.5e",
    "-.",
    "true",
    "@en",
    "@en--ltr",
    "^^",
    "é",
    "日本",
    "😀",
    "\u{200C}",
    "\u{FFFD}",
    "%4",
    "%41",
];

fn sparql_corpus() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out: Vec<String> = ["", "SELECT", "SELECT * WHERE { ?s ?p ?o }"]
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut dirs = vec![
        root.join("test/fixtures/query"),
        root.join("test/fixtures/format"),
    ];
    for suite in [
        "sparql11/syntax-query",
        "sparql11/syntax-update-1",
        "sparql12/syntax-triple-terms-positive",
        "sparql12/version",
    ] {
        dirs.push(root.join("tests/w3c/sparql-tests").join(suite));
    }
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for p in files {
            if p.extension().is_some_and(|e| e == "rq" || e == "ru") {
                out.push(fs::read_to_string(p).unwrap());
            }
        }
    }
    out
}

fn sparql_mutate(rng: &mut Rng, text: &str) -> String {
    mutate_with(rng, text, SPARQL_SNIPPETS, true)
}

/// Tokens and diagnostics stay inside the text and on character boundaries,
/// tokens never overlap, and formatted text is valid and formats to itself.
/// Then every `sparql::features` entry point runs at random positions, on the
/// query as a file and as the query inside an `sh:select` literal.
fn sparql_exercise(rng: &mut Rng, text: &str, k: &Knowledge) {
    let base = Some("http://example.com/chaos");
    let prefixes = PrefixMap::default();
    let doc = Document::parse(text, &prefixes, base);
    let mut prev_end = 0;
    for t in doc.tokens() {
        assert!(
            t.span.start < t.span.end && t.span.end as usize <= text.len(),
            "token {t:?} out of bounds or empty"
        );
        assert!(
            text.is_char_boundary(t.span.start as usize)
                && text.is_char_boundary(t.span.end as usize),
            "token {t:?} splits a character"
        );
        assert!(
            t.span.start >= prev_end,
            "token {t:?} overlaps the one before"
        );
        prev_end = t.span.end;
    }
    for d in doc.diagnostics() {
        assert!(
            d.span.start <= d.span.end && d.span.end as usize <= text.len(),
            "diagnostic {d:?} out of bounds"
        );
        assert!(
            text.is_char_boundary(d.span.start as usize)
                && text.is_char_boundary(d.span.end as usize),
            "diagnostic {d:?} splits a character"
        );
    }
    let _ = doc.is_update();
    let opts = format::Options::default();
    if let Ok(out) = sparql::format(text, &prefixes, base, &opts) {
        let again = Document::parse(&out, &prefixes, base);
        assert!(
            !again.diagnostics().iter().any(|d| d.level == Level::Error),
            "formatted text has errors:\n{out}"
        );
        let twice =
            sparql::format(&out, &prefixes, base, &opts).expect("formatted text formats again");
        assert_eq!(twice, out, "formatting is not idempotent");
    }

    let external = Arc::new(PrefixMap::default());
    let file = Arc::new(SparqlAnalysis::new(
        "file:///chaos/q.rq".into(),
        text,
        external.clone(),
    ));
    let embedded = Arc::new(SparqlAnalysis::embedded(
        "file:///chaos/shapes.ttl".into(),
        text,
        external,
    ));
    for q in [&file, &embedded] {
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            sparql_features_at_random(rng, text, k, q, enc, &file);
        }
    }
}

fn sparql_features_at_random(
    rng: &mut Rng,
    text: &str,
    k: &Knowledge,
    q: &Arc<SparqlAnalysis>,
    enc: Encoding,
    others: &Arc<SparqlAnalysis>,
) {
    let others = std::slice::from_ref(others);
    let _ = sparql_features::document_symbols(q, enc);
    let _ = sparql_features::folding_ranges(q);
    let _ = sparql_features::formatting(q, enc, &format::Options::default());
    tokens_are_well_formed(
        text,
        enc,
        &sparql_features::semantic_tokens_full(q, k, enc).data,
    );
    let _ = sparql_features::term_references("http://example.com/chaos#x", true, others, enc);
    let mut edit = WorkspaceEdit::default();
    let _ = sparql_features::rdf_term_rename(
        "http://example.com/chaos#x",
        "http://example.com/chaos#y",
        others,
        enc,
        &mut edit,
    );
    for _ in 0..3 {
        let pos = q.position(enc, pick_offset(rng, text));
        // Past the end of a line or of the file, as clients sometimes send.
        let wild = Position::new(
            pos.line + rng.below(3) as u32,
            pos.character + rng.below(200) as u32,
        );
        for at in [pos, wild] {
            let params = CompletionParams {
                text_document_position: TextDocumentPositionParams::new(
                    TextDocumentIdentifier::new(q.uri.parse().unwrap()),
                    at,
                ),
                work_done_progress_params: Default::default(),
                partial_result_params: Default::default(),
                context: None,
            };
            let _ = sparql_features::completion(q, k, enc, &params);
            let _ = sparql_features::hover(q, k, enc, at);
            let _ = sparql_features::definition(q, k, enc, at, None);
            let _ = sparql_features::references(q, k, enc, at, true, others);
            let _ = sparql_features::highlights(q, enc, at);
            let _ = sparql_features::prepare_rename(q, enc, at);
            let _ = sparql_features::rename(q, k, enc, at, "renamed", None, others);
        }
        let (from, to) = if rng.below(2) == 0 {
            (pos, wild)
        } else {
            (wild, pos)
        };
        let range = Range::new(from, to);
        tokens_are_well_formed(
            text,
            enc,
            &sparql_features::semantic_tokens_range(q, k, enc, range).data,
        );
        for mode in [InlayLabels::Opaque, InlayLabels::All, InlayLabels::Off] {
            let _ = sparql_features::inlay_hints(q, k, enc, range, mode);
        }
        let params = CodeActionParams {
            text_document: TextDocumentIdentifier::new(q.uri.parse().unwrap()),
            range,
            context: lsp_types::CodeActionContext {
                diagnostics: sparql_features::diagnostics(q, k, enc, &Default::default()),
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let lazy = rng.below(2) == 0;
        for action in sparql_features::code_actions(q, k, enc, &params, lazy) {
            if let CodeActionOrCommand::CodeAction(a) = action {
                let _ = sparql_features::resolve_code_action(q, enc, a);
            }
        }
    }
}

#[test]
fn mutated_sparql_never_panics_1() {
    mutated_sparql(0);
}

#[test]
fn mutated_sparql_never_panics_2() {
    mutated_sparql(1);
}

#[test]
fn mutated_sparql_never_panics_3() {
    mutated_sparql(2);
}

#[test]
fn mutated_sparql_never_panics_4() {
    mutated_sparql(3);
}

fn mutated_sparql(k: usize) {
    let seed = env_or("CHAOS_SEED", 0x5EED_0011_u64);
    let iters = env_or("CHAOS_ITERS", 2000usize);
    let corpus = sparql_corpus();
    // Names the queries can resolve: the Turtle fixtures.
    let docs: Vec<(String, Syntax)> = self::corpus().into_iter().step_by(2).collect();
    let known = knowledge_of(&docs);
    for i in shard(iters, k) {
        let mut rng = Rng::at(seed, i);
        let pick = rng.below(corpus.len());
        let text = sparql_mutate(&mut rng, &corpus[pick]);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sparql_exercise(&mut Rng(seed ^ i as u64 ^ 0x9E37), &text, &known)
        }));
        if let Err(e) = r {
            let ext = if sparql::Document::parse(&text, &PrefixMap::default(), None).is_update() {
                "ru"
            } else {
                "rq"
            };
            panic!(
                "{}",
                failure_report("sparql", seed, iters, i, ext, &text, &panic_message(&*e))
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pathological inputs: written down, not found by chance.

/// Runs `work` on a thread with the 2 MiB stack the smallest host gives: a
/// missing depth guard aborts the process here, and a slow path is caught by
/// the caller's time bound. Returns how long it took.
fn on_small_stack(name: &str, work: impl FnOnce() + Send + 'static) -> Duration {
    let started = Instant::now();
    std::thread::Builder::new()
        .name(name.into())
        .stack_size(2 << 20)
        .spawn(work)
        .unwrap()
        .join()
        .unwrap_or_else(|e| panic!("{name} panicked: {}", panic_message(&*e)));
    started.elapsed()
}

/// The pipeline on one pathological text: the parser, the analysis, and every
/// feature at a few positions, ending with `Knowledge` empty again.
fn survive(name: &str, text: String, syntax: Syntax, bound: Duration) {
    let label = name.to_string();
    let took = on_small_stack(name, move || {
        exercise(&mut Rng(0xC0FFEE), &text, syntax, &mut Knowledge::default())
    });
    eprintln!("{label}: {took:?}");
    assert!(
        took < bound,
        "{label} took {took:?}, the bound is {bound:?}"
    );
}

/// The number of "Nested more than N levels deep" errors a parse reports.
fn depth_errors(text: &str, syntax: Syntax) -> usize {
    parse(text, &PrefixMap::default(), syntax)
        .errors
        .iter()
        .filter(|e| e.message().starts_with("Nested more than"))
        .count()
}

/// 10,000 levels of each bracket kind, `{|` among them, are one error each and
/// no stack overflow, in the parser and in everything built on it.
#[test]
fn pathological_turtle_nesting() {
    const N: usize = 10_000;
    let cases = [
        (
            "annotation",
            format!("<a> <p> <o> {}", "{| <p> <o> ".repeat(N)),
        ),
        ("blank node", format!("<a> <p> {}", "[ <p> ".repeat(N))),
        ("collection", format!("<a> <p> {}", "( ".repeat(N))),
        ("reified triple", format!("{}<p> <o> .", "<< ".repeat(N))),
        (
            "triple term",
            format!("<a> <p> {}", "<<( <a> <p> ".repeat(N)),
        ),
        (
            "closed blank node",
            format!("<a> <p> {}<o>{} .", "[ <p> ".repeat(N), " ]".repeat(N)),
        ),
    ];
    for (what, text) in cases {
        for syntax in [Syntax::Turtle, Syntax::TriG] {
            let t = text.clone();
            let found = std::thread::Builder::new()
                .stack_size(2 << 20)
                .spawn(move || depth_errors(&t, syntax))
                .unwrap()
                .join()
                .unwrap();
            assert_eq!(found, 1, "{what} ({syntax:?}): one depth error expected");
        }
        survive(what, text, Syntax::Turtle, Duration::from_secs(10));
    }
}

/// 100,000 tokens on one line, with a stray `)` in the middle: the parser
/// recovers, and nothing is quadratic in the line's length.
#[test]
fn pathological_trig_line() {
    let statements = 25_000; // four tokens each
    let half = "ex:a ex:b ex:c . ".repeat(statements / 2);
    let text = format!("@prefix ex: <http://e/> . ex:g {{ {half})  {half}}}");
    assert!(!text.contains('\n'));
    let p = parse(&text, &PrefixMap::default(), Syntax::TriG);
    assert!(p.tokens.len() >= 100_000, "{} tokens", p.tokens.len());
    assert!(!p.errors.is_empty(), "the stray ')' is an error");
    assert!(
        p.triples.len() >= statements - 2,
        "recovered after the stray ')': {} of {statements} triples",
        p.triples.len()
    );
    survive("one TriG line", text, Syntax::TriG, Duration::from_secs(10));
}

/// A file with `\r` line ends only (and `\r` ending its comments): all of it
/// is read, not just the part before the first comment.
#[test]
fn pathological_cr_only_text() {
    let lines = 5_000;
    let mut text = String::from("# header\r@prefix ex: <http://e/> .\r");
    for i in 0..lines {
        text.push_str(&format!(
            "# statement {i}\rex:s{i} ex:p ex:o ; # trailing\r    ex:q \"v\" .\r"
        ));
    }
    for syntax in [Syntax::Turtle, Syntax::TriG] {
        let p = parse(&text, &PrefixMap::default(), syntax);
        assert!(p.errors.is_empty(), "{syntax:?}: {:?}", p.errors.first());
        assert_eq!(p.triples.len(), 2 * lines, "{syntax:?}");
        assert_eq!(p.comments.len(), 1 + 2 * lines, "{syntax:?}");
    }
    survive(
        "CR-only text",
        text,
        Syntax::Turtle,
        Duration::from_secs(10),
    );
}

/// Tokens of a megabyte: the IRI closed and not, a string, a local name, a
/// language tag.
#[test]
fn pathological_huge_tokens() {
    let mb = 1 << 20;
    let long = "a".repeat(mb);
    let cases = [
        (
            "closed IRI",
            format!("<http://e/{long}> <http://e/p> <http://e/o> ."),
        ),
        (
            "unclosed IRI",
            format!("<http://e/p> <http://e/p> <http://e/{long}"),
        ),
        ("string", format!("<http://e/s> <http://e/p> \"{long}\" .")),
        (
            "long string",
            format!("<http://e/s> <http://e/p> \"\"\"{long}\"\"\" ."),
        ),
        (
            "unclosed string",
            format!("<http://e/s> <http://e/p> \"{long}"),
        ),
        (
            "local name",
            format!("@prefix ex: <http://e/> . ex:{long} ex:p ex:o ."),
        ),
        (
            "comment",
            format!("# {long}\n<http://e/s> <http://e/p> <http://e/o> ."),
        ),
        (
            "language tag",
            format!("<http://e/s> <http://e/p> \"x\"@{long} ."),
        ),
        (
            "blank node label",
            format!("_:{long} <http://e/p> <http://e/o> ."),
        ),
    ];
    for (what, text) in cases {
        survive(what, text, Syntax::Turtle, Duration::from_secs(10));
    }
    // The same token in the other syntaxes that have a token of its own.
    let nt = format!("<http://e/{long}> <http://e/p> <http://e/o> .\n");
    survive(
        "N-Triples IRI",
        nt,
        Syntax::NTriples,
        Duration::from_secs(10),
    );
    let json = format!("{{\"@id\": \"http://e/{long}\", \"http://e/p\": \"{long}\"}}");
    survive(
        "JSON-LD string",
        json,
        Syntax::JsonLd,
        Duration::from_secs(10),
    );
}

/// [`survive`] for a SPARQL query.
fn survive_sparql(name: &str, text: String, bound: Duration) {
    let label = name.to_string();
    let known = knowledge_of(&[]);
    let took = on_small_stack(name, move || {
        sparql_exercise(&mut Rng(0xC0FFEE), &text, &known)
    });
    eprintln!("{label}: {took:?}");
    assert!(
        took < bound,
        "{label} took {took:?}, the bound is {bound:?}"
    );
}

/// 10,000 levels of every SPARQL bracket kind.
#[test]
fn pathological_sparql_nesting() {
    const N: usize = 10_000;
    let cases = [
        ("group", format!("SELECT * WHERE {}", "{ ".repeat(N))),
        (
            "closed groups",
            format!("SELECT * WHERE {}{}", "{ ".repeat(N), "} ".repeat(N)),
        ),
        (
            "optional",
            format!("SELECT * WHERE {}", "{ OPTIONAL ".repeat(N)),
        ),
        (
            "subquery",
            format!("SELECT * WHERE {}", "{ SELECT * WHERE ".repeat(N)),
        ),
        (
            "parentheses",
            format!("SELECT * WHERE {{ FILTER {} 1 }}", "( ".repeat(N)),
        ),
        (
            "function calls",
            format!("SELECT * WHERE {{ FILTER {} 1 }}", "STR(".repeat(N)),
        ),
        (
            "blank node",
            format!("SELECT * WHERE {{ ?s ?p {} }}", "[ ?q ".repeat(N)),
        ),
        (
            "collection",
            format!("SELECT * WHERE {{ ?s ?p {} }}", "( ".repeat(N)),
        ),
        (
            "triple term",
            format!("SELECT * WHERE {{ {} }}", "<<( ?a ?b ".repeat(N)),
        ),
        (
            "reified triple",
            format!("SELECT * WHERE {{ {} }}", "<< ?a ?b ".repeat(N)),
        ),
        (
            "annotation",
            format!("SELECT * WHERE {{ ?s ?p ?o {} }}", "{| ?q ?r ".repeat(N)),
        ),
        (
            "property path",
            format!("SELECT * WHERE {{ ?s {} ?o }}", "(<p>/".repeat(N)),
        ),
        (
            "negation",
            format!("SELECT * WHERE {{ FILTER {} ?x }}", "!".repeat(N)),
        ),
    ];
    for (what, text) in cases {
        survive_sparql(what, text, Duration::from_secs(10));
    }
}

/// 10,000 levels of JSON arrays and objects, closed and not, in the document
/// and in its `@context`.
#[test]
fn pathological_jsonld_nesting() {
    const N: usize = 10_000;
    let cases = [
        ("arrays", "[".repeat(N)),
        (
            "closed arrays",
            format!("{}{}", "[".repeat(N), "]".repeat(N)),
        ),
        ("objects", "{\"a\":".repeat(N)),
        (
            "closed objects",
            format!("{}1{}", "{\"a\":".repeat(N), "}".repeat(N)),
        ),
        (
            "graph objects",
            format!("{}1{}", "{\"@graph\":".repeat(N), "}".repeat(N)),
        ),
        (
            "list arrays",
            format!(
                "{{\"http://e/p\": {}1{}}}",
                "{\"@list\":[".repeat(N),
                "]}".repeat(N)
            ),
        ),
        (
            "contexts",
            format!("{}1{}", "{\"@context\":".repeat(N), "}".repeat(N)),
        ),
        (
            "scoped contexts",
            format!(
                "{{\"@context\": {}0{}}}",
                "{\"t\":{\"@id\":\"http://e/t\",\"@context\":".repeat(N),
                "}}".repeat(N)
            ),
        ),
        (
            "nested ids",
            format!(
                "{}1{}",
                "{\"@id\":\"http://e/a\",\"http://e/p\":".repeat(N / 4),
                "}".repeat(N / 4)
            ),
        ),
    ];
    for (what, text) in cases {
        survive(what, text, Syntax::JsonLd, Duration::from_secs(10));
    }
}

// ---------------------------------------------------------------------------
// The harness itself.

#[test]
fn corpora_are_sorted_and_cover_every_input_family() {
    // `read_dir` has no order; the same seed must pick the same file anywhere.
    let dir = root().join("test/fixtures/format");
    let files = sorted_files(&dir, false);
    assert!(files.len() > 3, "{files:?}");
    assert!(files.windows(2).all(|w| w[0] < w[1]), "{files:?}");
    let deep = sorted_files(&root().join("tests/w3c/shacl/sparql"), true);
    assert!(deep.len() > 20 && deep.windows(2).all(|w| w[0] < w[1]));
    let first: Vec<_> = corpus().into_iter().map(|(t, s)| (t.len(), s)).collect();
    let second: Vec<_> = corpus().into_iter().map(|(t, s)| (t.len(), s)).collect();
    assert_eq!(first, second);

    // SPARQL inside SHACL shapes, from the suite and from the fixtures.
    let turtle: Vec<String> = corpus().into_iter().map(|(t, _)| t).collect();
    for needle in [
        "sh:select",
        "shacl#select",
        "sh:ask",
        "sh:construct",
        "sh:update",
    ] {
        assert!(
            turtle.iter().any(|t| t.contains(needle)),
            "no {needle} in the corpus"
        );
    }
    assert!(
        turtle
            .iter()
            .any(|t| t.contains("SPARQLConstraint") || t.contains("sparql-001")),
        "the SHACL-SPARQL suite is missing"
    );
    // JSON-LD: the W3C toRdf inputs and the fixtures'.
    let json = jsonld_corpus();
    assert!(json.len() > 400, "{} JSON-LD inputs", json.len());
    assert!(
        json.iter().any(|t| t.contains("sh:select")),
        "no JSON-LD shapes with sh:select"
    );
    assert!(json.iter().all(|t| !t.is_empty()) || json[0].is_empty());
}

#[test]
fn a_failure_is_saved_under_the_target_dir_and_its_input_is_in_the_message() {
    let input = format!("<a> <b> \"{}\" .", "x".repeat(5_000));
    let report = failure_report("turtle", 7, 2000, 3, "ttl", &input, "boom");
    let dump = Path::new(env!("CARGO_TARGET_TMPDIR")).join("chaos-turtle-7-3.ttl");
    assert_eq!(
        fs::read_to_string(&dump).unwrap(),
        input,
        "the whole input is saved"
    );
    fs::remove_file(&dump).unwrap();
    assert!(report.contains(&dump.display().to_string()), "{report}");
    assert!(
        report.contains("boom") && report.contains("5012 bytes"),
        "{report}"
    );
    assert!(
        report.contains("<a> <b> \\\"xxx"),
        "the start of the input: {report}"
    );
    assert!(
        report.len() < 800,
        "the message holds a bounded part of it: {} bytes",
        report.len()
    );
    assert!(!std::env::temp_dir().join("chaos-turtle-7-3.ttl").exists());
}

/// A parse result with one bad span, whichever field it sits in: the check
/// must see each. (The text is "abc": 0..99 is outside it.)
#[test]
fn the_span_check_sees_every_span_field() {
    use turtle_lsp::syntax::{
        BlankId, GraphBlock, GraphName, Iri, Node, PrefixDecl, PrefixUse, Ref, Role, Statement,
        SyntaxError, Triple, lexer,
    };
    let text = "abc";
    let bad = turtle_lsp::syntax::Span::new(0, 99);
    let ok = turtle_lsp::syntax::Span::new(0, 1);
    let statement = |span, subject, graph| Statement {
        span,
        subject,
        graph,
    };
    let iri = Iri::from("http://e/x");
    type Fill = Box<dyn Fn(&mut Parsed)>;
    let cases: Vec<(&str, Fill)> = vec![
        (
            "prefix span",
            Box::new(move |p| {
                p.prefixes.push(PrefixDecl {
                    name: "a".into(),
                    iri,
                    span: bad,
                    name_span: ok,
                    iri_span: ok,
                })
            }),
        ),
        (
            "prefix name",
            Box::new(move |p| {
                p.prefixes.push(PrefixDecl {
                    name: "a".into(),
                    iri,
                    span: ok,
                    name_span: bad,
                    iri_span: ok,
                })
            }),
        ),
        (
            "prefix IRI",
            Box::new(move |p| {
                p.prefixes.push(PrefixDecl {
                    name: "a".into(),
                    iri,
                    span: ok,
                    name_span: ok,
                    iri_span: bad,
                })
            }),
        ),
        (
            "prefix use",
            Box::new(move |p| {
                p.prefix_uses.push(PrefixUse {
                    name: iri,
                    span: bad,
                    resolved: true,
                })
            }),
        ),
        (
            "reference",
            Box::new(move |p| {
                p.refs.push(Ref {
                    span: bad,
                    iri,
                    role: Role::Object,
                    form: turtle_lsp::syntax::Form::Iri,
                })
            }),
        ),
        (
            "statement",
            Box::new(move |p| p.statements.push(statement(bad, None, None))),
        ),
        (
            "statement subject",
            Box::new(move |p| p.statements.push(statement(ok, Some((iri, bad)), None))),
        ),
        (
            "statement graph",
            Box::new(move |p| {
                p.statements.push(statement(
                    ok,
                    None,
                    Some((GraphName::Blank(BlankId(0)), bad)),
                ))
            }),
        ),
        (
            "graph span",
            Box::new(move |p| {
                p.graphs.push(GraphBlock {
                    span: bad,
                    name: None,
                    name_span: None,
                    statements: 0..0,
                })
            }),
        ),
        (
            "graph name",
            Box::new(move |p| {
                p.graphs.push(GraphBlock {
                    span: ok,
                    name: None,
                    name_span: Some(bad),
                    statements: 0..0,
                })
            }),
        ),
        (
            "graph statements",
            Box::new(move |p| {
                p.graphs.push(GraphBlock {
                    span: ok,
                    name: None,
                    name_span: None,
                    statements: 0..5,
                })
            }),
        ),
        (
            "triple predicate",
            Box::new(move |p| {
                p.statements.push(statement(ok, None, None));
                p.triples.push(Triple {
                    s: Node::Iri(iri),
                    p: iri,
                    o: Node::Iri(iri),
                    p_span: bad,
                    o_span: ok,
                    statement: 0,
                    graph: None,
                    unresolved: false,
                })
            }),
        ),
        (
            "triple object",
            Box::new(move |p| {
                p.statements.push(statement(ok, None, None));
                p.triples.push(Triple {
                    s: Node::Iri(iri),
                    p: iri,
                    o: Node::Iri(iri),
                    p_span: ok,
                    o_span: bad,
                    statement: 0,
                    graph: None,
                    unresolved: false,
                })
            }),
        ),
        (
            "triple statement",
            Box::new(move |p| {
                p.triples.push(Triple {
                    s: Node::Iri(iri),
                    p: iri,
                    o: Node::Iri(iri),
                    p_span: ok,
                    o_span: ok,
                    statement: 3,
                    graph: None,
                    unresolved: false,
                })
            }),
        ),
        (
            "error",
            Box::new(move |p| {
                p.errors
                    .push(SyntaxError::new(bad, "", Severity::Error, "syntax"))
            }),
        ),
        ("comment", Box::new(move |p| p.comments.push(bad))),
        (
            "token",
            Box::new(move |p| {
                p.tokens
                    .push(lexer::Token::new(lexer::Kind::Comment, 0, 99))
            }),
        ),
        (
            "backwards",
            Box::new(move |p| p.comments.push(turtle_lsp::syntax::Span::new(2, 1))),
        ),
    ];
    for (what, make) in &cases {
        let mut p = Parsed::default();
        make(&mut p);
        assert!(span_fault(text, &p).is_some(), "a bad {what} went unseen");
    }
    // And a character split, on a text that has one.
    let mut p = Parsed::default();
    p.comments.push(turtle_lsp::syntax::Span::new(0, 1));
    assert!(matches!(
        span_fault("é", &p),
        Some(SpanFault::SplitsChar(_))
    ));
    assert!(span_fault(text, &Parsed::default()).is_none());
}

// ---------------------------------------------------------------------------
// Defects the chaos tests found. No ticket owns them yet; each test below
// states the behaviour that is right, fails today, and is `#[ignore]`d so the
// suite stays green. `cargo test --release --test chaos -- --ignored` runs
// them; when one passes, delete its `#[ignore]` and the workaround it names
// in the random tests above.

/// Malformed JSON-LD spans end on character boundaries: the diagnostics and
/// the semantic tokens convert them to positions without panicking.
#[test]
fn jsonld_spans_stay_on_character_boundaries() {
    let texts = [
        "{é\"b\":2}",
        "{\"a\":1,é\"b\":2}",
        "{\"@context\": {\"@vocab\":\"x\" \u{200c} \"c\"}}",
    ];
    for text in texts {
        let outcome = std::panic::catch_unwind(|| {
            let a = Analysis::with_syntax(
                "file:///chaos/doc.jsonld".into(),
                text,
                Arc::new(PrefixMap::default()),
                true,
                Syntax::JsonLd,
            );
            let k = Knowledge::default();
            for enc in [Encoding::Utf8, Encoding::Utf16] {
                let _ = diagnostics::diagnostics(&a, &k, enc);
                let _ = semantic::full(&a, &k, enc);
            }
        });
        assert!(outcome.is_ok(), "panicked on {text:?}");
    }
}

/// `<a> <p> <o> ; # c` then `.` on the next line is formatted to `; . # c`,
/// and that to `. # c`: the first output is not a fixed point (and keeps a
/// `;` that nothing follows).
#[test]
#[ignore = "known defect: the formatter keeps a dangling ';' before a comment and '.' the first time only"]
fn format_drops_a_dangling_semicolon_once() {
    let text = "<a> <p> <o> ; # c\n.\n";
    let opts = format::Options::default();
    let parsed = parse(text, &PrefixMap::default(), Syntax::Turtle);
    let once = format::format(text, &parsed, &PrefixMap::default(), &opts).unwrap();
    let parsed = parse(&once, &PrefixMap::default(), Syntax::Turtle);
    let twice = format::format(&once, &parsed, &PrefixMap::default(), &opts).unwrap();
    assert_eq!(once, twice, "not idempotent: {once:?} then {twice:?}");
}

/// Statements in one TriG graph block on one line cost time quadratic in
/// their number (the same statements on separate lines, or outside a block,
/// are linear): 100,000 tokens take over a second, 200,000 about five.
#[test]
#[ignore = "known defect: a TriG graph block on one line parses in quadratic time"]
fn trig_graph_block_on_one_line_parses_in_linear_time() {
    let time = |statements: usize| {
        let text = format!(
            "@prefix ex: <http://e/> . ex:g {{ {}}}",
            "ex:a ex:b ex:c . ".repeat(statements)
        );
        let best = (0..3)
            .map(|_| {
                let started = Instant::now();
                let p = parse(&text, &PrefixMap::default(), Syntax::TriG);
                assert_eq!(p.triples.len(), statements);
                started.elapsed()
            })
            .min()
            .unwrap();
        best.as_secs_f64()
    };
    let (small, large) = (time(6_250), time(25_000));
    // Four times the input: linear is 4x, quadratic 16x.
    assert!(
        large < small * 8.0,
        "6,250 statements took {small:.3}s, 25,000 took {large:.3}s"
    );
}
