# Tickets: query what the reasoner saw

Spec: [docs/specs/query-inference-simplify.md](../../specs/query-inference-simplify.md).
Integration branch: `feat/turbowl-reasoning`.

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 01 | [One document list, no filter, no renumbering](01-core.md) | done (51487e2) | — | `src/query/mod.rs`, `src/query/tests.rs`, `src/reasoning/infer.rs`, `src/reasoning/mod.rs`, `src/extensions/query.rs`, `src/extensions/run_query.rs`, `src/cli/query.rs`, `src/batch.rs`, `test/query-request.test.mjs`, `test/query-lsp.test.mjs`, `test/query-cli.test.mjs` |
| 02 | [Browser: queued cancel, hint, start errors, one time limit](02-browser.md) | done (a404593) | — | `web/app/query-pane.js`, `web/app/worker.js`, `web/test/editor-query.test.mjs`, `web/test/app-modules.test.mjs` |
| 03 | Close-out: docs, full validation, benchmark | done (this close-out) | 01, 02 | `CHANGELOG.md`, `README.md`, `ARCHITECTURE.md`, the three query specs, this README |

## Waves

1. **01, 02** together (disjoint files).
2. **03**.

## Departures

- The browser slice briefly used `git stash` in its worktree.
- Imports are not given `Source::External`; documents use plain URI order.
- The skipped sentence still says "the workspace has N triples", even for
  a `files` query.
