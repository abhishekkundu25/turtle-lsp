//! Randomized robustness test ("fuzzing" on stable Rust).
//!
//! Takes real Turtle, TriG and N-Triples files, mutates them thousands of times (cuts, unbalanced
//! brackets, stray quotes, multi-byte characters, CRLF, truncation) and runs
//! the parser and every editor feature on each result at random positions in
//! both position encodings. Any panic fails the test. Deterministic: the seed
//! is fixed, and CHAOS_SEED / CHAOS_ITERS override it.

use lsp_types::{CodeActionOrCommand, CodeActionParams, Position, Range};
use std::fs;
use std::path::Path;
use std::sync::Arc;
use turtle_lsp::analysis::{Analysis, Encoding};
use turtle_lsp::features::{completion, diagnostics, format, hover, navigation, refactor, symbols};
use turtle_lsp::knowledge::{Knowledge, Source};
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
        let _ = symbols::document_symbols(&a, text, enc);
        let _ = symbols::folding_ranges(&a);
        let _ = symbols::workspace_symbols(docs, enc, "a");
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
            let _ = navigation::rename(&a, docs, enc, pos, "renamed");
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
    k.remove(doc_uri(syntax));
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
