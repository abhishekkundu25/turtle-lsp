# 01: Server: Run query answers a result object for a capable client

**What to build:** spec Implementation Decisions "Server seam" and
"Per-run reasoning".

**Blocked by:** none. **Status:** done (d32ce2e)

- [ ] `test/query-lsp.test.mjs`: with `experimental["turtle/queryResults"]`, `executeCommand turtle.runQuery` answers the result object (SELECT with prefixes and inferred member; ASK; CONSTRUCT; a syntax error with line and column) and writes no file and sends no `window/showDocument`; with `{"reasoning": "default"}` as the second argument the answer has the inferred row and member though the setting is off; a bad reasoning value is InvalidParams; without the capability, today's Markdown document.
- [ ] fmt, clippy, lib unit tests, `test/query-lsp.test.mjs`, `test/query-request.test.mjs`.
