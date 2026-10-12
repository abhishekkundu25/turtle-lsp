//! Randomized robustness test ("fuzzing" on stable Rust).
//!
//! Takes real Turtle files, mutates them thousands of times (cuts, unbalanced
//! brackets, stray quotes, multi-byte characters, CRLF, truncation) and runs
//! the parser and every editor feature on each result at random positions in
//! both position encodings. Any panic fails the test. Deterministic: the seed
//! is fixed, and CHAOS_SEED / CHAOS_ITERS override it.

use lsp_types::Position;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use turtle_lsp::analysis::{Analysis, Encoding};
use turtle_lsp::features::{completion, diagnostics, hover, navigation, symbols};
use turtle_lsp::syntax::{PrefixMap, parse};

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
];

fn corpus() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("test/fixtures");
    let mut out = vec![String::new(), "@prefix".into(), "ex:a ex:b ex:c".into()];
    for dir in [
        "workspace",
        "project",
        "project/core",
        "project/data",
        "project/shapes",
    ] {
        if let Ok(entries) = fs::read_dir(root.join(dir)) {
            for e in entries.flatten() {
                if e.path().extension().is_some_and(|x| x == "ttl") {
                    out.push(fs::read_to_string(e.path()).unwrap());
                }
            }
        }
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

fn exercise(rng: &mut Rng, text: &str, all: &[Arc<Analysis>]) {
    // The parser alone: every span must be inside the text and on char boundaries.
    let p = parse(text, &PrefixMap::default());
    for span in p
        .refs
        .iter()
        .map(|r| r.span)
        .chain(p.errors.iter().map(|e| e.span))
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

    let a = Arc::new(Analysis::new(
        "file:///chaos/doc.ttl".into(),
        text,
        Arc::new(PrefixMap::default()),
        true,
    ));
    let mut docs = all.to_vec();
    docs.push(a.clone());
    for enc in [Encoding::Utf8, Encoding::Utf16] {
        let _ = diagnostics::diagnostics(&a, enc);
        let _ = symbols::document_symbols(&a, text, enc);
        let _ = symbols::folding_ranges(&a);
        let _ = symbols::workspace_symbols(&docs, enc, "a");
        for _ in 0..4 {
            let off = char_boundary(text, rng.below(text.len() + 1)) as u32;
            let pos: Position = a.position(enc, off);
            let _ = completion::complete(&a, text, &docs, enc, pos);
            let _ = hover::hover(&a, &docs, enc, pos);
            let _ = navigation::definition(&a, &docs, enc, pos, None);
            let _ = navigation::type_definition(&a, &docs, enc, pos);
            let _ = navigation::implementations(&a, &docs, enc, pos);
            let _ = navigation::references(&a, &docs, enc, pos, true);
            let _ = navigation::highlights(&a, enc, pos);
            let _ = navigation::prepare_rename(&a, enc, pos);
            let _ = navigation::rename(&a, &docs, enc, pos, "renamed");
            // Positions past the end of a line or of the file, as clients sometimes send.
            let wild = Position::new(
                pos.line + rng.below(3) as u32,
                pos.character + rng.below(200) as u32,
            );
            let _ = completion::complete(&a, text, &docs, enc, wild);
            let _ = hover::hover(&a, &docs, enc, wild);
        }
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
    let base: Vec<Arc<Analysis>> = corpus
        .iter()
        .enumerate()
        .map(|(i, t)| {
            Arc::new(Analysis::new(
                format!("file:///chaos/{i}.ttl"),
                t,
                Arc::new(PrefixMap::default()),
                true,
            ))
        })
        .collect();
    let mut rng = Rng(seed);
    for i in 0..iters {
        let pick = rng.below(corpus.len());
        let text = mutate(&mut rng, &corpus[pick]);
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            exercise(&mut Rng(seed ^ i as u64 ^ 0x9E37), &text, &base)
        }));
        if r.is_err() {
            let dump = std::env::temp_dir().join(format!("turtle-chaos-{seed}-{i}.ttl"));
            fs::write(&dump, &text).unwrap();
            panic!(
                "iteration {i} (seed {seed}) panicked; input saved to {}",
                dump.display()
            );
        }
    }
}
