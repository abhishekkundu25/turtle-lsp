# 06: Diagnostics per statement, and reuse by the change set

**What to build:** Part 4 of the spec.

- Each check declares what it reads: statement-local, document-wide or
  workspace-dependent (spec, Part 4 list). Model it as one table or enum
  next to the check codes, so a new check cannot be added without a class.
- Statement-local results are kept per statement as spans, shifted with
  the statement, and recomputed only for the spliced window. Per-subject
  checks recompute for the subjects in the window. Workspace-dependent
  results are reused unless they name an IRI or text key in the index's
  change set (02), or the document's own window changed.
- The composer in `diagnose::document` stays the one place that orders and
  filters; `check`, the browser and workspace diagnostics call it as
  before and take the full path.
- Any full parse (fallback) drops the cache.
- The setting 05 adds that turns the splice off turns this cache off too:
  with it off, the parse and the checks both take the full path.

**Blocked by:** 02, 05.

**Status:** open

- [ ] The oracle (chaos edit mode) passes at `CHAOS_ITERS=20000`: the
  composed diagnostics equal a fresh workspace's, including cross-file
  label results after another document changes.
- [ ] Unit tests: an edit in one statement recomputes only that
  statement's local checks (counted, not timed); a label edit that creates
  a cross-file duplicate refreshes the other document; a prefix edit
  (fallback) recomputes everything.
- [ ] `test/label-lints.test.mjs`, `test/lint.test.mjs` and the CLI tests
  pass.
