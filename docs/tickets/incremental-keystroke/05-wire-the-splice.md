# 05: Wire the splice

**What to build:** the rest of Part 3.

- The analysis of an open document with a kept edit (03) and a previous
  analysis calls the splice (04) and rebuilds the analysis maps from the
  new `Parsed`; a fallback, or no kept edit, runs the full parse. The line
  table comes from 03.
- A server setting turns the splice off (`settings.rs`, the VS Code
  setting in `editors/vscode/package.json`, the settings file). On by
  default only once chaos and the corpus pass (07 checks the corpus; if it
  is not green, ship it off).
- A test-only environment switch (for example
  `TURTLE_LSP_CHECK_INCREMENTAL=1`) that makes the server run both paths
  and log the first difference through the oracle (01).
- `tests/chaos.rs` edit mode records which path each analysis took (splice
  or which fallback, from 04's enum, exposed through the oracle) and fails
  when the splice share drops below a floor (set it from a 20,000-iteration
  run, with margin).
- `test/incremental.test.mjs` through `test/lsp-client.mjs`: a sequence of
  edits on a fixture gives the same published diagnostics as reopening it,
  for Turtle, TriG, N-Quads, SPARQL and JSON-LD.

**Blocked by:** 01, 03, 04.

**Status:** done

- [x] Chaos passes at `CHAOS_ITERS=20000` with the splice on, with the
  path counts printed and the floor checked.
- [x] `test/incremental.test.mjs`, `test/settings-file.test.mjs` and the
  VS Code tests pass; the setting off gives the full path (a test checks
  the path counter, not timing).
- [x] A `timing_` test: one edit in the middle of a 2.4 MB file's analysis
  is under a fixed budget, and N edits cost O(N × edit).
