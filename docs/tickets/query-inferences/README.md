# Tickets: SPARQL queries over inferred triples

Spec: [docs/specs/query-inferences.md](../../specs/query-inferences.md).
Lock files are in **bold**. Integration branch: `feat/turbowl-reasoning`
(owner's choice).

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 01 | [The query core reasons over its snapshot](01-core.md) | open | — | `src/query/mod.rs`, `src/query/tests.rs`, `src/rdf/graph.rs`, `src/rdf/dataset.rs`, `src/reasoning/` (one new function and its module line), **`src/settings.rs`** (the `query.reasoning` field and key), **`src/batch.rs`** (passes the setting through), **`src/extensions/query.rs`**, **`src/extensions/run_query.rs`** (only to compile: pass `off`), `src/lib.rs` (public-interface doctest, if `query`'s public surface changes) |
| 02 | [Command line: `--reasoning`](02-cli.md) | open | 01 | `src/cli/args.rs`, `src/cli/query.rs`, `test/query-cli.test.mjs` |
| 03 | [Editors: `turtle/query` and "▶ Run query"](03-editors.md) | open | 01 | `src/extensions/query.rs`, `src/extensions/run_query.rs`, `editors/vscode/package.json`, `editors/vscode/schemas/turtle-lsp.schema.json`, `editors/vscode/test/*.test.mjs` (manifest, schema), `test/query-request.test.mjs`, `test/query-lsp.test.mjs` |
| 04 | [Browser: the query pane's Inferred choice](04-browser.md) | open | 01 | `web/engine/src/api.rs`, `web/app/query-pane.js`, `web/app/worker.js`, `web/app/settings.js`, `web/app/query.css`, `web/test/editor-query.test.mjs`, `web/test/engine.test.mjs`, `web/test/editor-settings.test.mjs` |
| 05 | [Close-out](05-close-out.md) | open | 02, 03, 04 | `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/specs/query-inferences.md` (status line), this README |

## How the lock files stay apart

- **`src/settings.rs`**, **`src/batch.rs`**: only 01. It adds the
  `query.reasoning` field (default `off`, file-settable, a unit test) and
  passes it from `Settings` into the core, so 02 and 04 only set the field
  and 03 only reads it.
- **`src/extensions/query.rs`**, **`src/extensions/run_query.rs`**: 01 makes
  the smallest change that compiles (pass `off`, ignore the note); 03 owns
  every other hunk, after 01 lands.
- 02, 03 and 04 share no file.

## Waves

1. **01**.
2. **02, 03, 04** together.
3. **05**.

## Departures

- The command line and the browser query worker load no imports for a
  query (the batch query reads the files given), so their inferences use
  those files only; the spec's imports story holds for the editors.
- The browser editor's `query.reasoning` starts at `default`, as its
  `turtle.reasoning.rules` does (owner's choice, 2026-11-08).
