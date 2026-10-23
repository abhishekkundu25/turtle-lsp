//! Randomized robustness test ("fuzzing" on stable Rust).
//!
//! Takes real Turtle, TriG, N-Triples and N-Quads files (SHACL shapes with
//! embedded SPARQL among them), JSON-LD documents and SPARQL queries and
//! updates, mutates them thousands of times (cuts, unbalanced brackets, stray
//! quotes, multi-byte characters, CRLF and lone CR, truncation, repeated
//! brackets, huge tokens) and runs the parser and every editor feature on each
//! result at random positions in both position encodings. Any panic, and any
//! span outside the text, fails the test. Deterministic: the corpus is read in
//! sorted order, the seed is fixed, and CHAOS_SEED / CHAOS_ITERS override it;
//! CHAOS_ONLY=<i> runs iteration i alone.
//!
//! A failure's input is saved under `CARGO_TARGET_TMPDIR` (CI discards the
//! system temp directory) and its start is in the panic message. An iteration
//! that runs past its budget is reported the same way by a watchdog, which
//! then exits the process.
//!
//! The mutations rarely build a 10,000-level nest or a megabyte token, so
//! `pathological_inputs_are_survived` writes those down, each on a 2 MiB
//! thread, the smallest stack a host gives its threads.

use lsp_types::{
    CodeActionOrCommand, CodeActionParams, CompletionItem, CompletionParams, CompletionResponse,
    Position, Range, TextDocumentIdentifier, TextDocumentPositionParams, TextEdit, WorkspaceEdit,
};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use turtle_lsp::analysis::{Analysis, Encoding, SparqlAnalysis};
use turtle_lsp::batch;
use turtle_lsp::features::hierarchy;
use turtle_lsp::features::{
    completion, diagnostics, format, hover, inlay, navigation, quickfix, refactor, semantic,
    symbols,
};
use turtle_lsp::imports;
use turtle_lsp::jsonld;
use turtle_lsp::knowledge::{Knowledge, Source};
use turtle_lsp::settings::{InlayLabels, Settings};
use turtle_lsp::sparql::{self, Document, Level, TokenKind, features as sparql_features};
use turtle_lsp::syntax::lexer::Kind;
use turtle_lsp::syntax::{
    Form, GraphName, Iri, Node, Parsed, PrefixMap, Severity, Syntax, parse, parse_with_base,
};

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

/// The iterations shard `k` runs: its range, or only `CHAOS_ONLY` when that
/// is set (nothing when another shard owns it).
fn iterations(iters: usize, k: usize) -> std::ops::Range<usize> {
    let all = shard(iters, k);
    match std::env::var("CHAOS_ONLY")
        .ok()
        .and_then(|s| s.parse().ok())
    {
        Some(i) if all.contains(&i) => i..i + 1,
        Some(_) => 0..0,
        None => all,
    }
}

/// How long one iteration may run before the watchdog fails the run;
/// `CHAOS_ITERATION_BUDGET_MS` overrides it.
const ITERATION_BUDGET: Duration = Duration::from_secs(10);

/// The iteration a shard is running: its number, start, input and the
/// extension its input is saved under.
type Running = Option<(usize, Instant, Arc<str>, &'static str)>;

/// A shard's watchdog: a thread that wakes every second and, when the
/// iteration in the slot has run past the budget, reports it and exits the
/// process (a hang would otherwise end in a timeout that names no input).
/// Dropped, it prints the shard's five slowest iterations.
struct Watchdog {
    slot: Arc<Mutex<Running>>,
    slowest: Vec<(Duration, usize)>,
    kind: &'static str,
}

impl Watchdog {
    fn new(kind: &'static str, seed: u64, iters: usize) -> Watchdog {
        let budget = Duration::from_millis(env_or(
            "CHAOS_ITERATION_BUDGET_MS",
            ITERATION_BUDGET.as_millis() as u64,
        ));
        let slot: Arc<Mutex<Running>> = Arc::default();
        let weak = Arc::downgrade(&slot);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let Some(slot) = weak.upgrade() else { return };
                let running = slot.lock().unwrap();
                let Some((i, started, text, ext)) = &*running else {
                    continue;
                };
                if started.elapsed() > budget {
                    let why = format!(
                        "still running after {:?} (budget {budget:?})",
                        started.elapsed()
                    );
                    let report = failure_report(kind, seed, iters, *i, ext, text, &why);
                    // libtest captures `eprintln!` from this thread too, and the exit drops it.
                    let _ = std::io::stderr()
                        .lock()
                        .write_all(format!("{report}\n").as_bytes());
                    std::process::exit(101);
                }
            }
        });
        Watchdog {
            slot,
            slowest: Vec::new(),
            kind,
        }
    }

    /// Iteration `i` starts on `text`.
    fn start(&mut self, i: usize, text: &str, ext: &'static str) {
        let next = Some((i, Instant::now(), text.into(), ext));
        let last = std::mem::replace(&mut *self.slot.lock().unwrap(), next);
        self.time(last);
    }

    fn time(&mut self, last: Running) {
        if let Some((i, started, _, _)) = last {
            self.slowest.push((started.elapsed(), i));
            self.slowest.sort_by(|a, b| b.cmp(a));
            self.slowest.truncate(5);
        }
    }
}

impl Drop for Watchdog {
    fn drop(&mut self) {
        let last = self.slot.lock().unwrap().take();
        self.time(last);
        let slowest: Vec<String> = self
            .slowest
            .iter()
            .map(|(took, i)| format!("{i} ({took:.2?})"))
            .collect();
        // Past libtest's capture, so a passing run's log has them too.
        let line = format!("{} slowest iterations: {}\n", self.kind, slowest.join(", "));
        let _ = std::io::stderr().lock().write_all(line.as_bytes());
    }
}

/// The first line where `before` and `after` differ, with two lines of
/// context on each side, quoted so a `\r` shows.
fn first_difference(before: &str, after: &str) -> String {
    let (b, a): (Vec<&str>, Vec<&str>) =
        (before.split('\n').collect(), after.split('\n').collect());
    let n = b.iter().zip(&a).take_while(|(x, y)| x == y).count();
    let from = n.saturating_sub(2);
    let mut out = format!("first difference at line {}:", n + 1);
    for (sign, lines) in [("-", &b), ("+", &a)] {
        for l in lines.iter().skip(from).take(n - from + 3) {
            let l: String = l.chars().take(200).collect();
            out += &format!("\n  {sign} {l:?}");
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Token checks: what an edit may do to the tokens of a text.

/// A token: its kind (the lexer's enum as a number) and its byte range.
type Tok = (u8, usize, usize);

/// The lexer a token check reads a text with. TriG is Turtle's, with
/// graph blocks.
#[derive(Clone, Copy, PartialEq)]
enum Lexer {
    Turtle,
    TriG,
    Sparql,
    Json,
}

impl Lexer {
    fn of(syntax: Syntax) -> Lexer {
        match syntax {
            Syntax::JsonLd => Lexer::Json,
            Syntax::TriG => Lexer::TriG,
            _ => Lexer::Turtle,
        }
    }

    fn tokens(self, text: &str) -> Vec<Tok> {
        match self {
            Lexer::Turtle | Lexer::TriG => turtle_lsp::syntax::lexer::tokenize(text)
                .iter()
                .map(|t| (t.kind as u8, t.start as usize, t.end as usize))
                .collect(),
            Lexer::Sparql => sparql::lexer::tokenize(text)
                .iter()
                .map(|t| (t.kind as u8, t.span.start as usize, t.span.end as usize))
                .collect(),
            Lexer::Json => jsonld::json::tokens(text)
                .iter()
                .map(|t| (t.kind as u8, t.span.start as usize, t.span.end as usize))
                .collect(),
        }
    }

    fn comment(self, kind: u8) -> bool {
        kind == match self {
            Lexer::Turtle | Lexer::TriG => Kind::Comment as u8,
            Lexer::Sparql => TokenKind::Comment as u8,
            Lexer::Json => jsonld::json::TokenKind::Comment as u8,
        }
    }

    fn string(self, kind: u8) -> bool {
        kind == match self {
            Lexer::Turtle | Lexer::TriG => Kind::String as u8,
            Lexer::Sparql => TokenKind::String as u8,
            Lexer::Json => jsonld::json::TokenKind::String as u8,
        }
    }

    /// An IRI or a prefixed name.
    fn term(self, kind: u8) -> bool {
        match self {
            Lexer::Turtle | Lexer::TriG => kind == Kind::Iri as u8 || kind == Kind::PName as u8,
            Lexer::Sparql => kind == TokenKind::Iri as u8 || kind == TokenKind::PName as u8,
            Lexer::Json => false,
        }
    }

    /// Whether `t` starts a directive: `@prefix`, `PREFIX`, `@base`, `BASE`,
    /// `@version`, `VERSION`.
    fn directive(self, text: &str, t: Tok) -> bool {
        match self {
            Lexer::Turtle | Lexer::TriG => [
                Kind::PrefixDirective,
                Kind::BaseDirective,
                Kind::VersionDirective,
                Kind::SparqlPrefix,
                Kind::SparqlBase,
                Kind::SparqlVersion,
            ]
            .iter()
            .any(|k| *k as u8 == t.0),
            Lexer::Sparql => {
                t.0 == TokenKind::Keyword as u8
                    && ["PREFIX", "BASE", "VERSION"]
                        .iter()
                        .any(|k| text[t.1..t.2].eq_ignore_ascii_case(k))
            }
            Lexer::Json => false,
        }
    }
}

/// A text and its tokens: what the edits to it are checked against.
struct Lexed<'t> {
    text: &'t str,
    lex: Lexer,
    toks: Vec<Tok>,
    /// The spans of its TriG graph blocks, as the parser reads them.
    graphs: Vec<(usize, usize)>,
    /// What [`Lexed::splice`] said of each edit set: completions offer the
    /// same imports at every position, and a SPARQL check lexes the whole text.
    spliced: RefCell<HashMap<Spliced, Result<String, String>>>,
}

/// An edit set as [`Lexed::splice`] takes it, and its `keep`.
type Spliced = (Vec<(usize, usize, String)>, bool);

impl<'t> Lexed<'t> {
    fn new(text: &'t str, lex: Lexer) -> Lexed<'t> {
        Lexed {
            text,
            lex,
            toks: lex.tokens(text),
            graphs: Vec::new(),
            spliced: RefCell::default(),
        }
    }

    /// Whether `toks[j]` ends a document-level directive or statement: a
    /// `.`, a TriG graph block's `}` or the last token of a `PREFIX`, `BASE`
    /// or `VERSION` line (or of an `@prefix` line without its `.`, which the
    /// parser takes as one) outside every graph block, or a SPARQL update's
    /// `;` outside every group. (A SPARQL `PREFIX` line inside braces is
    /// misplaced, but what follows it is still the end of the prologue.)
    fn ends_statement(&self, j: usize) -> bool {
        let (text, lex) = (self.text, self.lex);
        let word = |t: &Tok| &text[t.1..t.2];
        let mut back = self.toks[..=j].iter().rev().filter(|t| !lex.comment(t.0));
        let (t, b1, b2) = (self.toks[j], back.nth(1), back.next());
        let is = |b: Option<&Tok>, keyword: &str| {
            b.is_some_and(|b| {
                lex.directive(text, *b)
                    && word(b)
                        .trim_start_matches('@')
                        .eq_ignore_ascii_case(keyword)
            })
        };
        let directive = is(b2, "PREFIX") || is(b1, "BASE") || is(b1, "VERSION");
        match lex {
            Lexer::Turtle => directive || word(&t) == ".",
            Lexer::TriG => {
                let in_graph = self.graphs.iter().any(|g| g.0 <= t.1 && t.2 < g.1);
                !in_graph && (directive || matches!(word(&t), "." | "}"))
            }
            Lexer::Sparql => {
                let depth = self.toks[..=j].iter().fold(0usize, |d, t| match word(t) {
                    "{" => d + 1,
                    "}" => d.saturating_sub(1),
                    _ => d,
                });
                directive || depth == 0 && word(&t) == ";"
            }
            Lexer::Json => false,
        }
    }

    /// [`Lexed::splice`] for LSP edits, their positions read with `offset`;
    /// the edited text only when `keep`.
    fn edit(
        &self,
        edits: &[TextEdit],
        offset: impl Fn(Position) -> Option<u32>,
        keep: bool,
    ) -> Result<String, String> {
        let mut spans = Vec::with_capacity(edits.len());
        for e in edits {
            match (offset(e.range.start), offset(e.range.end)) {
                (Some(s), Some(t)) if s <= t => {
                    spans.push((s as usize, t as usize, e.new_text.as_str()))
                }
                _ => return Err(format!("has an edit outside the text: {e:?}")),
            }
        }
        self.splice(spans, keep)
            .map_err(|m| format!("{m}\n  edits: {edits:?}"))
    }

    /// The text with `edits` (byte ranges and their new texts) applied, if
    /// it lexes to the old tokens with those each edit replaces swapped for
    /// its new text's, lexed alone. An edit that cuts into tokens (the local
    /// name of `<http://e/a>`, the inside of a JSON string or of an embedded
    /// query) is widened to them, and their new text must lex to tokens of
    /// the same kinds; one that adds a directive may not. A directive an
    /// edit adds must follow a document-level statement or directive, or
    /// only comments. Else what is wrong, with the token and its offset.
    fn splice(&self, edits: Vec<(usize, usize, &str)>, keep: bool) -> Result<String, String> {
        let key = (
            edits.iter().map(|&(s, e, n)| (s, e, n.into())).collect(),
            keep,
        );
        if let Some(r) = self.spliced.borrow().get(&key) {
            return r.clone();
        }
        let r = self.splice_anew(edits, keep);
        self.spliced.borrow_mut().insert(key, r.clone());
        r
    }

    /// [`Lexed::splice`], not looked up.
    fn splice_anew(
        &self,
        mut edits: Vec<(usize, usize, &str)>,
        keep: bool,
    ) -> Result<String, String> {
        let (text, lex, toks) = (self.text, self.lex, &self.toks);
        let shown = |s: &str| format!("{:?}", s.chars().take(80).collect::<String>());
        edits.sort_by_key(|e| (e.0, e.1));
        if let Some(w) = edits.windows(2).find(|w| w[0].1 > w[1].0) {
            return Err(format!("has overlapping edits {:?} and {:?}", w[0], w[1]));
        }
        // The token `at` falls strictly inside.
        let inside = |at: usize| {
            toks.get(toks.partition_point(|t| t.2 <= at))
                .filter(|t| t.1 < at)
        };
        // (start, end, edits) of each stretch of the old text an edit set
        // replaces: an edit, widened to the tokens it cuts into.
        let mut regions: Vec<(usize, usize, std::ops::Range<usize>)> = Vec::new();
        for (i, &(s, e, new)) in edits.iter().enumerate() {
            let (mut rs, mut re) = (s, e);
            for t in inside(s).into_iter().chain(inside(e)) {
                (rs, re) = (rs.min(t.1), re.max(t.2));
            }
            // An insertion that fuses with a token it touches is what is left
            // of a rewrite of that token once the text both share is trimmed
            // (`sh:pat` to `sh:path` inserts `h`): it is part of that token.
            if s == e {
                let alone = lex.tokens(new).len();
                let edge = &toks[toks.partition_point(|t| t.2 < s)..];
                for t in edge.iter().take_while(|t| t.1 <= s) {
                    let joined = match (t.2 == s, t.1 == s) {
                        (true, _) => format!("{}{new}", &text[t.1..t.2]),
                        (_, true) => format!("{new}{}", &text[t.1..t.2]),
                        _ => continue,
                    };
                    if lex.tokens(&joined).len() == alone {
                        (rs, re) = (rs.min(t.1), re.max(t.2));
                    }
                }
            }
            match regions.last_mut() {
                Some(r) if rs < r.1 => {
                    r.1 = r.1.max(re);
                    r.2.end = i + 1;
                }
                _ => regions.push((rs, re, i..i + 1)),
            }
        }
        let (Some(first), Some(last)) = (regions.first(), regions.last()) else {
            return Ok(text.to_string());
        };
        // What is lexed again: from two old tokens before the edits to two
        // after them and the tokens that touch those (the Turtle and JSON
        // lexers look only ahead, and not past a space: `@prefix0` is a
        // language tag and `0`), or the whole of a SPARQL query, whose lexer
        // reads a `<` by the tokens before it.
        let (w0, w1) = if lex == Lexer::Sparql {
            (0, text.len())
        } else {
            let before = toks.partition_point(|t| t.2 <= first.0);
            let mut end = toks.partition_point(|t| t.1 < last.1) + 1;
            while toks.get(end + 1).is_some_and(|n| n.1 == toks[end].2) {
                end += 1;
            }
            let w0 = before.checked_sub(2).map_or(0, |i| toks[i].1);
            (w0, toks.get(end).map_or(text.len(), |t| t.2))
        };
        // The window's new text; offsets below are into it.
        let mut after = String::new();
        let mut want: Vec<Tok> = Vec::new();
        let (mut at, mut next) = (w0, toks.partition_point(|t| t.1 < w0));
        regions.push((w1, w1, edits.len()..edits.len()));
        for (rs, re, range) in regions {
            let base = after.len();
            while next < toks.len() && toks[next].2 <= rs {
                let t = toks[next];
                want.push((t.0, t.1 + base - at, t.2 + base - at));
                next += 1;
            }
            after += &text[at..rs];
            if range.is_empty() {
                break;
            }
            let mut region = String::new();
            let mut from = rs;
            for &(s, e, new) in &edits[range.clone()] {
                region += &text[from..s];
                region += new;
                from = e;
            }
            region += &text[from..re];
            let new = lex.tokens(&region);
            let old = &toks[next..next + toks[next..].partition_point(|t| t.1 < re)];
            let widened = rs < edits[range.start].0 || re > edits[range.end - 1].1;
            let like = |a: &Tok, b: &Tok| a.0 == b.0 || lex.term(a.0) && lex.term(b.0);
            let directives =
                |ts: &[Tok], src: &str| ts.iter().filter(|t| lex.directive(src, **t)).count();
            let adds = directives(&new, &region) > directives(old, text);
            // A directive written into a token it then joins (a string) is
            // seen in the edit's own text: a keyword and an IRI (`base` in
            // `http://e/base/x` or `prefix` in `sh:prefix` is none).
            let written = edits[range.clone()].iter().any(|&(_, _, new)| {
                let ts = lex.tokens(new);
                let iri = |j: usize| ts.get(j).is_some_and(|t| new[t.1..].starts_with('<'));
                (0..ts.len()).any(|j| lex.directive(new, ts[j]) && (iri(j + 1) || iri(j + 2)))
            });
            if widened
                && (adds
                    || written
                    || new.len() != old.len()
                    || !new.iter().zip(old).all(|(a, b)| like(a, b)))
            {
                return Err(format!(
                    "cuts into {} at byte {rs}, which becomes {}",
                    shown(&text[rs..re]),
                    shown(&region)
                ));
            }
            let code = toks[..next].iter().rposition(|t| !lex.comment(t.0));
            if adds
                && let Some(j) = code
                && !self.ends_statement(j)
            {
                let p = toks[j];
                return Err(format!(
                    "adds a directive at byte {rs}, after {} at byte {}, which ends no document-level statement or directive",
                    shown(&text[p.1..p.2]),
                    p.1
                ));
            }
            let base = after.len();
            want.extend(new.iter().map(|t| (t.0, t.1 + base, t.2 + base)));
            after += &region;
            next += old.len();
            at = re;
        }
        let got = lex.tokens(&after);
        let whole = || format!("{}{after}{}", &text[..w0], &text[w1..]);
        if let Some(i) = (0..got.len().max(want.len())).find(|&i| got.get(i) != want.get(i)) {
            let token = |t: Option<&Tok>| {
                t.map_or("the end".into(), |t| {
                    format!(
                        "{} (kind {}) at byte {}",
                        shown(&after[t.1..t.2]),
                        t.0,
                        t.1 + w0
                    )
                })
            };
            return Err(format!(
                "makes {} what {} was: {}",
                token(got.get(i)),
                token(want.get(i)),
                first_difference(text, &whole())
            ));
        }
        Ok(if keep { whole() } else { String::new() })
    }
}

/// The external prefixes one iteration in eight is analysed with, as a
/// project's prefixes file or the `turtle.prefixes` setting supplies them.
fn common_prefixes() -> PrefixMap {
    [
        ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
        ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
        ("xsd", "http://www.w3.org/2001/XMLSchema#"),
        ("owl", "http://www.w3.org/2002/07/owl#"),
        ("sh", "http://www.w3.org/ns/shacl#"),
        ("ex", "http://example.org/"),
    ]
    .into_iter()
    .map(|(p, ns)| (p.to_string(), ns.to_string()))
    .collect()
}

/// New names for renames: empty, a local name that reads as a number's
/// exponent, ones ending in a digit, a word.
const RENAMES: [&str; 5] = ["", "e1", "e5", "x1", "renamed"];

/// The edits of `edit` to `uri`.
fn edits_to(edit: Option<&WorkspaceEdit>, uri: &str) -> Vec<TextEdit> {
    edit.and_then(|e| e.changes.as_ref())
        .into_iter()
        .flatten()
        .filter(|(u, _)| u.as_str() == uri)
        .flat_map(|(_, e)| e.iter().cloned())
        .collect()
}

/// The `additional_text_edits` of completion `items` (a prefix declared on
/// the way), each set once, keep the tokens. A completion at `at` inside a
/// string (an embedded query) edits that string; the SPARQL shard checks
/// those against the query's own tokens.
fn imports_keep_the_tokens(
    lexed: &Lexed,
    items: &[CompletionItem],
    offset: impl Fn(Position) -> Option<u32>,
    at: Position,
) {
    // The byte the product completes at: a position past a line's content
    // resolves to before its line break.
    let Some(at) = offset(at) else { return };
    let at = at as usize;
    let toks = &lexed.toks;
    let string = toks
        .get(toks.partition_point(|t| t.2 <= at))
        .filter(|t| t.1 < at && lexed.lex.string(t.0));
    let mut seen: Vec<&Vec<TextEdit>> = Vec::new();
    for item in items {
        let Some(edits) = &item.additional_text_edits else {
            continue;
        };
        if seen.contains(&edits) {
            continue;
        }
        seen.push(edits);
        let within = |t: &Tok| {
            edits.iter().all(|e| {
                offset(e.range.start).is_some_and(|s| t.1 < s as usize && (s as usize) < t.2)
            })
        };
        if string.is_some_and(within) {
            continue;
        }
        if let Err(m) = lexed.edit(edits, &offset, false) {
            panic!("completion '{}' {m}", item.label);
        }
    }
}

fn completion_items(r: Option<CompletionResponse>) -> Vec<CompletionItem> {
    match r {
        Some(CompletionResponse::Array(v)) => v,
        Some(CompletionResponse::List(l)) => l.items,
        None => Vec::new(),
    }
}

/// `before` and `after` (the text `batch::fix_all` made of it) have the
/// same tokens outside their directives; with `unsafe_too`, a term may
/// have become another (a "Change to" fix).
fn fixed_keeps_the_tokens(
    before: &str,
    after: &str,
    lex: Lexer,
    unsafe_too: bool,
) -> Result<(), String> {
    // Each `@prefix` or `PREFIX` line, its name, its IRI and a `.` after it
    // (the Turtle parser reads one after a SPARQL-style line into it too), with the
    // comments among them, left out; none when a keyword starts no such line.
    let outside = |text: &str| {
        let toks = lex.tokens(text);
        let word = |j: usize| toks.get(j).map_or("", |t: &Tok| &text[t.1..t.2]);
        let code = |j: usize| {
            (j..toks.len())
                .find(|&k| !lex.comment(toks[k].0))
                .unwrap_or(toks.len())
        };
        let (mut out, mut i) = (Vec::new(), 0);
        while i < toks.len() {
            let (name, iri) = (code(i + 1), code(code(i + 1) + 1));
            let prefix = word(i) == "@prefix" || word(i).eq_ignore_ascii_case("PREFIX");
            if !(lex.directive(text, toks[i]) && prefix) {
                out.push(toks[i]);
                i += 1;
            } else if word(name).ends_with(':')
                && word(iri).starts_with('<')
                && lex.term(toks[iri].0)
            {
                let dot = code(iri + 1);
                i = if word(dot) == "." && lex != Lexer::Sparql {
                    dot + 1
                } else {
                    iri + 1
                };
            } else {
                return None;
            }
        }
        // A `.` with no statement before it is an empty statement: no content,
        // and which directive the parser folds it into differs between texts.
        let mut prev_dot = true;
        out.retain(|t: &Tok| {
            if lex.comment(t.0) {
                return true;
            }
            let dot = &text[t.1..t.2] == ".";
            let keep = !(dot && prev_dot);
            prev_dot = dot;
            keep
        });
        Some(out)
    };
    // shortcut: what a fix removes of a broken declaration is the parser's
    // reading of it, which the tokens do not show; such texts are not compared.
    let Some(old) = outside(before) else {
        return Ok(());
    };
    let new = outside(after).ok_or("left a declaration that is not a whole line")?;
    let same = |a: &Tok, b: &Tok| {
        (a.0, &before[a.1..a.2]) == (b.0, &after[b.1..b.2])
            || unsafe_too && lex.term(a.0) && lex.term(b.0)
    };
    let i = (0..old.len().max(new.len())).find(|&i| match (old.get(i), new.get(i)) {
        (Some(a), Some(b)) => !same(a, b),
        _ => true,
    });
    match i {
        None => Ok(()),
        Some(i) => {
            let token = |t: Option<&Tok>, s: &str| {
                t.map_or("the end".into(), |t| {
                    format!("{:?} at byte {}", &s[t.1..t.2], t.1)
                })
            };
            Err(format!(
                "changed token {i} outside the directives from {} to {}: {}",
                token(old.get(i), before),
                token(new.get(i), after),
                first_difference(before, after)
            ))
        }
    }
}

/// `batch::fix_all` on `text` alone as file `name`, for one input in four,
/// with unsafe fixes too or safe ones only: no token outside a directive
/// changes, and a text without syntax errors (`clean` then tells whether a
/// fixed text has one) gets none. (Around garbage a fix leaves as it was,
/// the parser's recovery can count differently.) A JSON-LD fix adds
/// `@context` members, not directives: those are only checked for syntax
/// errors.
fn fix_all_keeps_the_tokens(
    rng: &mut Rng,
    name: &str,
    text: &str,
    lex: Lexer,
    clean: Option<&dyn Fn(&str) -> bool>,
) {
    // shortcut: each run costs a workspace (0.1 ms and up), and one on every
    // input would take the push-sized run past its time budget.
    let unsafe_too = match rng.below(8) {
        0 => false,
        1 => true,
        _ => return,
    };
    let file = batch::BatchFile::named(name, text.to_string(), false);
    let out = batch::fix_all(&[file], &Settings::default(), unsafe_too);
    let after = &out.files[0].text;
    let which = if unsafe_too { "unsafe" } else { "safe" };
    assert!(
        !clean.is_some_and(|f| f(after)),
        "fix all ({which}) added a syntax error: {}",
        first_difference(text, after)
    );
    if lex != Lexer::Json
        && let Err(m) = fixed_keeps_the_tokens(text, after, lex, unsafe_too)
    {
        panic!("fix all ({which}) {m}");
    }
}

/// `n` with IRI `from` read as `to`, in triple terms and datatypes too.
fn merged(n: &Node, (from, to): (Iri, Iri)) -> Node {
    match n {
        Node::Iri(i) if *i == from => Node::Iri(to),
        Node::Triple(t) => {
            let p = if t.1 == from { to } else { t.1 };
            Node::Triple(Box::new((
                merged(&t.0, (from, to)),
                p,
                merged(&t.2, (from, to)),
            )))
        }
        Node::Literal { datatype, .. } if *datatype == from => {
            let mut n = n.clone();
            if let Node::Literal { datatype, .. } = &mut n {
                *datatype = to;
            }
            n
        }
        n => n.clone(),
    }
}

/// The sorted triples of `p`, with IRI `merge.0` read as `merge.1`.
fn graph(p: &Parsed, merge: Option<(Iri, Iri)>) -> Vec<String> {
    let node = |n: &Node| merge.map_or_else(|| n.clone(), |m| merged(n, m));
    let iri = |i: Iri| merge.map_or(i, |(from, to)| if i == from { to } else { i });
    let mut v: Vec<String> = p
        .triples
        .iter()
        .map(|t| {
            let g = t.graph.map(|g| match g {
                GraphName::Iri(i) => GraphName::Iri(iri(i)),
                g => g,
            });
            format!("{:?} {:?} {:?} {:?}", node(&t.s), iri(t.p), node(&t.o), g)
        })
        .collect();
    v.sort();
    v
}

/// The IRI a rename at `off` changes and the IRI it becomes for local name
/// `name`, as `navigation::target_at` and `renamed_iri` pick them; none for
/// a prefix, whose rename keeps every IRI.
fn renamed(a: &Analysis, off: u32, name: &str) -> Option<(Iri, Iri)> {
    let inside = |s: turtle_lsp::syntax::Span| s.start <= off && off <= s.end;
    if a.parsed.prefixes.iter().any(|p| inside(p.name_span)) {
        return None;
    }
    let refs = &a.parsed.refs;
    let from = refs.partition_point(|r| r.span.end < off);
    let mut near = refs[from..].iter().take_while(|r| r.span.start <= off);
    let r = near.find(|r| inside(r.span))?;
    if let Form::PName { prefix_len } = r.form
        && off <= r.span.start + prefix_len
    {
        return None;
    }
    let old = r.iri.as_str();
    let cut = old.rfind(['#', '/', ':']).map_or(0, |i| i + 1);
    Some((Iri::from(format!("{}{name}", &old[..cut]).as_str()), r.iri))
}

/// The `after` text of an edit to `text` (document `a`, parsed `before`)
/// parses without errors to the same graph, once `merge` reads a renamed
/// IRI as the old one: `a`, implied datatypes and a collection's
/// `rdf:first` keep the old IRI.
fn keeps_the_graph(
    a: &Analysis,
    text: &str,
    before: &Parsed,
    after: &str,
    merge: Option<(Iri, Iri)>,
) -> Result<(), String> {
    let base = a.parsed.document_base.as_deref();
    let reparsed = parse_doc(after, a.parsed.syntax, base, &a.external);
    let (old, new) = (graph(before, merge), graph(&reparsed, merge));
    let wrong = match reparsed.errors.first() {
        Some(e) => format!("left the syntax error {:?}", e.message()),
        None if old != new => {
            let i = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
            format!("changed triple {:?} to {:?}", old.get(i), new.get(i))
        }
        None => return Ok(()),
    };
    Err(format!("{wrong}: {}", first_difference(text, after)))
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
    // A declaration's line with data on it, or a long string with a line break.
    "@prefix d: <http://d/> . d:s d:p d:o .\n",
    "@prefix q: <http://q/> . q:s q:p \"\"\"a\nb\"\"\" .\n",
    " ; # c\n .",
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

/// Mutates `text` one to four times: inserts, cuts, copies, truncations,
/// squeezes, and now and then a bulk edit that reaches what single snippets
/// do not (a long run of one snippet is a deep nest or a long line, `\n` to a
/// lone `\r`, a huge token).
fn mutate_with(rng: &mut Rng, text: &str, snippets: &[&str], wide: bool, lex: Lexer) -> String {
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
            51..=56 => s = squeeze(rng, &s, lex),
            30..=50 => {
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
    mutate_with(rng, text, SNIPPETS, true, Lexer::Turtle)
}

/// Deletes the whitespace between up to eight pairs of neighbouring tokens
/// that lex the same joined (`a<x>`, `ex:a.b.<x>`, `?p?<x>`): the adjacency
/// an edit must keep apart. Returns `s` unchanged when the whole text would
/// then lex differently.
fn squeeze(rng: &mut Rng, s: &str, lex: Lexer) -> String {
    let lexed = Lexed::new(s, lex);
    let toks = &lexed.toks;
    if toks.len() < 2 {
        return s.to_string();
    }
    let mut gaps: Vec<(usize, usize, &str)> = Vec::new();
    for _ in 0..8 {
        let i = rng.below(toks.len() - 1);
        let (l, r) = (toks[i], toks[i + 1]);
        let joined = format!("{}{}", &s[l.1..l.2], &s[r.1..r.2]);
        let n = l.2 - l.1;
        if l.2 < r.1 && lex.tokens(&joined) == [(l.0, 0, n), (r.0, n, joined.len())] {
            gaps.push((l.2, r.1, ""));
        }
    }
    gaps.sort();
    gaps.dedup();
    lexed.splice(gaps, true).unwrap_or_else(|_| s.to_string())
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
/// Turtle family with `prefixes` from outside.
fn parse_doc(text: &str, syntax: Syntax, base: Option<&str>, prefixes: &PrefixMap) -> Parsed {
    if syntax == Syntax::JsonLd {
        jsonld::parse(text, base, &jsonld::NoNetwork::default())
    } else {
        parse_with_base(text, prefixes, syntax, base)
    }
}

fn exercise(rng: &mut Rng, text: &str, syntax: Syntax, k: &mut Knowledge) {
    // The parser alone: every span must be inside the text and on char boundaries.
    let p = parse_doc(text, syntax, None, &PrefixMap::default());
    spans_are_inside(text, &p);

    // Import sources: catalogs and RDF/XML come from outside too.
    let _ = imports::parse_catalog(text, std::path::Path::new("/chaos"));
    let _ = imports::sniff(text.as_bytes());
    let _ = imports::rdf_xml_to_ntriples(text.as_bytes(), "http://example.com/chaos");

    let external = Arc::new(if rng.below(8) == 0 {
        common_prefixes()
    } else {
        PrefixMap::default()
    });
    let a = Arc::new(Analysis::with_syntax(
        doc_uri(syntax).into(),
        text,
        external.clone(),
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
    let lean = Analysis::with_syntax(doc_uri(syntax).into(), text, external, false, syntax);
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
        // The editor's second Format Document: the same URI and external prefixes.
        let re = Analysis::with_syntax(
            doc_uri(syntax).into(),
            &out,
            a.external.clone(),
            true,
            syntax,
        );
        match format::format(&out, &re.parsed, &re.external, &opts) {
            None => {
                let errors: Vec<String> = re
                    .parsed
                    .errors
                    .iter()
                    .filter(|e| e.severity == Severity::Error)
                    .map(|e| {
                        let at = out.get(e.span.start as usize..e.span.end as usize);
                        format!("{} {:?} at {at:?}", e.code, e.message())
                    })
                    .collect();
                panic!("the formatted text is refused: {errors:?}\n  formatted: {out:?}");
            }
            Some(again) => assert!(
                again == out,
                "formatting is not idempotent: {}",
                first_difference(&out, &again)
            ),
        }
    }
    let mut lexed = Lexed::new(text, Lexer::of(syntax));
    let graphs = a.parsed.graphs.iter();
    lexed.graphs = graphs
        .map(|g| (g.span.start as usize, g.span.end as usize))
        .collect();
    // The text as the refactorings' and renames' graph checks read it.
    let before = a.parsed.errors.is_empty().then(|| {
        let base = a.parsed.document_base.as_deref();
        parse_doc(text, syntax, base, &a.external)
    });
    for enc in [Encoding::Utf8, Encoding::Utf16] {
        let _ = format::formatting(&a, enc, &opts);
        let diags = diagnostics::diagnostics(&a, docs, enc);
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
        quick_fixes_keep_the_tokens(&a, docs, &lexed, enc, diags);
        for _ in 0..4 {
            let off = pick_offset(rng, text);
            let pos: Position = a.position(enc, off);
            let items = completion_items(Some(completion::complete(&a, docs, enc, pos)));
            imports_keep_the_tokens(&lexed, &items, |p| a.offset(enc, p), pos);
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
            let name = RENAMES[rng.below(RENAMES.len())];
            if let Ok(edit) = navigation::rename(&a, docs, enc, pos, name, scope) {
                let edits = edits_to(Some(&edit), doc_uri(syntax));
                let graph = before.is_some() && syntax != Syntax::JsonLd;
                let after = lexed
                    .edit(&edits, |p| a.offset(enc, p), graph)
                    .unwrap_or_else(|m| panic!("renaming to {name:?} {m}"));
                // The IRI renamed is the one at the offset the rename reads `pos` as.
                let target = a.offset(enc, pos).and_then(|at| renamed(&a, at, name));
                if let Some(before) = before.as_ref().filter(|_| graph)
                    && let Err(m) = keeps_the_graph(&a, text, before, &after, target)
                {
                    panic!("renaming to {name:?} {m}\n  edits: {edits:?}");
                }
            }
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
                &lexed,
                enc,
                Range::new(from, to),
                rng.below(2) == 0,
                before.as_ref(),
            );
        }
    }
    let has_errors = |t: &str| {
        let p = parse_doc(t, syntax, None, &PrefixMap::default());
        p.errors.iter().any(|e| match syntax {
            Syntax::JsonLd => e.code == "json-syntax",
            _ => e.severity == Severity::Error,
        })
    };
    let clean = (!has_errors(text)).then_some(&has_errors as &dyn Fn(&str) -> bool);
    let name = format!("doc.{}", extension(syntax));
    fix_all_keeps_the_tokens(rng, &name, text, lexed.lex, clean);
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

/// The quick fixes of every diagnostic of the file (they do not depend on
/// the range asked for) keep the tokens they do not replace, and a
/// declaration one adds goes between document-level statements.
fn quick_fixes_keep_the_tokens(
    a: &Analysis,
    k: &Knowledge,
    lexed: &Lexed,
    enc: Encoding,
    diagnostics: Vec<lsp_types::Diagnostic>,
) {
    let params = CodeActionParams {
        text_document: TextDocumentIdentifier::new(doc_uri(a.parsed.syntax).parse().unwrap()),
        range: Range::default(),
        context: lsp_types::CodeActionContext {
            diagnostics,
            ..Default::default()
        },
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    for c in quickfix::code_actions(a, k, enc, &params) {
        if let CodeActionOrCommand::CodeAction(c) = c
            && let Err(m) = lexed.edit(&refactor::edits_of(a, enc, &c), |p| a.offset(enc, p), false)
        {
            panic!("'{}' {m}", c.title);
        }
    }
}

/// Every refactoring keeps the tokens it does not replace; on a file
/// without syntax errors (parsed `before`), it also keeps the parsed
/// triples.
fn code_actions_keep_the_graph(
    a: &Analysis,
    lexed: &Lexed,
    enc: Encoding,
    range: Range,
    lazy: bool,
    before: Option<&Parsed>,
) {
    let params = CodeActionParams {
        text_document: TextDocumentIdentifier::new(doc_uri(a.parsed.syntax).parse().unwrap()),
        range,
        context: Default::default(),
        work_done_progress_params: Default::default(),
        partial_result_params: Default::default(),
    };
    for c in refactor::code_actions(a, enc, &params, lazy) {
        let CodeActionOrCommand::CodeAction(c) = c else {
            continue;
        };
        let edits = refactor::edits_of(a, enc, &c);
        let after = lexed
            .edit(&edits, |p| a.offset(enc, p), before.is_some())
            .unwrap_or_else(|m| panic!("'{}' {m}", c.title));
        if let Some(before) = before
            && let Err(m) = keeps_the_graph(a, lexed.text, before, &after, None)
        {
            panic!("'{}' {m}\n  edits: {edits:?}", c.title);
        }
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
        "iteration {i} (seed {seed}, {kind}; replay with CHAOS_SEED={seed} CHAOS_ITERS={iters} CHAOS_ONLY={i}) failed: {why}\n  input ({} bytes): {head:?}{more}\n  saved to {}",
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
    let mut dog = Watchdog::new("turtle", seed, iters);
    for i in iterations(iters, k) {
        let mut rng = Rng::at(seed, i);
        let pick = rng.below(corpus.len());
        let (source, syntax) = &corpus[pick];
        let mut syntax = *syntax;
        let text = mutate(&mut rng, source);
        // Turtle is nearly TriG: read some Turtle files as TriG too.
        if syntax == Syntax::Turtle && rng.below(4) == 0 {
            syntax = Syntax::TriG;
        }
        dog.start(i, &text, extension(syntax));
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
    let mut dog = Watchdog::new("jsonld", seed, iters);
    for i in iterations(iters, k) {
        let mut rng = Rng::at(seed, i);
        let source = &seeds[rng.below(seeds.len())];
        let text = mutate_with(&mut rng, source, JSON_SNIPPETS, false, Lexer::Json);
        dog.start(i, &text, "jsonld");
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
    "REGEX(REGEX(REGEX(",
    "!(",
    ":f(REGEX(",
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
    mutate_with(rng, text, SPARQL_SNIPPETS, true, Lexer::Sparql)
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
        assert!(
            twice == out,
            "formatting is not idempotent: {}",
            first_difference(&out, &twice)
        );
    }

    let external = Arc::new(if rng.below(8) == 0 {
        common_prefixes()
    } else {
        PrefixMap::default()
    });
    let error = |doc: &Document| doc.diagnostics().iter().any(|d| d.level == Level::Error);
    let has_errors = |t: &str| error(&Document::parse(t, &external, base));
    let had = if external.is_empty() {
        error(&doc)
    } else {
        has_errors(text)
    };
    let clean = (!had).then_some(&has_errors as &dyn Fn(&str) -> bool);
    let lexed = Lexed::new(text, Lexer::Sparql);
    let file = Arc::new(SparqlAnalysis::new(
        "file:///chaos/q.rq".into(),
        text,
        external.clone(),
    ));
    let embedded = Arc::new(SparqlAnalysis::embedded(
        "file:///chaos/shapes.ttl".into(),
        text,
        external.clone(),
    ));
    for q in [&file, &embedded] {
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            sparql_features_at_random(rng, k, q, enc, &file, &lexed, clean);
        }
    }
    fix_all_keeps_the_tokens(rng, "q.rq", text, Lexer::Sparql, clean);
}

/// The features at random positions of `q` (the text `lexed`), each edit
/// checked against its tokens; `clean`, for a query without syntax errors,
/// tells whether an edited text has one.
fn sparql_features_at_random(
    rng: &mut Rng,
    k: &Knowledge,
    q: &Arc<SparqlAnalysis>,
    enc: Encoding,
    others: &Arc<SparqlAnalysis>,
    lexed: &Lexed,
    clean: Option<&dyn Fn(&str) -> bool>,
) {
    let text = lexed.text;
    let edited = |what: &str, edits: &[TextEdit]| {
        let after = lexed
            .edit(edits, |p| q.offset(enc, p), clean.is_some())
            .unwrap_or_else(|m| panic!("{what} {m}"));
        assert!(
            !clean.is_some_and(|f| f(&after)),
            "{what} left a syntax error in a query without one: {}\n  edits: {edits:?}",
            first_difference(text, &after)
        );
    };
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
            let items = completion_items(sparql_features::completion(q, k, enc, &params));
            imports_keep_the_tokens(lexed, &items, |p| q.offset(enc, p), at);
            let _ = sparql_features::hover(q, k, enc, at);
            let _ = sparql_features::definition(q, k, enc, at, None);
            let _ = sparql_features::references(q, k, enc, at, true, others);
            let _ = sparql_features::highlights(q, enc, at);
            let _ = sparql_features::prepare_rename(q, enc, at);
            let name = RENAMES[rng.below(RENAMES.len())];
            if let Ok(edit) = sparql_features::rename(q, k, enc, at, name, None, others) {
                edited(
                    &format!("renaming to {name:?}"),
                    &edits_to(Some(&edit), &q.uri),
                );
            }
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
                let a = sparql_features::resolve_code_action(q, enc, a);
                edited(
                    &format!("'{}'", a.title),
                    &edits_to(a.edit.as_ref(), &q.uri),
                );
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
    let mut dog = Watchdog::new("sparql", seed, iters);
    for i in iterations(iters, k) {
        let mut rng = Rng::at(seed, i);
        let pick = rng.below(corpus.len());
        let text = sparql_mutate(&mut rng, &corpus[pick]);
        // shortcut: an update is saved as .rq too; telling them apart parses, which can hang.
        dog.start(i, &text, "rq");
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
        // 200 levels, under `MAX_NESTING`: spargebra parses them.
        (
            "nested regex",
            format!("SELECT * WHERE {{ FILTER({}?x) }}", "REGEX(".repeat(200)),
        ),
        (
            "nested not",
            format!("SELECT * WHERE {{ FILTER({}?x) }}", "!(".repeat(200)),
        ),
        (
            "nested custom call",
            format!(
                "PREFIX : <http://example.org/>\nSELECT * WHERE {{ FILTER({}?x) }}",
                ":f(".repeat(200)
            ),
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

/// An iteration past its budget: the child (this test binary, run again on
/// this test) exits with 101 and writes the report to stderr, past libtest's
/// capture.
#[test]
fn a_hung_iteration_is_reported_and_exits_the_run() {
    if std::env::var_os("CHAOS_WATCHDOG_CHILD").is_some() {
        let mut dog = Watchdog::new("turtle", 7, 2000);
        dog.start(5, "<a> <b> <c> .", "ttl");
        std::thread::sleep(Duration::from_secs(30));
        panic!("the watchdog did not fire");
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "a_hung_iteration_is_reported_and_exits_the_run"])
        .env("CHAOS_WATCHDOG_CHILD", "1")
        .env("CHAOS_ITERATION_BUDGET_MS", "100")
        .output()
        .unwrap();
    let dump = Path::new(env!("CARGO_TARGET_TMPDIR")).join("chaos-turtle-7-5.ttl");
    assert_eq!(fs::read_to_string(&dump).unwrap(), "<a> <b> <c> .");
    fs::remove_file(&dump).unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(101), "{stderr}");
    assert!(
        stderr.contains("iteration 5 ")
            && stderr.contains("CHAOS_SEED=7 CHAOS_ITERS=2000 CHAOS_ONLY=5")
            && stderr.contains("budget 100ms"),
        "{stderr}"
    );
}

/// `text` with `new` put in place of the first `old` (at its end when
/// `old` is empty and `after` is set), through the token check.
fn spliced(text: &str, lex: Lexer, old: &str, at: usize, new: &str) -> Result<String, String> {
    let s = text.find(old).expect("old is in the text") + at;
    let e = if at > 0 { s } else { s + old.len() };
    Lexed::new(text, lex).splice(vec![(s, e, new)], true)
}

#[test]
fn a_declaration_goes_only_between_document_level_statements() {
    let text = "# header\nex:s ex:p \"\"\"x\ny\"\"\" .\nPREFIX a: <http://a/>\n";
    let decl = "@prefix b: <http://b/> .\n";
    // Into a long string: a cut, named with the string and where it starts.
    let m = spliced(text, Lexer::Turtle, "y\"", 0, &format!("{decl}y\"")).unwrap_err();
    assert!(
        m.contains("cuts into \"\\\"\\\"\\\"x\\ny\\\"\\\"\\\"\" at byte 19"),
        "{m}"
    );
    // After a subject: no statement ends there.
    let m = spliced(text, Lexer::Turtle, "ex:s", 4, &format!("\n{decl}")).unwrap_err();
    assert!(m.contains("after \"ex:s\" at byte 9, which ends no"), "{m}");
    // After the header, a `.` or a SPARQL-style line: fine.
    for (after, at) in [("# header\n", 9), (" .\n", 3), ("<http://a/>\n", 12)] {
        spliced(text, Lexer::Turtle, after, at, decl).unwrap();
    }
    // In TriG, not inside a graph block.
    let trig = "ex:g { ex:s ex:p ex:o . }\n";
    let mut lexed = Lexed::new(trig, Lexer::TriG);
    lexed.graphs = vec![(0, trig.len() - 1)];
    let inside = trig.find(" }").unwrap();
    assert!(
        lexed
            .splice(vec![(inside, inside, "\n@prefix b: <http://b/> .")], true)
            .is_err()
    );
    let end = trig.len();
    lexed.splice(vec![(end, end, decl)], true).unwrap();
    // SPARQL: after a PREFIX line, and not after a pattern's `;`.
    let q = "PREFIX a: <http://a/>\nSELECT * { ?s a:p 1 ; a:q 2 }";
    spliced(
        q,
        Lexer::Sparql,
        "<http://a/>\n",
        12,
        "PREFIX b: <http://b/>\n",
    )
    .unwrap();
    let m = spliced(q, Lexer::Sparql, "; ", 2, "PREFIX b: <http://b/>\n").unwrap_err();
    assert!(m.contains("after \";\" at byte"), "{m}");
    // A completion at the end of a text whose open string ends in `\r` is
    // inside the string: its import goes into the embedded query.
    let text = "ex:s ex:p \"\"\"SELECT\r";
    let a = Analysis::new("file:///t/a.ttl".into(), text, Arc::default(), true);
    let at = Position::new(0, 13);
    let item = CompletionItem {
        additional_text_edits: Some(vec![TextEdit::new(Range::new(at, at), decl.into())]),
        ..CompletionItem::default()
    };
    let end = a.position(Encoding::Utf8, text.len() as u32);
    let offset = |p| a.offset(Encoding::Utf8, p);
    imports_keep_the_tokens(&Lexed::new(text, Lexer::Turtle), &[item], offset, end);
}

#[test]
fn a_rewrite_that_fuses_or_splits_tokens_names_the_token() {
    // `ex:o` before `.ex:t` is one name: the count of tokens drops by two.
    let text = "<http://e/s> <http://e/p> <http://e/o>.ex:t <http://e/p> 1 .";
    let m = spliced(text, Lexer::Turtle, "<http://e/o>", 0, "ex:o").unwrap_err();
    assert!(
        m.contains("makes \"ex:o.ex:t\" (kind 1) at byte 26 what \"ex:o\""),
        "{m}"
    );
    // A split and a fusion keep the count: the sequence still differs.
    let text = "ex:s ex:p ex:o , ex:q :x .";
    let (o, q) = (text.find("ex:o").unwrap(), text.find(" :x").unwrap());
    let edits = vec![(o, o + 4, "ex:o ex:x"), (q, q + 1, "")];
    let m = Lexed::new(text, Lexer::Turtle)
        .splice(edits, true)
        .unwrap_err();
    assert!(m.contains("makes \"ex:q:x\" (kind 1) at byte 22"), "{m}");
    // SPARQL: `ex:a` before `.5` is one name.
    let q = "SELECT * { ?s ?p <http://e/a>.5 }";
    let m = spliced(q, Lexer::Sparql, "<http://e/a>", 0, "ex:a").unwrap_err();
    assert!(m.contains("\"ex:a.5\""), "{m}");
    // A trimmed rewrite (`sh:pat` to `sh:path`) and a rename inside an IRI.
    spliced("ex:s sh:pat ex:o .", Lexer::Turtle, "sh:pat", 6, "h").unwrap();
    spliced(
        "ex:s ex:p <http://e/alice> .",
        Lexer::Turtle,
        "alice",
        0,
        "e1",
    )
    .unwrap();
    // JSON: a member written inside a string.
    let json = "{\"@context\": {\"a\": \"http://a/\"}}";
    let m = spliced(json, Lexer::Json, "//a/", 0, "\"ex\": \"http://e/\", ").unwrap_err();
    assert!(
        m.contains("cuts into \"\\\"http://a/\\\"\" at byte 19"),
        "{m}"
    );
}

#[test]
fn fix_all_may_change_only_the_directives() {
    let before = "ex:s ex:p ex:o , ex:q .\n";
    let fixed = "@prefix ex: <http://e/> .\nex:s ex:p ex:o , ex:q .\n";
    fixed_keeps_the_tokens(before, fixed, Lexer::Turtle, false).unwrap();
    let fused = "@prefix ex: <http://e/> .\nex:s ex:pex:o , ex:q .\n";
    let m = fixed_keeps_the_tokens(before, fused, Lexer::Turtle, false).unwrap_err();
    assert!(
        m.contains("from \"ex:p\" at byte 5 to \"ex:pex:o\" at byte 31"),
        "{m}"
    );
    // An unsafe fix may change a term to another.
    let changed = "ex:s ex:p <http://e/o> , ex:q .\n";
    fixed_keeps_the_tokens(before, changed, Lexer::Turtle, true).unwrap();
    assert!(fixed_keeps_the_tokens(before, changed, Lexer::Turtle, false).is_err());
    // A broken `<...>` lexes as an error and a comment `#> .`: not compared.
    let broken = "@prefix a: <http://x/a\t#> .\n@prefix b: <http://x/b#> .\n.\n";
    fixed_keeps_the_tokens(broken, ".\n", Lexer::Turtle, false).unwrap();
    // A PREFIX line put above a stray `.` takes it in; an empty statement
    // carries nothing, so the texts still compare equal.
    let stray = "# c\n.\n[ ex:a true ] .\n";
    let added = "# c\nPREFIX ex: <http://e/>\n.\n[ ex:a true ] .\n";
    fixed_keeps_the_tokens(stray, added, Lexer::Turtle, false).unwrap();
}

#[test]
fn a_rename_keeps_the_graph_but_for_the_new_iri() {
    let text = "@prefix ex: <http://e/> .\nex:a ex:p ex:b , \"1\"^^ex:a .\n";
    let a = Analysis::with_syntax(
        doc_uri(Syntax::Turtle).into(),
        text,
        Arc::new(PrefixMap::default()),
        true,
        Syntax::Turtle,
    );
    let base = a.parsed.document_base.as_deref();
    let before = parse_doc(text, Syntax::Turtle, base, &PrefixMap::default());
    let at = text.find("ex:a").unwrap() as u32 + 3;
    let target = renamed(&a, at, "e1");
    assert_eq!(
        target,
        Some((Iri::from("http://e/e1"), Iri::from("http://e/a")))
    );
    let good = text.replace("ex:a", "ex:e1");
    keeps_the_graph(&a, text, &before, &good, target).unwrap();
    let bad = good.replace("ex:b", "ex:c");
    let m = keeps_the_graph(&a, text, &before, &bad, target).unwrap_err();
    assert!(
        m.contains("changed triple") && m.contains("http://e/b"),
        "{m}"
    );
    // On the prefix, nothing is renamed.
    assert_eq!(renamed(&a, 9, "e1"), None);
}

#[test]
fn a_squeeze_keeps_the_tokens() {
    let text = "<http://e/s> <http://e/p> 1 , \"x\" ; <http://e/q> ex:o .";
    let toks = |t: &str| {
        let ts = Lexer::Turtle.tokens(t);
        ts.iter()
            .map(|x| (x.0, t[x.1..x.2].to_string()))
            .collect::<Vec<_>>()
    };
    let mut shorter = false;
    for seed in 1..50 {
        let out = squeeze(&mut Rng(seed), text, Lexer::Turtle);
        assert_eq!(toks(&out), toks(text), "{out:?}");
        shorter |= out.len() < text.len();
    }
    assert!(shorter, "no squeeze took");
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
// Defects the chaos tests found, kept as fixed inputs. A new one is fixed,
// not ignored: no test in this file is `#[ignore]`d and no check has an
// exemption.

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
