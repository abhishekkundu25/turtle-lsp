# Running SPARQL queries over the workspace

Status: done in 0.7.0. Builds on docs/specs/lsp-capability.md (the shared
`rdf` module, the command module and code lenses). Tickets:
docs/tickets/workspace-query/.

## Problem Statement

turtle-lsp checks SPARQL queries but can't run them. To see what a query
returns over the project's own files, a user needs another tool: a triple
store, a workbench extension, or a script with rdflib. The engine to run them
is already inside turtle-lsp (`spareval`, used for SHACL-SPARQL); it just has
no way in.

## Solution

One query feature in the core library, reached three ways:

- **Command line:** `turtle-lsp query` runs a query over the workspace's
  files and prints the results as a table, JSON, CSV, TSV or, for CONSTRUCT,
  Turtle.
- **Editors:** a "▶ Run query" code lens on each query in a `.rq` file runs
  it over the workspace as the editor sees it, unsaved edits included, and
  opens the results as a Markdown document through `window/showDocument`.
  Standard LSP only, so every LSP editor gets it with no client code.
- **Browser editor:** a Run button and a results pane for `.rq` files.

Queries run only when asked, off the main loop, on a snapshot; typing is
never slowed.

## User Stories

1. As a user with RDF files in a folder, I want `turtle-lsp query q.rq` to print what the query returns over those files, so that I don't need a triple store.
2. As a user writing a quick one-off, I want to pass the query inline (`-e 'SELECT …'`), so that I don't need a file.
3. As a scripter, I want SELECT results as SPARQL JSON, CSV or TSV, so that other tools can read them.
4. As a terminal user, I want an aligned table by default, so that results are readable.
5. As a user running CONSTRUCT or DESCRIBE, I want the graph as formatted Turtle with the project's prefixes, so that it's readable and reusable.
6. As a user running ASK, I want `true` or `false` and an exit code, so that scripts can branch on it.
7. As a CI user, I want exit code 0 for a query that ran, 2 for a syntax error or a tool failure, so that failures are clear.
8. As a user of headerless files, I want `prefixes.ttl` and the `prefixes` setting to apply to the query and the data, so that it behaves like the editor.
9. As a user of a project with TriG, N-Quads or JSON-LD, I want every triple of every graph in the default graph, so that a plain query sees all the data.
10. As a user choosing files, I want to pass paths, as with `check`, so that I can query a subset.
11. As an editor user, I want "▶ Run query" above each query in a `.rq` file, so that running it is one click.
12. As an editor user, I want the results to open beside the query as a readable table, so that I don't leave the editor.
13. As an editor user, I want the query to see my unsaved edits, so that the results match what's on screen.
14. As an editor user, I want typing to stay fast while a query runs, so that a slow query doesn't freeze the editor.
15. As an editor user, I want a slow query stopped after a time limit with a clear message, so that it can't run forever.
16. As an editor user, I want large results cut at a row limit with a note of the total, so that the document stays usable.
17. As an editor user whose client can't open documents, I want the result summary as a message, so that the command still does something.
18. As a browser editor user, I want a Run button on `.rq` files and the results under the editor, so that I can try queries in the playground.
19. As a maintainer, I want one engine behind the CLI, the server and the browser, so that results never differ between them.
20. As a user writing an UPDATE, I want a clear "not supported" message, so that I know it wasn't run.

## Implementation Decisions

- **Core query module** (new, in the library): takes the documents to query
  (the same analyses SHACL validation reads), the query text, its base and
  prefixes, and limits (time, rows). It builds an `rdf::graph::Graph` over
  the documents and evaluates with `spareval` through `rdf::dataset`. It
  returns a typed outcome: solutions (variables and rows of terms), a
  boolean, or triples; or an error (syntax, with position; unsupported form;
  time limit). UPDATE is rejected as unsupported. Evaluation is cancellable
  through the evaluator's cancellation hook (or a deadline check), so a time
  limit stops it.
- **Dataset.** The default graph is the union of every triple of every
  document, named graphs included (as SHACL validation sees shared data). No
  named graphs in v1: `GRAPH` patterns and `FROM NAMED` match nothing; this
  is documented. `FROM` is ignored with the same note.
- **Rendering** (in the same module, pure functions): aligned text table;
  Markdown table (escaped pipes, a row limit with "showing N of M"); SPARQL
  1.1 Query Results JSON; CSV and TSV per the SPARQL results CSV/TSV formats;
  triples as Turtle through the existing formatter, with the project's
  prefixes. IRIs shown compact where a known prefix applies (table and
  Markdown only).
- **Batch API.** `batch` gains a query entry point next to `analyse`, so the
  CLI and the browser engine share file collection, settings and prefix
  handling.
- **CLI.** `turtle-lsp query (FILE.rq | -e QUERY) [PATH…]` with `--format
  table|json|csv|tsv|turtle`, `--root`, `--timeout SECONDS` (default 30),
  `--limit ROWS` (table only; default none). Exit codes: 0 ran, 1 ASK false,
  2 error. Usage text and `--help` updated.
- **Server.** A code lens "▶ Run query" on each query in a SPARQL document
  (one per document today), when the document parses as a query (not an
  update). Command `turtle.runQuery` with the document URI. The server takes
  a snapshot of the workspace analyses (open documents' current text
  included) and runs the query on a background thread with the time limit,
  never on the main loop. The result is written as Markdown to
  `<cache dir>/query-results/<file stem>.md` (overwritten each run) and opened
  with `window/showDocument` (`takeFocus: false`) when the client supports it;
  otherwise a `window/showMessage` with the summary (row count or boolean or
  triple count, and the time). The command's response returns the same
  summary. Settings: `query.timeoutSeconds` (default 30), `query.maxRows`
  (default 1000).
- **Browser.** The web engine gains a query request through its existing
  `handle` API, built on the batch entry point. The editor page shows a Run
  button for `.rq` files and a results pane (table, boolean or Turtle), with
  the same limits.

## Testing Decisions

- Core module: Rust unit tests for each query form, the union default graph,
  `GRAPH` matching nothing, UPDATE rejected, syntax error position, the time
  limit, and each renderer (prior art: src/sparql/tests.rs, src/batch.rs
  tests).
- CLI: Node tests (prior art: test/cli.test.mjs) for each format, inline
  queries, paths, prefixes from `prefixes.ttl`, exit codes.
- Server: Node LSP tests (prior art: test/lsp.test.mjs) for the lens, the
  command writing the results file and requesting `window/showDocument`, the
  fallback message when the client lacks support, unsaved edits seen, a
  query over the time limit stopped, and a keystroke answered while a slow
  query runs.
- Browser: web tests (prior art: web/test/editor.test.mjs) for the Run button
  and the results pane.
- Performance: `bench/perf.mjs` keystroke rows unchanged; a new row for
  running a query over the benchmark workspace.

## Out of Scope

- SPARQL UPDATE, named graphs, `FROM`/`FROM NAMED`, `SERVICE`.
- Remote endpoints, notebooks, saved queries, result paging or editing.
- A custom results view in any editor (Markdown via `showDocument` only).
- Running SPARQL embedded in other files (SHACL-SPARQL, Markdown).

## Further Notes

- Named graphs are the likely first follow-up: TriG-heavy projects will want
  `GRAPH ?g`. The graph would need to keep each triple's graph name; that
  touches SHACL's graph too, so it's left out of v1.
