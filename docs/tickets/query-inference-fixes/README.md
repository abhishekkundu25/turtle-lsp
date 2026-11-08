# Tickets: queries over inferred triples, review fixes

Spec: [docs/specs/query-inference-fixes.md](../../specs/query-inference-fixes.md).
Lock files in **bold**. Integration branch: `feat/turbowl-reasoning`.

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 01 | [Cap and filter the inferred triples, limits during conversion](01-cap-filter.md) | open | — | `src/query/mod.rs`, `src/query/tests.rs`, `src/reasoning/infer.rs`, `src/rdf/graph.rs`, `src/cli/query.rs`, **`src/extensions/run_query.rs`** (the note's match only), **`src/extensions/query.rs`** (the `inferred` member's match only), `web/engine/src/api.rs`, `test/query-cli.test.mjs` |
| 02 | [`files`: every other document reasons](02-files.md) | open | — | **`src/extensions/query.rs`** (`sources` and its call only), `test/query-request.test.mjs` |
| 03 | [A `query.*` change re-runs nothing](03-settings.md) | open | — | `src/server.rs`, `src/server/*.rs` as needed, a server unit test |
| 04 | [Browser: Cancel, watchdog, crash hint](04-browser.md) | open | — | `web/app/query-pane.js`, `web/app/worker.js`, `web/app/query.css`, `web/test/editor-query.test.mjs` |
| 05 | Close-out and arena review | open | 01-04 | `CHANGELOG.md`, `docs/specs/query-inference-fixes.md`, this README |

## How the lock files stay apart

- **`src/extensions/query.rs`**: 01 edits only the match on `Inferred` that
  writes the `inferred` member; 02 edits only `sources` and the code that calls
  it. Separate hunks.
- **`src/extensions/run_query.rs`**: only 01 (the note's match).

## Waves

1. **01, 02, 03, 04** together.
2. **05**.

## Departures

## Follow-ups

- TurbOWL drops a literal's base direction (`"x"@en--rtl` merged with
  `"x"@en`) in its dictionary (`crates/turbowl/src/term.rs`): wrong query rows
  and false `owl:sameAs`; hover reasoning likely affected (owner: later).
