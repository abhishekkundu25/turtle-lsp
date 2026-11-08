# 02: VS Code: the Query Results panel

**What to build:** spec Solution, "Client" and "Webview" decisions, against
the JSON contract in the spec.

**Blocked by:** none. **Status:** done (a5e0ff5, review fixes 13ae677)

- [ ] `editors/vscode/test/query-results.test.mjs` (node): the table model for SELECT, ASK, CONSTRUCT and error answers; sort by a column; IRI shortening with prefixes; CSV quoting; JSON export; the header line and the inferred note.
- [ ] Extension-host test in `suite.cjs`: running the lens on a `.rq` opens one Query Results panel; a rerun reuses it. (If the server half is not landed, stub the command's answer.)
- [ ] `npm run build`, `npm run typecheck`, `(cd editors/vscode && timeout 120 node --test test/*.test.mjs)`, and the extension-host run if it fits the time budget.
