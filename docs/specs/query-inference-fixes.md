# Queries over inferred triples: review fixes

Status: done. Tickets:
[../tickets/query-inference-fixes/](../tickets/query-inference-fixes/README.md).
Fixes findings of a three-reviewer bug hunt of
[query-inferences.md](query-inferences.md) (2026-11-08).
The import, filter and `files` decisions are superseded by [query-inference-simplify.md](query-inference-simplify.md).

## Problem Statement

Queries with inferred triples have four defects found in review:

1. A small workspace makes one query use gigabytes. `turtle.reasoning.maxTriples`
   bounds the triples the files hold, not what TurbOWL infers: an 8,000-link
   chain on one transitive property infers about 32 million triples. A query
   then takes about 4 GB natively and runs well past its time limit (6.5 s for
   a 3 s limit), because turning the inferred triples into query data and
   building the query graph check no limit. In the browser the worker holds
   3.4 GB until the page reloads, or crashes with a raw Rust panic, and every
   later query crashes the same way. A cloned repository can set this up, as
   its `turtle-lsp.json` may turn `query.reasoning` on.
2. Inferences drawn only from imports leak into results: the imported
   vocabulary's own triples are left out of the data, but what TurbOWL infers
   among them is added, so a query sees `A ⊑ C` without `A ⊑ B` and `B ⊑ C`,
   and a large import floods queries.
3. `turtle/query` with `files` reasons with the wrong documents: a workspace
   ontology that a queried file imports is dropped (it is a workspace
   document, not an external import), while external imports only other files
   have are kept.
4. Changing `query.reasoning` makes the server redo all its reasoning and SHACL
   validation, though the setting only affects queries: 1 to 5 s without hover
   and diagnostics in the browser per change.

## Solution

1. Inferred triples are capped at `turtle.reasoning.maxTriples`. Past the cap
   the query runs without them and says so ("TurbOWL inferred more than M
   triples, over the reasoning limit; they are left out"). Turning inferred
   triples into query data and building the query graph check the query's time
   limit and cancel, so a query stops near its limit. In the browser, the query
   pane gets a Cancel button while a query runs and a watchdog that ends the
   query worker a few seconds past the time limit, and an engine crash during
   a query with inferred triples says to try Inferred off.
2. A query keeps only inferred triples whose subject or object is a term of the
   queried documents. Imported axioms still drive what is inferred about the
   user's data; the imported vocabulary's own closure stays out.
3. Reasoning for a query reads every document of the workspace that is not
   queried, and the external imports, as reasoning-only input; with item 2,
   only inferences about the queried documents' terms reach the results.
4. A settings change that touches only `query.*` keys schedules no reasoning,
   SHACL validation or re-check.

## User Stories

1. As a user, I want a query over a workspace whose inferences explode to finish near its time limit, so that the editor stays usable.
2. As a user, I want such a query to tell me the inferred triples were left out and why, so that I know the results are only what the files say.
3. As a user, I want a cloned repository's `turtle-lsp.json` unable to make my queries take gigabytes, so that opening a project is safe.
4. As a browser-editor user, I want to cancel a running query, so that a slow one never holds the pane.
5. As a browser-editor user, I want a runaway query ended a few seconds past its limit, and the worker's memory released, so that the tab recovers without a reload.
6. As a browser-editor user, I want an engine crash during a query with inferred triples to suggest Inferred off, so that I can get past it.
7. As a user, I want query results with inferred triples to be about my files' terms, not the closure of an imported vocabulary, so that results stay small and coherent.
8. As a tool author querying some `files`, I want the ontology they import from the workspace to count in reasoning, so that `files` does not change what is inferred about them.
9. As a tool author querying some `files`, I want only inferences about those files' terms in the answer, so that other files' data does not leak in.
10. As a user, I want flipping the query's Inferred choice to leave hover and diagnostics responsive, so that comparing results costs nothing else.

## Implementation Decisions

- **The cap is a new note.** `query::Inferred` gains the case "too many": TurbOWL inferred more than the limit (the limit is `Inference::max_triples`, which is `turtle.reasoning.maxTriples`). The answer carries no inferred triples. Every renderer matches it: the CLI's stderr line, "▶ Run query"'s line, `turtle/query`'s `inferred` member (`{"skipped": {"inferred": true, "limit": m}}` or a shape that keeps today's `skipped` form for the input case), the browser engine's member.
- **Counting stops early.** The conversion from TurbOWL's inferred triples stops as soon as the count passes the cap, and checks the query's watch (time limit, cancel) every 64k triples; the graph build checks the clock (`check_now`). TurbOWL itself has no output cap: a closure too large for memory inside TurbOWL is a TurbOWL follow-up.
- **The filter is in the reasoning-to-query step** (`reasoning::infer`): an inferred triple is kept when its subject or its object is a term of a queried document (an IRI, a literal, or a blank node of a queried document). The cap counts kept triples.
- **Reasoning input for `files`.** `turtle/query` passes every non-queried workspace document and every external import as reasoning-only input. "▶ Run query" queries every document, so it is unchanged.
- **Settings.** The server's settings-change handling compares the reasoning-relevant and validation-relevant settings, and a change to `query.*` only applies nothing beyond storing the settings.
- **Browser.** The query pane's watchdog uses the query's `timeoutSeconds` plus a grace of 5 s; ending the worker fails its pending queries with a message and starts a fresh worker on the next run, as a crash does today.

## Testing Decisions

- Test at the query entry point (`src/query/tests.rs`) for the cap, the filter, the time limit during conversion; over the protocol for `files` (`test/query-request.test.mjs`) and settings (a server unit test or `test/reasoning.test.mjs`); in Chrome for the pane (`web/test/editor-query.test.mjs`).
- Prior art: the review's repros under the scratchpad (`review-a`, `review-b`, `review-c`): the transitive chain, the `owl:sameAs` chain, the import-only subclass chain, the `files` + workspace ontology case, the settings flip.
- Light on timing (owner's choice): bounds asserted loosely (well under the unbounded time), timing tests named `timing_`.

## Out of Scope

- TurbOWL dropping a literal's base direction (`"x"@en--rtl` read as `"x"@en`): logged as a TurbOWL follow-up (owner's choice, 2026-11-08).
- An output cap inside TurbOWL, and cancelling background (hover) reasoning.
- The review's low findings (plurals, schema text, relative `base`, empty query, narrow header, stale note while running, sample warnings, stale comments, the `maxTriples: 0` mismatch, the double-counted import): a later fix ticket.

## Principles

- **fix-root-causes:** the blow-up is fixed where the size is decided (the cap at the conversion), not only by a browser watchdog.
- **model-the-domain, type-system-discipline:** "too many" is a case of the note, matched exhaustively by every renderer.
- **boundary-discipline:** the `files` split into queried and reasoning-only documents happens once, at the request boundary.
- **separate-before-serializing-shared-state:** each ticket owns separate files or separate hunks; see the README.
