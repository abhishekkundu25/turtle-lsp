# Tickets: query results in a panel (VS Code)

Spec: [docs/specs/query-results-panel.md](../../specs/query-results-panel.md).
Branch: `feat/query-results-panel`.

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 01 | [Server: Run query answers a result object for a capable client](01-server.md) | open | — | `src/extensions/run_query.rs`, `src/extensions/query.rs` (share the JSON helpers), `src/extensions/mod.rs` if needed, `test/query-lsp.test.mjs` |
| 02 | [VS Code: the Query Results panel](02-panel.md) | open | — (the JSON contract is in the spec) | `editors/vscode/src/query-results.ts`, `editors/vscode/src/query-results-view.js`, `editors/vscode/src/query-results.css`, `editors/vscode/src/activate.ts`, `editors/vscode/package.json` (build script only), `editors/vscode/test/query-results.test.mjs`, `editors/vscode/test/suite.cjs` |
| 03 | Close-out: docs, full validation, review | open | 01, 02 | `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, this README, the spec's status |

## Waves

1. **01, 02** together (disjoint files, the spec's JSON contract between them).
2. **03**.

## Departures

- Review fixes: Run query's answers go out as JSON text through
  `Client::respond`, widened from `pub(super)` to `pub(crate)` like
  `Client::send`. `$/cancelRequest` keeps the standard RequestCanceled
  error (the one exception to the cancelled object). The panel's Inferred
  choice starts at `turtle.query.reasoning` even before a result arrives.
  The extension-host check uses the real server only, with two query
  documents.
