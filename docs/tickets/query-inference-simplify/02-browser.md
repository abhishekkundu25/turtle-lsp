# 02: Browser: queued cancel, hint, start errors, one time limit

**What to build:** spec Implementation Decisions, Browser.

**Blocked by:** none. **Status:** open

- [ ] `web/test/editor-query.test.mjs` (or `app-modules` with a fake worker): cancelling a queued query leaves the running one running; the hint appears only on the query the engine was running; a worker that cannot start shows an error in the pane; the watchdog uses the limit the pane sends.
- [ ] The existing web query, editor and settings tests pass.
