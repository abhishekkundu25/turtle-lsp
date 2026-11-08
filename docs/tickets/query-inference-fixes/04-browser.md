# 04: Browser: Cancel, watchdog, crash hint

**What to build:** spec item 1 (browser part). While a query runs, the Run
button becomes Cancel (or a Cancel beside it), which ends the query worker;
a watchdog ends it at the query's `timeoutSeconds` plus 5 s; either way the
pane says so, and the next run starts a fresh worker. An engine crash
during a run with Inferred on says to try Inferred off.

**Blocked by:** none. **Status:** open

- [ ] `web/test/editor-query.test.mjs`: Cancel ends a slow query (a workspace whose query takes seconds) and the next query runs; a crash message with Inferred on names Inferred off (simulate with a worker that throws if a real crash is impractical).
- [ ] The existing web query and editor tests pass.
