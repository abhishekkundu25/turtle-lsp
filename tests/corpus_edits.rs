//! Every Turtle, TriG, N-Triples and N-Quads file of the public corpus,
//! opened and then edited as a client edits it, must give what a fresh open
//! of the edited text gives (`turtle_lsp::oracle`, which also checks each
//! spliced analysis against a full one as it goes).
//!
//! Each file gets edits at fixed places, picked from its tokens near a
//! quarter, the half and three quarters of the text, so a run is the same
//! on every machine. At each place:
//!
//! - one session of keystrokes the statement splice takes: a character
//!   typed inside a string, inside a comment and inside a name (two more
//!   typed before the next analysis), then a statement deleted and typed
//!   back;
//! - one session per splice hazard, each from the file as it is: a `"""`, a
//!   `#`, an `@prefix` line or a `{` at the start of a line, a line break
//!   before a subject at the start of a line, and a `.` after a number;
//!   then a character typed inside a string, after the hazard.
//!
//! Places a file lacks (a comment, a number) are left out for that file.
//! The changes' positions are in UTF-8 for every other file and UTF-16 for
//! the rest. Each shard prints how the analyses were parsed (the splice or
//! why not), for the named files at the top of the corpus and for each
//! folder of files apart, and the time of each file over 1 MB. Every file
//! is edited: nothing is capped. (Most SIB examples are one statement with
//! a `_:` label in it, so most of their edits fall back.) A file that
//! differs is named with the session and the first field that differs, and
//! the test fails after the whole shard ran.
//!
//! The corpus is downloaded by `tools/corpus/fetch.sh` (`npm run corpus`)
//! into `test/corpus/`, which is not tracked. Without it the test prints
//! `skipped`, and fails when `CI=true`.
//!
//!     cargo test --release --locked --test corpus_edits -- --nocapture

use lsp_types::TextDocumentContentChangeEvent;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;
use turtle_lsp::analysis::Encoding;
use turtle_lsp::oracle::{self, Batch};
use turtle_lsp::syntax::lexer::{Kind, tokenize};
use turtle_lsp::syntax::{PrefixMap, Syntax, parse};

include!("support/corpus.rs");

/// The shards run as separate tests, so the harness runs them side by side.
const SHARDS: usize = 4;

#[test]
fn edited_corpus_matches_a_fresh_open_1() {
    edited_corpus_matches_a_fresh_open(0);
}

#[test]
fn edited_corpus_matches_a_fresh_open_2() {
    edited_corpus_matches_a_fresh_open(1);
}

#[test]
fn edited_corpus_matches_a_fresh_open_3() {
    edited_corpus_matches_a_fresh_open(2);
}

#[test]
fn edited_corpus_matches_a_fresh_open_4() {
    edited_corpus_matches_a_fresh_open(3);
}

fn edited_corpus_matches_a_fresh_open(k: usize) {
    let Some(dir) = corpus_path("") else {
        return;
    };
    let mut files = Vec::new();
    collect(&dir, &mut files);
    // The largest first, dealt round the shards, so each gets its share of
    // the big files.
    files.sort_by_key(|(p, _)| {
        (
            std::cmp::Reverse(fs::metadata(p).map_or(0, |m| m.len())),
            p.clone(),
        )
    });
    let mut paths: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let (mut sessions, mut failures) = (0, Vec::new());
    let mine: Vec<_> = files
        .iter()
        .enumerate()
        .filter(|(i, _)| i % SHARDS == k)
        .collect();
    for (i, (path, syntax)) in &mine {
        let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let name = path
            .strip_prefix(&dir)
            .unwrap_or(path)
            .display()
            .to_string();
        let group = match name.split_once(['/', '\\']) {
            Some((folder, _)) => folder.to_string(),
            None => "named files".to_string(),
        };
        let enc = [Encoding::Utf8, Encoding::Utf16][i % 2];
        let t0 = Instant::now();
        for (session, batches, want) in sessions_for(&text, *syntax, enc) {
            sessions += 1;
            match oracle::check(*syntax, &text, batches, enc) {
                Err(diff) => failures.push(format!("{name} ({session}, {enc:?}): {diff}")),
                Ok(r) if r.text != want => {
                    failures.push(format!("{name} ({session}): the edits left another text"))
                }
                Ok(r) => {
                    let counts = paths.entry(group.clone()).or_default();
                    for p in r.paths {
                        *counts.entry(format!("{p:?}")).or_default() += 1;
                    }
                }
            }
        }
        if text.len() > 1 << 20 {
            eprintln!("  {name}: {} bytes, {:?}", text.len(), t0.elapsed());
        }
    }
    eprintln!(
        "corpus edits, shard {}: {} files, {sessions} sessions",
        k + 1,
        mine.len()
    );
    for (group, counts) in &paths {
        eprintln!("  analyses, {group}: {counts:?}");
    }
    assert!(
        failures.is_empty(),
        "{} edit sessions differ from a fresh open:\n{}",
        failures.len(),
        failures.join("\n")
    );
    let spliced = paths
        .values()
        .filter_map(|c| c.get("Spliced"))
        .sum::<usize>();
    assert!(spliced > 0, "no analysis was spliced");
}

/// The RDF files under `dir`, all levels, with their syntax.
fn collect(dir: &Path, out: &mut Vec<(PathBuf, Syntax)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            collect(&p, out);
        } else if let Some(s) = Syntax::from_extension(&p.to_string_lossy())
            && s != Syntax::JsonLd
        {
            out.push((p, s));
        }
    }
}

/// The text being edited, and the changes since the last batch.
struct Edited {
    text: String,
    enc: Encoding,
    changes: Vec<TextDocumentContentChangeEvent>,
    batches: Vec<Batch>,
}

impl Edited {
    fn new(text: &str, enc: Encoding) -> Edited {
        Edited {
            text: text.to_string(),
            enc,
            changes: Vec::new(),
            batches: Vec::new(),
        }
    }

    fn replace(&mut self, range: std::ops::Range<usize>, new: &str) {
        self.changes
            .push(oracle::change(&self.text, range.clone(), new, self.enc));
        self.text.replace_range(range, new);
    }

    fn insert(&mut self, at: usize, new: &str) {
        self.replace(at..at, new);
    }

    /// End the batch, with the analysis asked for after it or not.
    fn send(&mut self, analyse: bool) {
        if !self.changes.is_empty() {
            self.batches.push(Batch {
                changes: std::mem::take(&mut self.changes),
                analyse,
            });
        }
    }

    fn done(mut self, session: &'static str) -> Option<(&'static str, Vec<Batch>, String)> {
        self.send(true);
        (!self.batches.is_empty()).then_some((session, self.batches, self.text))
    }
}

/// The token of a kind in `kinds` that starts nearest `at` of the way
/// through `text`.
fn near(text: &str, kinds: &[Kind], at: Place) -> Option<(usize, usize)> {
    let mid = text.len() * at.0 / 4;
    tokenize(text)
        .iter()
        .filter(|t| kinds.contains(&t.kind))
        .map(|t| (t.start as usize, t.end as usize))
        .min_by_key(|(s, _)| s.abs_diff(mid))
}

/// The first line start at or after `at` of the way through `text` where a
/// line holds something other than blanks or a comment, if any.
fn line_start(text: &str, at: Place) -> Option<usize> {
    let b = text.as_bytes();
    (text.len() * at.0 / 4..b.len()).find(|&i| {
        i > 0
            && (b[i - 1] == b'\n' || (b[i - 1] == b'\r' && b[i] != b'\n'))
            && !b[i].is_ascii_whitespace()
            && b[i] != b'#'
    })
}

/// A character typed inside the string nearest `at`, after its opening
/// quotes.
fn in_string(e: &mut Edited, at: Place) -> bool {
    let Some((s, _)) = near(&e.text, &[Kind::String], at) else {
        return false;
    };
    let long = e.text[s..].starts_with("\"\"\"") || e.text[s..].starts_with("'''");
    e.insert(s + if long { 3 } else { 1 }, "x");
    true
}

/// Where an edit goes: the token or line start nearest `n` quarters of
/// the way through the text.
#[derive(Clone, Copy)]
struct Place(usize);

const PLACES: [Place; 3] = [Place(1), Place(2), Place(3)];

/// The edit sessions for one file, each with the text it must leave.
fn sessions_for(
    text: &str,
    syntax: Syntax,
    enc: Encoding,
) -> Vec<(&'static str, Vec<Batch>, String)> {
    let mut out = Vec::new();
    for at in PLACES {
        let mut e = Edited::new(text, enc);
        if in_string(&mut e, at) {
            e.send(true);
        }
        if let Some((_, end)) = near(&e.text, &[Kind::Comment], at) {
            e.insert(end, " edited");
            e.send(true);
        }
        if let Some((s, end)) = near(&e.text, &[Kind::PName, Kind::Iri], at) {
            // Before an IRI's `>`; after a prefixed name.
            let at = if e.text[s..].starts_with('<') {
                end - 1
            } else {
                end
            };
            e.insert(at, "y");
            e.send(true);
            e.insert(at + 1, "z");
            e.send(false);
            e.insert(at + 2, "w");
            e.send(true);
        }
        let parsed = parse(&e.text, &PrefixMap::default(), syntax);
        let n = parsed.statements.len();
        if let Some(st) = parsed.statements.get(n * at.0 / 4) {
            let (s, end) = (st.span.start as usize, st.span.end as usize);
            let statement = e.text[s..end].to_string();
            e.replace(s..end, "");
            e.send(true);
            e.insert(s, &statement);
            e.send(true);
        }
        out.extend(e.done("keystrokes"));

        let at_line: [(&'static str, &str); 5] = [
            ("long string", "\"\"\""),
            ("comment", "#"),
            ("prefix", "@prefix zz: <http://example.org/zz#> .\n"),
            ("brace", "{ "),
            ("line break", "\n"),
        ];
        if let Some(line) = line_start(text, at) {
            for (session, hazard) in at_line {
                let mut e = Edited::new(text, enc);
                e.insert(line, hazard);
                e.send(true);
                in_string(&mut e, at);
                out.extend(e.done(session));
            }
        }
        if let Some((_, end)) = near(text, &[Kind::Number], at) {
            let mut e = Edited::new(text, enc);
            e.insert(end, ".");
            e.send(true);
            in_string(&mut e, at);
            out.extend(e.done("dot after a number"));
        }
    }
    out
}
