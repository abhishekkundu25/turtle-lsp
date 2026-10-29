# Incremental keystroke tickets

Spec: docs/specs/incremental-keystroke.md. Research:
docs/research/incremental-reparse.md. Built with parallel slices on the
integration branch `feat/incremental-keystroke`, one PR, stacked on PR #47
(label lints), whose knowledge index and `big-vocab` benchmark it builds on.

| # | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|
| 00 | Measure: lexing, parsing, a span shift, the index swap | — | none (done by the orchestrator) |
| 01 | The oracle: incremental equals a fresh workspace; chaos edit mode | — | new src/oracle.rs; **src/lib.rs**; **src/syntax/mod.rs** (derives only); **src/knowledge.rs** (`snapshot` only); tests/chaos.rs |
| 02 | Index delta: counts per IRI, `put` applies the difference, change set | — | **src/knowledge.rs** (all but `snapshot`) |
| 03 | Keep the edits and the line starts; a per-line line table | — | src/workspace.rs; src/analysis.rs |
| 04 | Statement splice: the parser's start state and the splice module | — | src/syntax/parser.rs; new src/syntax/splice.rs; **src/syntax/mod.rs** (one `mod` line) |
| 05 | Wire the splice: analysis, workspace, setting, chaos floor, debug check | 01, 03, 04 | src/analysis.rs; src/workspace.rs; src/settings.rs; editors/vscode/package.json; src/server.rs (debug switch); tests/chaos.rs; new test/incremental.test.mjs |
| 06 | Diagnostics per statement, and reuse by the index's change set | 02, 05 | src/diagnose.rs; src/features/diagnostics.rs; src/features/curation.rs; src/workspace.rs |
| 07 | Close-out: corpus edit test, docs, full validation, bench | 01–06 | README.md; CHANGELOG.md; ARCHITECTURE.md; editors/vscode/README.md; tests/corpus test; full validation |

Lock files and how the edits stay apart:

- `src/syntax/mod.rs`: 01 adds `PartialEq` to the derives of `Parsed` and
  the types it holds; 04 adds one `pub(crate) mod splice;` line.
- `src/knowledge.rs`: 01 only removes `#[cfg(test)]` from
  `Knowledge::snapshot` and makes it `pub(crate)`; 02 owns the rest, and
  extends `snapshot` only after 01 has landed (02 starts first, so it leaves
  `snapshot` alone and says in its report if new fields need dumping).
- `src/lib.rs`: 01 adds one `#[doc(hidden)] pub mod oracle;` line and its
  line in the "Public interface" doctest.
- `src/analysis.rs`, `src/workspace.rs`: 03, then 05, then 06 (one at a
  time).
- `tests/chaos.rs`: 01, then 05.

Waves: 00 (orchestrator); then 01, 02, 03 and 04 together; then 05; then
06; then 07.

## Part 0 results (2026-10-29)

A temporary example timed the pieces in process (median of 15, load about
2.5 from other apps, so the ratios count, not the absolute numbers):

| File | Size | Tokens | Lex | Lex + parse | Shift every span after the middle | Whole analysis | Index swap (`Knowledge::set`) |
|---|---:|---:|---:|---:|---:|---:|---:|
| big-file | 2.5 MB | 223,616 | 3.75 ms | 6.44 ms | 0.16 ms | 9.16 ms | 1.27 ms |
| big-vocab | 2.4 MB | 255,024 | 2.73 ms | 7.16 ms | 0.17 ms | 10.70 ms | 2.60 ms |

The shift costs about 2.5% of the parse, so the splice is worth building
(the spec's gate was "stop after part 2 if the shift costs most of the
parse"). Lexing is over half of the parse: re-lexing only the window
matters as much as re-parsing only it. The analysis maps and line table
after the parse are about 2.7 to 3.5 ms; part 3 rebuilds the maps in full
at first, so they become the next target if the bench says so.

`_:` labels: 1,224 of the 1,356 Turtle and TriG corpus files have at least
one (most are small W3C test files). The fallback is per window (a `_:` in
the old or new window), not per file, so this does not rule the splice out.

## Departures from the plan

- One PR for the four parts, not one per part (the user asked for one
  branch and one PR). Each part still lands as its own commits, and the
  close-out records the bench after each.
- Part 3 is split into 04 (the splice on `Parsed` alone, with unit tests)
  and 05 (wiring), so the parser work runs alongside 01 to 03.
- 03: the line table keeps a per-line flag (has a multi-byte character)
  and marks only inside such lines over 4 KB, counted from the line start,
  instead of running totals: every conversion is a column inside one line,
  so nothing reads totals, and an edit shifts no counts. A conversion on a
  short multi-byte line scans up to that line's length (at most 4 KB)
  instead of 64 bytes. The kept state is an enum on the document
  (`Analyzed`: none, current, failed, outdated, edited); an edit after a
  prefix or context change keeps the outdated analysis as `last`, so 05
  must fall back when the prefixes or base in effect differ.
