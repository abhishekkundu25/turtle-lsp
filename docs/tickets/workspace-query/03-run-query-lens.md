# 03: Run query lens and command

**What to build:** "▶ Run query" code lens on a SPARQL document that parses as a query; command `turtle.runQuery` (uri), advertised. The server snapshots the workspace analyses (open documents included) and runs on a background thread with `query.timeoutSeconds` (default 30) and `query.maxRows` (default 1000); writes Markdown to `<cache dir>/query-results/<stem>.md` and opens it with `window/showDocument` (takeFocus false) when supported, else `window/showMessage` with the summary; the response returns the summary. VS Code contributes the two settings.

**Blocked by:** 01; lsp-capability 02 (commands) and 04 (lens module).

**Status:** ready-for-agent

- [ ] Node LSP tests: lens present for a query, absent for an update; command writes the file and requests showDocument; fallback message; unsaved edits seen; time limit; a keystroke answered while a slow query runs.
- [ ] Keystroke rows of bench/perf.mjs unchanged.
