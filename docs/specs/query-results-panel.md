# Query results in a panel (VS Code)

Status: in progress. Tickets:
[../tickets/query-results-panel/](../tickets/query-results-panel/README.md).
Branch: `feat/query-results-panel` (from `feat/turbowl-reasoning`).

## Problem Statement

"▶ Run query" writes its results as a Markdown file in the cache and opens
it. That works in every LSP editor, but in VS Code it is a static page: no
sorting, no copy of a cell, no export, no rerun, no way to flip inferred
triples on or off for one run, and a file on disk per query. Query
extensions in VS Code (SQL clients, Stardog, SPARQL tools) show results as a
table in a panel beside the editor, with rerun and export.

## Solution

In VS Code, "▶ Run query" opens a **Query Results** panel beside the query:
a header with the row count, the time and the files read; the inferred-
triples note when there is one; an **Inferred** choice (Off, Default rules,
Complete rules) and **Rerun**; **Copy** and **Export CSV / JSON**; and the
results as a sortable table (true/false for ASK, the triples for CONSTRUCT
and DESCRIBE), IRIs shortened with the query's prefixes. Rerunning updates
the same panel. Other editors keep the Markdown document.

## User Stories

1. As a VS Code user, I want query results in a table beside the query, so that I read them without a file opening.
2. As a VS Code user, I want rerunning a query to refresh the same panel, so that tabs do not pile up.
3. As a VS Code user, I want one panel per query file, so that two queries' results stay side by side.
4. As a VS Code user, I want the row count, the time and the number of files read in the header, so that I see the run at a glance.
5. As a VS Code user, I want the inferred-triples note in the header, so that I know when results include inferences.
6. As a VS Code user, I want an Inferred choice in the panel, so that I flip reasoning for this query without opening Settings.
7. As a VS Code user, I want the Inferred choice to start at my `turtle.query.reasoning` setting, so that the panel agrees with my settings.
8. As a VS Code user, I want Rerun in the panel, so that I run again after editing data.
9. As a VS Code user, I want to sort by a column, so that I scan results.
10. As a VS Code user, I want IRIs shown with the query's prefixes and the full IRI on hover, so that rows are short and exact.
11. As a VS Code user, I want to copy the table as text, so that I paste it elsewhere.
12. As a VS Code user, I want to export the results as CSV or JSON to a file I choose, so that other tools read them.
13. As a VS Code user, I want ASK to show true or false and CONSTRUCT to show its triples, so that every query form is readable.
14. As a VS Code user, I want a syntax error shown in the panel with its line and column, so that I fix the query.
15. As a VS Code user, I want rows past the row limit counted ("1,000 of 12,345 rows"), so that I know the table is cut.
16. As a VS Code user, I want "Running…" in the panel while a query runs, so that I know it is working; the lens still offers Cancel.
17. As a user of VS Code for the Web, I want the same panel, so that both entries agree.
18. As a Neovim or other LSP editor user, I want the Markdown document as today, so that nothing changes for me.
19. As a VS Code user, I want the panel to follow the editor's theme, so that it fits.
20. As a keyboard user, I want the panel's controls reachable by Tab with visible names, so that I can use it without a mouse.

## Implementation Decisions

- **Server seam.** The client declares `experimental: {"turtle/queryResults": true}` in its `initialize` capabilities. Then `workspace/executeCommand` `turtle.runQuery` writes no file and calls no `showDocument`: it answers with a JSON object:
  `{ "uri", "name", "files", "elapsedMs", "summary", "reasoning": "off"|"default"|"complete", "prefixes": {prefix: namespace}, "answer": … , "inferred"?: … }`
  where `answer` is `turtle/query`'s answer shape (`{"kind":"select","head","results","omitted"}`, `{"kind":"ask","boolean"}`, `{"kind":"construct","triples","omitted"}`) or `{"kind":"error","message","line"?,"column"?}` for a syntax error, a time limit or a failure, and `inferred` is `turtle/query`'s member. The JSON helpers of `turtle/query` are shared, not copied. A cancelled run answers `{"cancelled": true, "uri"}`.
- **Per-run reasoning.** `turtle.runQuery` takes an optional second argument `{"reasoning": "off"|"default"|"complete"}` that overrides `query.reasoning` for that run; parsed at the boundary, a bad value is InvalidParams.
- **Client.** A new `query-results.ts` beside `graph.ts`: a middleware on `executeCommand` sends `turtle.runQuery` on, and when the answer is a result object shows it in the document's panel (created or revealed beside the editor, `preserveFocus`). The panel posts `rerun` (with the chosen reasoning), `copy` and `export` (format) back; export uses a save dialog and `workspace.fs`. Rendering, sorting, IRI shortening and CSV/JSON export are pure functions in a module the webview and the tests share (`query-results-view.js`), bundled like the graph view.
- **Webview.** Theme through VS Code's CSS variables; a content security policy with a nonce, as the graph view has; no external resources.
- **Other editors** are unchanged: without the capability the server writes the Markdown document as today.

## Testing Decisions

- Server, over the protocol (prior art `test/query-lsp.test.mjs`): with the capability, `executeCommand` answers the result object for SELECT, ASK, CONSTRUCT, a syntax error and a reasoning override, and writes no file; without it, the Markdown document as today; a bad reasoning argument is InvalidParams.
- Client, pure functions in node (prior art `editors/vscode/test/*.test.mjs`): table model from each answer kind, sort, IRI shortening, CSV (RFC 4180 quoting) and JSON export.
- Client, the extension host (prior art `editors/vscode/test/suite.cjs` with `run.mjs`): running the lens opens one panel per query document and a rerun reuses it.
- Light verification (owner's choice): no benchmark.

## Out of Scope

- A results panel in the browser editor (it has its own pane) and in other editors.
- Notebooks, query history, charts of results, paging past `query.maxRows`.
- Jumping from an IRI in the results to its definition.

## Principles

- **boundary-discipline:** the capability, the reasoning argument and the webview's messages are parsed where they enter; inside, typed.
- **model-the-domain:** the panel renders one result type with a kind per answer form, matched exhaustively.
- **separate-before-serializing-shared-state:** the server and client tickets touch disjoint files and meet at the JSON contract above.
