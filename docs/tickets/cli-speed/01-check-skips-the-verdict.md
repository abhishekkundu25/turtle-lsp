# 01: `check` skips the formatter verdict; columns from the analysis's line index

**What to build:**

- `batch::Options` gains `verdict: bool` (default true). With it off,
  `analyse_in` does not call `Input::format`; `FileReport::format` is None.
- `cli::check` sets `verdict: false`. The browser engine
  (`web/engine/src/api.rs`) and every test keep the default.
- Code-point conversion (`to_code_points`, `LineIndexes`) uses the line
  index of each file's `Analysis` (or `SparqlAnalysis`) when the workspace
  has one, and builds one only for a file without an analysis.

**Blocked by:** none.

**Status:** todo

- [ ] Unit test: over files with problems and fixes, the report with
  `verdict: false` has the same problems and fixes as with `verdict: true`,
  and every `format` is None.
- [ ] Unit test: code-point positions from the analysis's line index equal
  those from a fresh `LineIndex` on non-ASCII and CRLF text, for diagnostics
  and for fix edits that target another file.
- [ ] `test/cli.test.mjs` and the other check CLI tests green, unchanged.
- [ ] `tests/diagnostics_agree.rs` and the browser engine tests green.
