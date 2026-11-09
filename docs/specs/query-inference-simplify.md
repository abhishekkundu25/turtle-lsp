# Queries over inferred triples: query what the reasoner saw

Status: done. Tickets:
[../tickets/query-inference-simplify/](../tickets/query-inference-simplify/README.md).
Supersedes the import and `files` decisions of
[query-inferences.md](query-inferences.md) and
[query-inference-fixes.md](query-inference-fixes.md) (owner's choice,
2026-11-09).

## Problem Statement

A query with inferred triples keeps reasoning input apart from query data:
imports, and with `files` the other documents, feed TurbOWL but are not
queried, and a filter decides which inferences are "about the user's terms".
That split is the cause of most defects found in two reviews: conclusions
without their premises (`A ⊑ C` without `A ⊑ B`), a filter that leaks through
a shared class or `owl:Thing` and drops properties used only as predicates, a
`files` query skipped because the whole workspace counts toward the limit,
and an open question about which documents' data may drive rows. No other
tool (GraphDB, Stardog, Jena, RDF4J, Virtuoso) filters inferences this way:
with reasoning on, a query sees everything that was loaded plus everything
inferred.

## Solution

With reasoning on, a query runs over everything the reasoner read and
everything it inferred: the queried documents, the imports' own triples, and
TurbOWL's inferred triples, with no filter. With `files`, the queried and
reasoned documents are those files plus the workspace's external imports;
other workspace documents take no part. The cap on inferred triples, the time
checks and the browser's Cancel and watchdog stay. Off is unchanged.

## User Stories

1. As an ontology author, I want a query with reasoning to see the imported ontology's triples, so that every inferred fact comes with the facts it rests on.
2. As an ontology author, I want no inference hidden because of which terms it mentions, so that properties, classes and literals are treated alike.
3. As a tool author querying some `files`, I want reasoning and querying to read the same documents (those files and the imports), so that the answer is predictable and other files never drive rows.
4. As a tool author, I want a `files` query sized by what it reads, so that a small query in a large workspace is not skipped.
5. As a user, I want the same results as a reasoner that materialised the closure, so that I can trust query reasoning.
6. As a user with reasoning off, I want every query unchanged, so that nothing moves for me.
7. As a user, I want the inferred-triples note worded the same on the command line and in "▶ Run query", so that the two never disagree.
8. As a browser-editor user, I want cancelling a waiting query to leave another group's running query alone.
9. As a browser-editor user, I want the "try Inferred off" hint only when reasoning made the engine fail.
10. As a browser-editor user, I want an error shown if the query engine cannot start.
11. As a browser-editor user, I want the watchdog and the engine to use the same time limit.

## Implementation Decisions

- **One document list.** `query::Inference` no longer carries reasoning-only documents: the query's documents are what it queries and what TurbOWL reads. Callers add the imports to the queried documents when reasoning is on (`turtle/query`, "▶ Run query"); the batch path (CLI, browser) has none, as today.
- **No filter.** `reasoning::infer` keeps every inferred triple; the queried-term set and its test go.
- **No renumbering.** With reasoning on, the query sorts its documents as the reasoning base does (`(Source, uri)`), so the base's blank-node numbers are the query graph's: an inferred blank node `g<n>` is node `n`. The `Blanks` translation goes; the existing base/graph equivalence test guards the numbering.
- **Limits.** The input check counts the queried documents (now including imports); the `TooMany` cap and the time checks stay.
- **`files`.** `files` plus the workspace's external imports; other workspace documents are not read. `sources` returns one list.
- **One note text.** The note's sentence is written once in the query module; the CLI and "▶ Run query" use it. JSON shapes stay as they are.
- **Browser.** Cancelling a query that has not started rejects only it; the hint marks only the query the engine was running; the worker call is inside the run's `try`; the pane sends its time limit with the query, so the engine and the watchdog read the same number.

## Testing Decisions

- The seam is the query entry point (`src/query/tests.rs`). New oracle test: for a few workspaces, a query with reasoning gives the same rows as the same query without reasoning over TurbOWL's materialised closure (base plus inferred, written as Turtle).
- Change, don't add, the tests that asserted the split: import triples now appear; `files` no longer reads other documents (`test/query-request.test.mjs`).
- Browser: `web/test/editor-query.test.mjs` for the queued cancel and the hint.
- Benchmark (light, owner's choice): `bench/perf.mjs --only editing` for main, before and after, with query reasoning off and on.

## Out of Scope

- TurbOWL's base-direction bug, an output cap inside TurbOWL, background-reasoning cancellation.
- Following `owl:imports` per file for `files` (all external imports are read).

## Principles

- **subtract-before-you-add:** the filter, the reasoning-only split and the renumbering are deleted, not patched.
- **redesign-from-first-principles:** the query's dataset is "what the reasoner read plus what it inferred", as in other tools.
- **fix-root-causes:** the review's filter and `files` findings are fixed by removing the split that caused them.
- **explain-the-number:** the benchmark row for a query with reasoning names what limits it (reasoning versus evaluation).
