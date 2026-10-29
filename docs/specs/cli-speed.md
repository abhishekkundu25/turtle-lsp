# CLI speed: do only the work each command reports

Research: docs/research/cli-speed.md. Built on `feat/incremental-keystroke`
(PR #54), because it edits the same batch and workspace code.

## Problem Statement

`turtle-lsp check` and `turtle-lsp format --check` run one shared pipeline,
`batch::analyse_on_disk`. It computes, for every file, the diagnostics and
the formatter's verdict, and it resolves imports and builds the knowledge
index. `check` never reads the verdict and `format` never reads the
diagnostics. On a 2.5 MB file and on the 202-file bench workspace the
verdict alone is 45 to 47% of `check`. The column conversion builds a
second line index per file that the analysis already has.

A user running `check` in CI or a pre-commit hook waits for work whose
result is thrown away.

## Solution

Each command asks the batch pipeline for what it reports:

- `check` asks for diagnostics (and fixes for `json` and `sarif`), not the
  formatter verdict.
- `format` asks for the verdict, not diagnostics, import resolution or the
  knowledge index' diagnostics inputs.
- The browser engine keeps asking for both, as today.
- Code-point columns are converted with each analysis's own line index.

No reported result changes: every diagnostic, fix, verdict, exit code and
byte of output is the same as before, on every input.

## User Stories

1. As a user running `turtle-lsp check` on a big file, I want it to skip the formatter, so that it returns in about half the time.
2. As a user running `format --check` in CI, I want it to skip diagnostics and import resolution, so that it costs only the parse and the formatter.
3. As a user running `format` (in place), I want the same savings, so that formatting a tree of files is quick.
4. As a user with `owl:imports`, I want `format` not to load or fetch imports at all, so that formatting never waits on the import cache.
5. As a user with a `prefixes.ttl`, I want `format` to keep using the project prefixes for headerless files, so that its verdict does not change.
6. As a user of `check --format json` or `sarif`, I want the same problems, fixes and order as before, so that my tooling's baseline does not churn.
7. As a user of `check --statistics`, I want the same counts, so that dashboards stay comparable.
8. As a user of `check --select` / `--ignore`, I want the same filtering, so that my scripts behave the same.
9. As a user of `check --no-shacl`, I want it as fast as before or faster, so that the fast path stays fast.
10. As a user of `check --fix`, I want `fix_all` to behave exactly as before, so that fixes are not affected by the speed work.
11. As a user whose files have syntax errors, I want `format` to refuse them with the same message, so that the error count stays right.
12. As a user formatting a SPARQL file with errors, I want the same "the query has N errors" message, so that it still counts errors without the full diagnostics run.
13. As a user of the browser playground, I want diagnostics and the formatter verdict as today, so that the toolbox shows both.
14. As a user with code-point columns (the CLI default), I want the same line and column numbers, so that editors jump to the right place.
15. As a user with non-ASCII text, I want code-point columns still right after the line index is reused, so that columns past a multi-byte character are correct.
16. As a user with CRLF files, I want the same positions, so that Windows checkouts report the same places.
17. As a user checking one 2.5 MB file, I want peak memory no higher than today, so that the big-file memory budget holds.
18. As a user of the language server, I want no change to editing latency, so that the CLI work does not touch the keystroke path.
19. As a maintainer, I want one option per piece of work in `batch::Options`, so that each caller states what it needs.
20. As a maintainer, I want a test that the results with the verdict off equal the results with it on (minus the verdict), so that skipping work cannot change the diagnostics.
21. As a maintainer, I want an end-to-end comparison of `check --format json` and `format --check` output against the base build on the corpus and the bench workspace, so that "no change to any reported result" is checked, not assumed.
22. As a maintainer, I want the bench's `check-cli` and `big-file-peak` rows recorded against main, so that the gain is measured.
23. As a maintainer of the browser build, I want the engine to compile and its tests to pass unchanged, so that the browser build still works.

## Implementation Decisions

- **Options.** `batch::Options` gains two flags: `verdict` (compute the
  formatter's verdict; on by default) and `diagnostics` (compute
  diagnostics, fixes, SHACL and import resolution; on by default). The
  default stays "everything", so the browser engine and the tests keep their
  behaviour. `check` sets `verdict: false`; `format` sets
  `diagnostics: false` (and `shacl: false`, `fixes: false` as today). A
  `FileReport` built without a verdict has `format: None`; one built without
  diagnostics has empty `problems`.
- **Format without diagnostics.** With `diagnostics: false` the batch input
  does not resolve imports and does not compute diagnostics. The project
  prefixes (`prefixes.ttl`, the `prefixes` setting) still reach each
  document, because the verdict depends on them. A SPARQL file with errors
  still gets "the query has N errors": the count comes from the query's own
  syntax errors without running the full diagnostics, and must equal what
  the old path printed (the old path counted error-severity diagnostics with
  an empty SHACL list; a test pins the equality on the fixtures). Whether
  the knowledge index merge can be skipped too is the slice's call: skip it
  only if the documents are still analysed in parallel and the verdicts are
  identical.
- **Tokens for the verdict.** The formatter needs tokens, which the batch
  workspace drops (`keep_tokens` off), so the verdict re-lexes. Keeping
  tokens in `format` mode avoids that re-lex but raises peak memory; keep
  them only if the `big-file-peak` budget (64 bytes per input byte) still
  holds with margin, else keep the re-lex.
- **Column conversion.** Code-point conversion uses the line index of the
  file's `Analysis` (the same text) instead of building a new one. Edits in
  fixes that target another file convert against that file's analysis. A
  file with no analysis falls back to a fresh line index.
- No new public items. Anything new is `pub(crate)`; the two `Options`
  fields are on an already public struct.

## Testing Decisions

- A good test checks what a caller sees (the `Report`, the CLI's stdout and
  exit code, the SHACL `ByUri` map), not how it was computed.
- `src/batch.rs` unit tests (prior art: `columns_count_as_asked`,
  `fixes_are_found_only_when_asked`, `the_formatter_gives_its_verdict`,
  `code_point_columns_follow_the_line_index`): verdict off gives the same
  problems and no verdict; diagnostics off gives the same verdicts and no
  problems; code points from the analysis's line index equal a fresh one on
  non-ASCII and CRLF text.
- Node CLI tests (prior art: `test/cli.test.mjs`, `test/format-cli*.test.mjs`
  if present): `check` and `format --check` outputs unchanged; these
  already exist and must stay green unchanged.
- `tests/diagnostics_agree.rs` stays green: the browser path still equals
  the server.
- End-to-end equality (close-out): the base build and the branch build run
  `check --format json`, `check` (concise), `check --statistics` and
  `format --check` on `test/corpus`, `test/fixtures` and the bench
  `check-cli` workspace; the outputs and exit codes are byte-identical.

## Budgets (pass/fail)

- [ ] Output of every command in the end-to-end comparison is
  byte-identical to the base build, exit codes included.
- [ ] `check-cli` bench row at least 25% faster than main.
- [ ] `check` on `test/corpus` not slower than the base build.
- [ ] `format --check` on the 2.5 MB file at least 15% faster than the base
  build.
- [ ] `big-file-peak`: within the 64 bytes-per-byte budget, and no more
  than 5% above main for either command.
- [ ] Keystroke and index rows of the bench do not move (more than 5%).

## Out of Scope

- Lexer tuning (byte-class tables, memchr), the language-tag cache and
  zero-copy literals (research options 6 and 7).
- The parallel per-file loop and the SHACL focus-node lookup and parallel
  datasets (options 4 and 5): small gains for their code today; revisit
  when a user's SHACL check is slow.
- Parallel parsing of one N-Triples, N-Quads, Turtle or TriG file (options 8
  and 9).
- The unexplained gap on `dbpedia-ontology.nt` (stage sum 32 ms, command
  85 ms); profile it later.
- Any change to the language server's paths.
- `check --fix` (`fix_all`) speed.

## Further Notes

The research figures were taken on a loaded machine and are indicative; the
budgets above are checked on a quiet one at close-out.
