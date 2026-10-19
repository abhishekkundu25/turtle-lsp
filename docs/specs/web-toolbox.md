# RDF Toolbox in the browser (iteration 1)

Status: iteration 1 done, on branch `web-showcase` (not merged to main).

## Problem Statement

People who work with RDF want to format a file, find what is wrong with it,
validate data against SHACL shapes or tidy a SPARQL query without installing
an editor extension. The browser tools that exist each do one of these, often
by uploading the data to a server, and none of them knows the vocabularies,
offers fixes or proves that formatting kept the graph. turtle-lsp does all of
this, but only inside an editor.

## Solution

A static web page, the RDF Toolbox, that runs turtle-lsp itself as
WebAssembly in a Web Worker. Nothing is uploaded; the page works offline once
loaded. Four tools share one input (a set of files):

- **Format**: the formatted text as a diff against the input, with the
  guarantee the formatter already checks ("same graph" for RDF, "same SPARQL
  algebra" for queries), or the reason it will not format.
- **Validate**: every problem turtle-lsp reports in every file, each with its
  quick fixes; one click applies a fix, "Apply all" applies every preferred one.
- **SHACL**: the violations of the data against the shapes in the input. The
  tab appears only when the input has shapes.
- **SPARQL**: the problems and the formatted text of the queries and updates
  in the input.

The layout is layout A of the prototype (`web/prototype/toolbox-tools.html`,
"Tool tabs"): a header, one tab per tool with a live count, and a split view
with the input on the left and the selected tool on the right, stacked on a
phone.

## User Stories

1. As a user, I want to open the page and see sample files already analysed, so that I see what the toolbox does before giving it my data.
2. As a user, I want to pick another sample set, so that I can see each tool on data that suits it.
3. As a user, I want to paste text as a new file, so that I can check a snippet quickly.
4. As a user, I want to open files from my disk, so that I can check real files.
5. As a user, I want to drop files on the page, so that I do not need the file picker.
6. As a user, I want each file's kind (Turtle, TriG, N-Triples, N-Quads, SPARQL) taken from its name, so that I do not have to say it.
7. As a user, I want to edit a file in the input and see every tool update as I type, so that I can fix problems in place.
8. As a user, I want to remove a file from the input, so that I can narrow down what is checked.
9. As a user, I want my data to stay in my browser, so that I can check private data.
10. As a user, I want each tab to show a live count (files that would change, problems, violations), so that I know where to look.
11. As a user, I want the formatted text shown as a diff against mine, so that I can see what changes.
12. As a user, I want to be told the formatter kept the same graph (or the same query algebra), so that I trust the result.
13. As a user, I want a file with syntax errors to say it will not be formatted and why, so that I fix the errors first.
14. As a user, I want to apply the formatting to the input or copy it, so that I can use the result.
15. As a user, I want every problem listed with its file, line, severity and message, so that I can find it.
16. As a user, I want clicking a problem to show its line in the input, so that I see the context.
17. As a user, I want each problem's quick fixes as buttons, so that I can fix it in one click.
18. As a user, I want "Apply all" to apply every preferred fix across files, so that I can clean up a file at once.
19. As a user, I want the problems to update after a fix, so that I see what is left.
20. As a user, I want SHACL violations listed by focus node and shape, so that I can see which data breaks which constraint.
21. As a user, I want the SHACL tab to appear only when the input has shapes, so that it does not distract otherwise.
22. As a user, I want SPARQL problems (syntax, undeclared or unused prefixes, unbound projections) with fixes such as adding a PREFIX line, so that I can tidy a query.
23. As a user, I want the formatted query with its algebra guarantee, so that I trust it.
24. As a user, I want the page to work on my phone, so that I can check a file on the go.
25. As a user, I want the page to keep working after a crash in the engine (it restarts and says so), so that one bad input does not break the page.
26. As a user, I want to download a file from the input, so that I can save the fixed or formatted result.
27. As a maintainer, I want the browser build to use the same engine code as the language server, so that the two never disagree.
28. As a maintainer, I want the language server build and its tests unchanged by the browser build, so that editors are not affected.
29. As a maintainer, I want the page deployable as static files, so that it can be hosted anywhere (a subdomain on GitHub or Cloudflare Pages).

## Implementation Decisions

- **Engine crate.** A separate crate in the web folder, depending on turtle-lsp
  by path, builds to a `wasm32-unknown-unknown` library. It exposes a raw
  JSON-in/JSON-out interface (allocate, free, call, result pointer and length):
  no wasm-bindgen, no generated glue, so the build needs only cargo and the
  page loads the module with plain `WebAssembly.instantiate`.
- **Engine operations.** One request names an operation and carries the
  whole file set (name and text per file):
  - `analyse`: per file, its kind, its diagnostics (UTF-16 line/character
    ranges, severity, code, message) each with its quick fixes (title,
    preferred, edits per file), and its formatting (the new text, "already
    formatted", or the reason it is not formatted, with the guarantee
    checked); for the set, the SHACL violations (in the diagnostics, code
    `shacl-violation`) and whether the input has shapes.
  - `fix_all`: the texts after applying every preferred quick fix, repeated
    until none is left or a round changes nothing (bounded rounds), skipping
    edits that overlap one already taken.
  - `version`: the turtle-lsp version.
  The engine builds a turtle-lsp `Workspace` from the files (every file open,
  under `file:///work/`), so cross-file knowledge (a property declared in the
  ontology, shapes in another file) works as in an editor. It calls the same
  feature functions the server calls for diagnostics, code actions,
  formatting and SHACL validation (synchronously, cancel never).
- **Changes to turtle-lsp, browser target only.** No change to the native
  build:
  - `ureq` and `mimalloc` become native-only dependencies.
  - Remote import fetching returns an error in the browser build.
  - File URL and path conversions return "no path" in the browser build
    (there is no file system); the prefix file lookup and the import cache
    are therefore off there.
  - The SPARQL formatter's algebra check runs inline instead of on a thread
    with a large stack; the browser build's stack is set large at link time
    instead.
- **Randomness.** The engine provides getrandom's custom backend from the
  page's `crypto.getRandomValues` (an imported function), so no JS glue crate
  is needed.
- **Worker bridge.** The page talks to a Web Worker by messages
  (`{id, op, files}` and `{id, result | error}`). Analysis is debounced while
  typing; only the latest answer is applied. A trap in the engine (a panic is
  an abort in WebAssembly) is caught, the module is instantiated again, and
  the page shows an error notice instead of results.
- **Page.** Plain HTML, CSS and JS modules, no framework and no build step
  beyond copying the `.wasm`. The input is a list of files with a text editor
  for the selected one. Imports (`owl:imports`) are not followed in the
  browser; the page says so.
- **Deployment.** Static files from the web app folder; a workflow building
  the `.wasm` and publishing the folder comes later, with the domain.

## Testing Decisions

- Tests check behaviour seen from outside: what the engine answers for given
  files, and what the page shows and does when a user acts. Not internal
  functions.
- **Engine tests** (Node, the built `.wasm` loaded as the page loads it):
  analyse reports a syntax error, an unknown term with its fix, undeclared
  and unused prefixes with fixes, cross-file knowledge, SHACL violations only
  when shapes exist, formatting with the reason for refusing, SPARQL
  diagnostics and formatting; fix_all leaves no fixable problem and is
  bounded; a crash-prone deep input is refused rather than trapping;
  the module survives many calls without growing without bound.
- **Browser tests** (Playwright driving the installed Chrome, the page served
  by a local static server on its own port): the page loads the samples and
  shows counts on the tabs; each tab shows its results; applying a fix and
  "Apply all" change the input and the counts; editing the input updates the
  tools; pasting a file adds it; the SHACL tab follows the presence of shapes;
  the phone width stacks the panes with no horizontal scroll; no console
  errors; no network requests other than the page's own files.
- Prior art: the Node LSP tests (`test/*.test.mjs`) drive the server from
  outside in the same style; the engine tests reuse their fixtures where they
  fit.
- The native build keeps passing its existing suites (`cargo test`,
  `npm test`, clippy); no test of the language server changes.

## Out of Scope

- Convert between RDF syntaxes (iteration 2).
- Following `owl:imports` from the browser.
- Hover, completion, rename and the other editor features.
- The deploy workflow and the domain.
- Persisting the input between visits.

## Further Notes

The build check before this spec (a `cargo check` of the library for
`wasm32-unknown-unknown`) found that only `ureq` (its TLS stack), `mimalloc`,
getrandom's backend and ten errors in the import and workspace path code
stood in the way; the parser, diagnostics, formatter, SHACL and the SPARQL
module compiled unchanged.

## Departures from the plan

- The randomness backend and the stack size are set in the engine crate's
  cargo configuration, not by a feature switch in turtle-lsp: the browser
  differences in turtle-lsp key on the `wasm32` target, so the native build
  has no new feature to keep in step.
- In the browser build the SPARQL formatter refuses a query whose check would
  need more than the smallest native stack (about 32,000 tokens) instead of
  risking a trap.
- Opening the SPARQL tab puts its query in the editor.

## Verification (2026-10-19)

- Engine (Node, the built module): 22/22, including parity with the native
  server's diagnostics on all three sample sets.
- Browser (Playwright, installed Google Chrome, desktop and phone width):
  10/10; no console errors, no request outside the page's own files.
  Analysis of the starter set takes 13–14 ms. WebKit was not run (Playwright's
  WebKit build is not installed).
- Native: `cargo test` all suites pass, clippy clean on both crates, Node
  end-to-end 490/490 (Neovim included).
- Module size: 5.9 MB uncompressed.
- Resource hygiene: no processes, servers or temp directories left; idle CPU.
