//! Randomized robustness test ("fuzzing" on stable Rust).
//!
//! Takes real Turtle, TriG, N-Triples and N-Quads files (and, in a second
//! test, SPARQL queries and updates), mutates them thousands of times (cuts, unbalanced
//! brackets, stray quotes, multi-byte characters, CRLF, truncation) and runs
//! the parser and every editor feature on each result at random positions in
//! both position encodings. Any panic fails the test. Deterministic: the seed
//! is fixed, and CHAOS_SEED / CHAOS_ITERS override it.

use lsp_types::{CodeActionOrCommand, CodeActionParams, Position, Range};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use turtle_lsp::analysis::{Analysis, Encoding};
use turtle_lsp::features::hierarchy;
use turtle_lsp::features::{
    completion, diagnostics, format, hover, inlay, navigation, refactor, semantic, symbols,
};
use turtle_lsp::imports;
use turtle_lsp::knowledge::{Knowledge, Source};
use turtle_lsp::settings::InlayLabels;
use turtle_lsp::sparql::{self, Document, Level};
use turtle_lsp::syntax::{PrefixMap, Severity, Syntax, parse};

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
];

fn corpus() -> Vec<(String, Syntax)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
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
    ] {
        if let Ok(entries) = fs::read_dir(root.join(dir)) {
            for e in entries.flatten() {
                if let Some(syntax) = Syntax::from_extension(&e.path().to_string_lossy()) {
                    out.push((fs::read_to_string(e.path()).unwrap(), syntax));
                }
            }
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

fn char_boundary(s: &str, mut i: usize) -> usize {
    i = i.min(s.len());
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn mutate(rng: &mut Rng, text: &str) -> String {
    let mut s = text.to_string();
    for _ in 0..1 + rng.below(4) {
        let at = char_boundary(&s, rng.below(s.len() + 1));
        match rng.below(5) {
            0 => s.insert_str(at, SNIPPETS[rng.below(SNIPPETS.len())]),
            1 => {
                let end = char_boundary(&s, at + rng.below(40));
                s.replace_range(at..end, "");
            }
            2 => {
                let end = char_boundary(&s, at + rng.below(60));
                let chunk = s[at..end].to_string();
                let to = char_boundary(&s, rng.below(s.len() + 1));
                s.insert_str(to, &chunk);
            }
            3 => s.truncate(at),
            _ => {
                let n = rng.below(6);
                for _ in 0..n {
                    let at = char_boundary(&s, rng.below(s.len() + 1));
                    s.insert_str(at, SNIPPETS[rng.below(SNIPPETS.len())]);
                }
            }
        }
    }
    s
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

fn exercise(rng: &mut Rng, text: &str, syntax: Syntax, k: &mut Knowledge) {
    // The parser alone: every span must be inside the text and on char boundaries.
    let p = parse(text, &PrefixMap::default(), syntax);
    for span in p
        .refs
        .iter()
        .map(|r| r.span)
        .chain(p.errors.iter().map(|e| e.span))
        .chain(p.statements.iter().map(|s| s.span))
        .chain(p.graphs.iter().map(|g| g.span))
    {
        assert!(
            span.start <= span.end && span.end as usize <= text.len(),
            "span {span:?} out of bounds"
        );
        assert!(
            text.is_char_boundary(span.start as usize) && text.is_char_boundary(span.end as usize),
            "span {span:?} splits a character"
        );
    }

    for g in &p.graphs {
        assert!(g.statements.start <= g.statements.end);
        assert!(g.statements.end as usize <= p.statements.len());
    }

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
    // The document joins the index (and leaves it again below), as an edited
    // open document does.
    k.set(doc_uri(syntax), Source::Open, a.clone());
    let docs = &*k;
    // Formatting: refused on errors, else a fixed point.
    let opts = format::Options::default();
    let formatted = format::format(text, &a.parsed, &a.external, &opts);
    if a.parsed
        .errors
        .iter()
        .all(|e| e.severity != Severity::Error)
    {
        let out = formatted.expect("formatter refused a document without syntax errors");
        let again = format::format(
            &out,
            &parse(&out, &PrefixMap::default(), syntax),
            &a.external,
            &opts,
        );
        assert_eq!(
            again.as_deref(),
            Some(out.as_str()),
            "formatting is not idempotent"
        );
    } else {
        assert!(
            formatted.is_none(),
            "formatted a document with syntax errors"
        );
    }
    for enc in [Encoding::Utf8, Encoding::Utf16] {
        let _ = format::formatting(&a, enc, &opts);
        let _ = diagnostics::diagnostics(&a, docs, enc);
        let lint_off = diagnostics::Options {
            lint_undescribed_terms: false,
        };
        let _ = diagnostics::diagnostics_with(&a, docs, enc, &lint_off);
        let unresolved = imports::Status::Unresolved("x".into());
        for d in imports::diagnostics_with(&a, enc, |_| Some(&unresolved)) {
            assert!(d.range.start <= d.range.end, "bad import diagnostic range");
        }
        let _ = symbols::document_symbols(&a, text, enc);
        let _ = symbols::folding_ranges(&a);
        let _ = symbols::workspace_symbols(docs, enc, "a");
        tokens_are_well_formed(text, enc, &semantic::full(&a, docs, enc).data);
        for _ in 0..4 {
            let off = char_boundary(text, rng.below(text.len() + 1)) as u32;
            let pos: Position = a.position(enc, off);
            let _ = completion::complete(&a, text, docs, enc, pos);
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
            let _ = completion::complete(&a, text, docs, enc, wild);
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
        docs: docs.docs().map(|(_, s, a)| (s, a.clone())).collect(),
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
    };
    let _ = turtle_lsp::features::shacl_validate::run(&job, &|| false);
    k.remove(doc_uri(syntax));
}

/// Semantic tokens: sorted, non-empty, each inside one line, never overlapping.
fn tokens_are_well_formed(text: &str, enc: Encoding, data: &[lsp_types::SemanticToken]) {
    let lines: Vec<&str> = text.split('\n').collect();
    let width = |l: &str| match enc {
        Encoding::Utf8 => l.len() as u32,
        Encoding::Utf16 => l.encode_utf16().count() as u32,
    };
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
        let l = lines.get(line as usize).expect("token past the last line");
        assert!(col + t.length <= width(l), "token {i} runs past its line");
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
    let _ = diagnostics::code_actions(
        a,
        k,
        text,
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
        let mut v: Vec<String> = parse(t, &PrefixMap::default(), syntax)
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
        let reparsed = parse(&after, &PrefixMap::default(), syntax);
        assert!(
            reparsed.errors.is_empty() && triples(&after) == before,
            "'{}' changed the graph:\n{after}",
            c.title
        );
    }
}

#[test]
fn mutated_turtle_never_panics() {
    let seed = std::env::var("CHAOS_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5EED_0007_u64);
    let iters = std::env::var("CHAOS_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000usize);
    let corpus = corpus();
    let mut base = Knowledge::default();
    for (i, (t, syntax)) in corpus.iter().enumerate() {
        let uri = format!("file:///chaos/{i}.ttl");
        let a = Analysis::with_syntax(
            uri.clone(),
            t,
            Arc::new(PrefixMap::default()),
            true,
            *syntax,
        );
        base.set(&uri, Source::Disk, Arc::new(a));
    }
    let mut rng = Rng(seed);
    for i in 0..iters {
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
        if r.is_err() {
            let dump = std::env::temp_dir().join(format!("turtle-chaos-{seed}-{i}.ttl"));
            fs::write(&dump, &text).unwrap();
            panic!(
                "iteration {i} (seed {seed}, {syntax:?}) panicked; input saved to {}",
                dump.display()
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
    let mut s = text.to_string();
    for _ in 0..1 + rng.below(4) {
        let at = char_boundary(&s, rng.below(s.len() + 1));
        match rng.below(4) {
            0 => s.insert_str(at, SPARQL_SNIPPETS[rng.below(SPARQL_SNIPPETS.len())]),
            1 => {
                let end = char_boundary(&s, at + rng.below(40));
                s.replace_range(at..end, "");
            }
            2 => {
                let end = char_boundary(&s, at + rng.below(60));
                let chunk = s[at..end].to_string();
                let to = char_boundary(&s, rng.below(s.len() + 1));
                s.insert_str(to, &chunk);
            }
            _ => s.truncate(at),
        }
    }
    s
}

/// Tokens and diagnostics stay inside the text and on character boundaries,
/// tokens never overlap, and formatted text is valid and formats to itself.
fn sparql_exercise(text: &str) {
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
}

#[test]
fn mutated_sparql_never_panics() {
    let seed = std::env::var("CHAOS_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0x5EED_0011_u64);
    let iters = std::env::var("CHAOS_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(3000usize);
    let corpus = sparql_corpus();
    let mut rng = Rng(seed);
    for i in 0..iters {
        let pick = rng.below(corpus.len());
        let text = sparql_mutate(&mut rng, &corpus[pick]);
        if std::panic::catch_unwind(|| sparql_exercise(&text)).is_err() {
            // Under target/, not the system temp directory.
            let dump =
                Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("chaos-sparql-{seed}-{i}.rq"));
            fs::write(&dump, &text).unwrap();
            panic!(
                "iteration {i} (seed {seed}) panicked; input saved to {}",
                dump.display()
            );
        }
    }
}
