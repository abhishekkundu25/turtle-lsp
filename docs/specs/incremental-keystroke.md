## Problem Statement

Typing in a big file is slower than it has to be, because every keystroke redoes all the work for the whole file. On a 2.4 MB vocabulary of 17,000 classes, a keystroke takes about 17 ms on main (22 ms with the label checks), against 4 to 5 ms for a normal file. The time goes to five steps, each over the whole document:

| Step | Share | About |
|---|---|---|
| Lexing and parsing | 30% | 5.1 ms |
| Line table | 5% | 0.9 ms |
| Rest of the analysis (defs, refs, labels, types) | 7% | 1.2 ms |
| Index update (the file's facts, then swapping them into the workspace index) | 27% | 4.6 ms |
| Diagnostics | 31% | 5.3 ms |

The editor already sends only the changed range (incremental sync), and the workspace already applies it and moves its line starts. It then throws both away and starts from the full text.

The cost grows with the file, so new checks make it worse in proportion: the label checks added about 25% on vocabulary-shaped files, because each of them walks every label again on every keystroke. Users of big ontologies (schema.org-sized and up) feel this as lag; the roadmap's budget is a p95 keystroke of 12 ms on the 30,000-line file.

The research in `docs/research/incremental-reparse.md` found that most language servers re-parse the whole file (the VS Code JSON and YAML servers, Stardog's Turtle server, Mentor, Qlue-ls, rust-analyzer's analysis path, TypeScript 7), and that the gains come from not redoing the work after the parse. The servers that do reuse old parse results (tree-sitter, Roslyn, IntelliJ, TypeScript up to 6) share four rules: widen the edit by one token, never reuse anything the edit touched or that holds an error, reuse only under the same parser state, and fall back to a full parse when in doubt.

## Solution

A keystroke costs about the size of the edit, not the size of the file, and gives exactly what a full rebuild would give.

The work comes in four parts, each shippable alone and each measured before the next starts:

1. **Index delta.** The workspace index takes the difference between the file's old and new facts, instead of removing every old fact and adding every new one. This targets the 27% without touching the parser.
2. **Keep the edits.** The workspace keeps the edit ranges and the moved line starts until the next analysis, and the line table is reused instead of rebuilt.
3. **Statement splice.** Re-lex from one token before the edit until the new tokens line up with the old ones, re-parse whole statements from one statement before the edit until a new statement starts where an old one did, and splice the result into the old parse. Any case the rules don't cover falls back to a full parse.
4. **Diagnostics per statement.** Checks that read only one statement are cached per statement and recomputed only for the changed ones; checks that read the workspace index are recomputed only when an index entry they read changed.

Every part is proven against a full rebuild by an oracle that runs in the chaos tests on every iteration. A setting turns the new paths off.

## User Stories

1. As an ontology author editing a 2 MB file, I want each keystroke to update diagnostics in a few milliseconds, so that the editor never lags behind my typing.
2. As an ontology author, I want the diagnostics after an edit to be exactly what a fresh open of the file would show, so that I can trust what I see.
3. As an ontology author editing a label in a 17,000-class vocabulary, I want the duplicate-label check to look at the label I changed, so that the cost of the check doesn't grow with the file.
4. As an ontology author, I want typing inside a comment to cost almost nothing, so that writing documentation in a big file is smooth.
5. As an ontology author, I want an edit that opens a `"""` string to still give correct results, even though it changes how the rest of the file reads, so that unfinished typing never leaves stale errors.
6. As an ontology author, I want an edit to an `@prefix` or `@base` line to still give correct results everywhere it applies, so that renaming a prefix is safe.
7. As an ontology author, I want an edit that leaves a statement broken to report the same errors as a full parse, including errors in the statements the broken one swallows, so that error recovery behaves the same.
8. As an ontology author using blank nodes, I want `[ ]` and `( )` edits to keep every later blank node's identity right, so that hover and diagnostics on them stay correct.
9. As an ontology author editing a TriG file, I want edits inside a graph block to be correct, and edits to the block's braces to be correct too, so that dataset files are as safe as Turtle.
10. As an author of N-Triples or N-Quads dumps, I want one-line edits in a large dump to be cheap, so that fixing a line in a 100 MB-class file is practical.
11. As a SPARQL author, I want query files to behave exactly as today, so that nothing changes where the gain is negligible.
12. As a JSON-LD author, I want JSON-LD documents to behave exactly as today, so that context changes stay correct.
13. As a user pasting a large block, I want the paste to be correct and no slower than today, so that big edits never regress.
14. As a user whose editor sends several changes in one batch (multi-cursor, format on type), I want them treated as one edit, so that batches are as cheap as single edits.
15. As a user whose editor sends a full-text change, I want it handled as a full rebuild, so that every client works.
16. As a user with CRLF or lone CR line endings, I want the line table and ranges to stay right after edits, so that diagnostics land on the right line.
17. As a user typing multi-byte characters, I want positions (UTF-16 and code points) to stay right after edits, so that squiggles sit under the right text.
18. As a user of the workspace index (completion, hover, references), I want results after an edit to match a reopened workspace, so that cross-file features never go stale.
19. As a user with the label and house-style checks on, I want cross-file results (a label shared with another file) to refresh when another file changes, as today, so that the delta doesn't lose updates.
20. As a user who hits a bug in the new path, I want a setting that turns it off, so that I can keep working while it is fixed.
21. As a maintainer, I want an oracle that compares every incremental result with a full rebuild, field by field, so that a wrong splice fails a test instead of reaching users.
22. As a maintainer, I want the chaos tests to apply their mutations as edits to the previous text, chained several at a time, so that the incremental path meets the same hostile input as the parser.
23. As a maintainer, I want the chaos run to record which path each iteration took (splice, or which fallback), so that a test fails if the splice path stops being exercised.
24. As a maintainer, I want a debug switch in the server that runs both paths and logs the first difference, so that real editing sessions can be checked without slowing anyone down.
25. As a maintainer, I want the fallback rules strict at first and relaxed one at a time, each with the oracle, so that every relaxation is a reviewed decision.
26. As a maintainer, I want the benchmark to show each part's gain on the big data file and the big vocabulary, so that each part earns its complexity.
27. As a maintainer, I want normal-size files and every existing benchmark row to stay within budget, so that small files never pay for big-file work.
28. As a maintainer, I want the parser itself to stay one parser, with only a way to start it in a given state, so that full and incremental parses can't drift apart.
29. As a maintainer, I want the command line (`check`, `format`) and the browser build to keep using the full path, so that batch tools stay simple and agree with the editor.
30. As a maintainer, I want a new check to declare whether it is statement-local, document-wide or workspace-dependent, so that the diagnostics cache stays correct as checks are added.

## Implementation Decisions

**Order and gates.** The parts land in this order, each as its own PR, each with a measurement first:

0. Measure: split lexing from parsing, the file's facts from the index swap, and time one pass that shifts every span of the big file. The results decide whether the splice is worth building; if the shift pass costs most of the parse, stop after part 2.
1. Index delta.
2. Keep the edits, and reuse the line table.
3. Statement splice, behind a setting, off by default until chaos and the corpus pass, then on.
4. Diagnostics per statement, starting with the statement-local checks.

**Part 1: index delta.**
- The per-file facts record (`DocFacts`) holds a count per IRI for each kind of contribution (subject, object, predicate, reference, definition, prefix binding), instead of sets.
- The index swap (`Knowledge::put`) computes the difference between the old and new records by a sorted merge, and toggles a posting list only when a count moves between zero and non-zero. An unchanged record (for example after typing inside a comment) skips the swap entirely.
- Facts that are per subject rather than per statement (the curation summaries) are recomputed only for subjects whose statements changed, once part 3 gives the changed statements; until then they come from the merge, as `touched_iris` does today.
- The label, definition and comment entries hold statement-relative positions, so a splice before them doesn't move them.
- The change set of a swap (the IRIs and text keys whose index entries changed) is returned to the caller. It already exists as scratch state for the label checks' refresh; it becomes the interface part 4 reads.

**Part 2: keep the edits.**
- The workspace document keeps, until its next analysis, the collapsed edit (one range in the coordinates of the last analysed text, and the new length) and the moved line starts. Several batches collapse into one range. A full-text change, or a batch the collapse can't express, clears it and forces a full rebuild.
- The analysis takes the line starts instead of computing them. The line table counts UTF-16 and code-point offsets per line, keeping fixed-stride marks only inside long lines (over about 4 KB), so an edit only touches the lines it changed.

**Part 3: statement splice.**
- A new module owns the splice; the parser gains only a constructor that starts in a given state (prefixes and base in effect, graph block state, blank nodes allocated so far, statement index).
- Re-lex: start at the token before the one containing the edit (lexer lookahead: `1.` then `5`, prefixed names backing off a dot, `^^`, `{|`). Stop at the first new token at or past the edit's end whose start, shifted back by the length change, is an old token's start. The lexer keeps no state between tokens, so from there on the tokens are the old ones shifted. If they never meet, the window runs to the end of the file; that is correct, only not cheap.
- Re-parse: start one statement before the token window (error recovery crosses statement ends and can swallow the next statement). Parse statements until one ends past the lexer's meeting point and the next token starts an old statement, with the same parser state as before.
- Splice the tokens, statements, triples, refs, prefix uses, graphs, and each of the three error groups separately (lexer errors, then parser errors, then the N-Triples restrictions), so that the editor's cap on syntax errors picks the same errors. Shift every later span by the length change, every later triple's statement index by the change in statement count, and every later fresh blank node by the change in blank nodes allocated.
- Full-parse fallbacks, checked on both the old and new window: a directive (`@prefix`, `PREFIX`, `@base`, `BASE`, `VERSION`); a `_:label`; a TriG `{`, `}` or `GRAPH`; nesting near the parser's maximum; SPARQL and JSON-LD documents (always); a change to the external prefixes; an old parse with no statements; a window over about 25% of the file.
- The analysis maps (defs, refs by IRI, labels, descriptions, types, supers) are rebuilt in full at first. Making them per-IRI counts with statement-relative indexes is a later step, only if part 0's measurement shows it pays.
- Setting: a boolean in the server settings that turns the splice off (the full path stays the reference). The environment switch for the both-paths check is a separate, test-only knob.

**Part 4: diagnostics per statement.**
- Each check declares what it reads:
  - **statement-local:** syntax errors, undeclared prefixes (given unchanged prefixes), literal checks, `label-whitespace`;
  - **document-wide:** `duplicate-prefix`, `redefined-prefix`, unused prefixes, and per-subject checks such as `multiple-labels`;
  - **workspace-dependent:** unknown and vocabulary terms, `duplicate-label`, deprecations, the house-style checks.
- Statement-local results are cached per statement as spans, shifted with the statement, and recomputed only for the window. Per-subject checks recompute for the subjects in the window. Workspace-dependent results are reused unless they name an IRI or text key in the swap's change set from part 1, or the document's own window changed.
- Spans become LSP ranges only when answering. The composer in `diagnose::document` stays the one place that orders and filters diagnostics; the server, workspace diagnostics, `check` and the browser keep calling it.

**Unchanged contracts.** The `Parsed` structure, every feature that reads it, the LSP surface, the settings file format (apart from the one new setting), the command line and the browser engine.

## Testing Decisions

- **The seam is the workspace.** A test applies edits to a document through the workspace, as a client would, and compares what the workspace then gives (the parse, the analysis, the index snapshot and the composed diagnostics) with a fresh workspace opened on the final text. This is one seam for all four parts; tests don't reach into the splice module's internals.
- **Equality is field by field and includes errors.** rust-analyzer's reparse fuzzing skips comparing errors; this one must not. `Parsed` and its parts get equality for this (test-only if preferred). The index comparison extends the existing snapshot used by `incremental_updates_match_a_rebuild` in the knowledge tests.
- **Chaos.** `tests/chaos.rs` already mutates real files with cuts, stray quotes, unbalanced brackets, CRLF and lone CR, and multi-byte characters. It gains a mode that applies each mutation as an edit to the previous text, chains several edits before one analysis, and runs the oracle every iteration. New mutations aim at each hazard: insert or delete `"""`, `"`, `#`, `<`, `@prefix`, `_:b`, `{` and `}`, `.` after a number, and a newline before a column-0 subject. The run counts which path each iteration took and fails if the splice path's share drops below a floor, so the fallbacks can't quietly swallow every case. A chaos failure prints its replay line as today, and is fixed with a unit test on the reduced input.
- **Unit tests per hazard** in the parser's test module, each a small text and one edit: a directive edit, an opened and closed `"""`, a lookahead edit (`1.` then `5`), a broken statement swallowing the next, a `[ ]` added before later blank nodes, TriG braces, an N-Quads line edit, a multi-batch collapse, a full-text change.
- **Timing tests** (`timing_…`, run serially): one edit in the middle of a 2.4 MB file stays under a fixed budget, and N edits cost O(N × edit), not O(N × file).
- **Node tests** through `test/lsp-client.mjs`: a sequence of edits on a fixture gives the same published diagnostics as reopening it, for Turtle, TriG, N-Quads, SPARQL and JSON-LD (the last two prove the fallback).
- **Corpus:** every corpus file, opened and then edited at a few positions, matches a reopen; part 3's setting turns on only after this passes.
- **Benchmark:** `big-file` and `big-vocab` rows before and after each part, main against the branch on a quiet machine. Prior art: the label-lints close-out (`docs/tickets/label-lints/07-close-out.md`).

Budgets, as pass/fail lines:
- Results equal a full rebuild in every chaos iteration and on the corpus.
- `big-vocab` keystroke at most 8 ms after all four parts (17.4 ms on main today); `big-file` keystroke at most 8 ms (15.3 ms today).
- No other keystroke row of the benchmark gets more than 5% slower; the initial index stays within 10%.
- Memory: the big-file RSS rows stay within 10%.

## Out of Scope

- Replacing the parser with rowan or tree-sitter. The research found both would mean rewriting every feature that reads the flat `Parsed` vectors, and losing the resilient recovery and its messages.
- Incremental SPARQL and JSON-LD parsing: they always take the full path.
- Making the command line or the browser engine incremental.
- Relative offsets in spans (Roslyn, rowan): it would remove the tail shift but change every consumer of spans.
- Incremental SHACL validation; it runs on save.
- Incremental workspace indexing at startup and on file-watcher events (those are whole-file changes).
- Changing the LSP diagnostics protocol (the answer is still the full list per document).

## Further Notes

- Source: `docs/research/incremental-reparse.md`, which cites every external claim at a pinned commit, and the profile of 2026-10-29.
- The estimates are from profile shares, not measurements: parser and line table alone about 17 to 12–13 ms on the big file; with the index delta and the diagnostics cache about 5 to 7 ms. Normal files gain at most about a millisecond.
- The main risk is a wrong splice giving stale or misplaced diagnostics, which is worse than a slow keystroke. Strict fallbacks and the oracle contain it; a fallback costs only speed.
- The blank-node rule may fall back often on files that use `_:` labels heavily; count how many corpus files do during part 0.
- The label checks in PR #47 are the first beneficiary: with parts 1 and 4, they look only at the labels that changed.
