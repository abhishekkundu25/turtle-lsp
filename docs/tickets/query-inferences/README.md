# Tickets: SPARQL queries over inferred triples

Spec: [docs/specs/query-inferences.md](../../specs/query-inferences.md).
Lock files are in **bold**. Integration branch: `feat/turbowl-reasoning`
(owner's choice).

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 01 | [The query core reasons over its snapshot](01-core.md) | done (8c9e6a6) | — | `src/query/mod.rs`, `src/query/tests.rs`, `src/rdf/graph.rs`, `src/rdf/dataset.rs`, `src/reasoning/` (one new function and its module line), **`src/settings.rs`** (the `query.reasoning` field and key), **`src/batch.rs`** (passes the setting through), **`src/extensions/query.rs`**, **`src/extensions/run_query.rs`** (only to compile: pass `off`), `src/lib.rs` (public-interface doctest, if `query`'s public surface changes) |
| 02 | [Command line: `--reasoning`](02-cli.md) | done (836e569) | 01 | `src/cli/args.rs`, `src/cli/query.rs`, `test/query-cli.test.mjs` |
| 03 | [Editors: `turtle/query` and "▶ Run query"](03-editors.md) | done (c6bcc4f) | 01 | `src/extensions/query.rs`, `src/extensions/run_query.rs`, `editors/vscode/package.json`, `editors/vscode/schemas/turtle-lsp.schema.json`, `editors/vscode/test/*.test.mjs` (manifest, schema), `test/query-request.test.mjs`, `test/query-lsp.test.mjs` |
| 04 | [Browser: the query pane's Inferred choice](04-browser.md) | done (c46cd32) | 01 | `web/engine/src/api.rs`, `web/app/query-pane.js`, `web/app/worker.js`, `web/app/settings.js`, `web/app/query.css`, `web/test/editor-query.test.mjs`, `web/test/engine.test.mjs`, `web/test/editor-settings.test.mjs` |
| 05 | [Close-out](05-close-out.md) | done | 02, 03, 04 | `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/specs/query-inferences.md` (status line), this README |

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
- 01: `query::run` takes `&[&Arc<Analysis>]` (the reasoning base needs
  `Arc`s) and an `Inference` input, and returns an `Outcome` (the `Answer`
  and the `Inferred` note). `settings::Reasoning` is now `pub`, as the
  public note carries it. `rdf::Graph::with_triples` takes the inferred
  triples, their blank nodes renumbered from the base's (URI order) to the
  graph's (given order).
- 02: without `--reasoning`, the project's `query.reasoning` applies
  (`turtle-lsp.json`), then `off`; the flag overrides it. The usage text
  lives in `src/cli/mod.rs`, also edited.
- 03: the untrusted-workspace description now names the restricted query
  limits instead of `turtle.query.*`; `turtle.query.reasoning` is not
  restricted, as `turtle.reasoning.rules` is not.
- 04: the browser's request choice is a settings layer
  (`Settings::from_layers`), since the field is `pub(crate)`; the Inferred
  control is a select beside Run, bound to the setting both ways. Changing
  it does not rerun the query.
- Close-out: the browser sample's `reasoning/` folder gains two queries
  (`04-certified-parts.rq`, `05-merged-names.rq`) that answer only with
  inferred triples, tested in `web/test/engine.test.mjs`.

## Follow-ups

- `Inferred::Added` carries `settings::Reasoning`, whose `Off` it never
  holds; a rules type without `Off` would drop the dead arm in each
  renderer.
- A triple term with a blank node inside, in an inferred triple, from any
  document but the first, is a different term in the query graph from the
  written copy: a join through it can miss.
- The Inferred note sits outside the pane's `role=status`, so screen
  readers do not announce it.
