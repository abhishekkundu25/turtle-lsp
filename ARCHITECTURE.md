# Architecture

turtle-lsp is one Rust crate with three front ends: the language server
(`turtle-lsp` with no command), the command line (`check`, `format`,
`query`) and the browser engine (`web/engine`, the same crate compiled to
WebAssembly). This page says where things live and which rules hold the
parts together. The crate doc in `src/lib.rs` has the module index that this
page follows; the module docs (`//!` at the top of each file) say the rest.

## The modules

| Module | Owns | Called by |
|---|---|---|
| `syntax` | Turtle, TriG, N-Triples and N-Quads lexer and parser; `syntax::names`, the rules for writing an IRI in Turtle | every feature |
| `sparql` | SPARQL documents: tokens, structure, checks, formatting; the SPARQL language features in `sparql::features` | server, `query`, `shacl` |
| `jsonld` | JSON-LD 1.1: a fault-tolerant JSON parser, context processing, expansion and conversion to the same parse as Turtle | `analysis` |
| `analysis` | One document's parse, positions and per-IRI indexes | `features`, `server` |
| `workspace` | Open documents, indexed files and project prefixes | `server`, `batch`, `cli` |
| `knowledge` | What the workspace says about each IRI | `features` |
| `vocab` | The embedded well-known vocabularies (`data/vocab.tsv`) | `knowledge`, `features` |
| `imports` | `owl:imports` resolution, catalogs, RDF/XML conversion and the import cache | `server`, `cli` |
| `fetch` | Web requests: redirects, the address rule, size, time and request budgets, the cache file layout (not in the browser build) | `imports`, `jsonld` |
| `features` | Completion, hover, diagnostics, formatting and the other editor features of RDF documents | `server`, `batch` |
| `diagnose` | The diagnostics composer: one document's diagnostics, the same for the editor and `check` | `server`, `batch` |
| `shacl` | Shapes, SHACL Core and SHACL-SPARQL validation | `features` |
| `rdf` | The union of documents' triples as a graph and as a SPARQL dataset | `query`, `shacl` |
| `query` | Running SPARQL queries and rendering the results | `cli`, `server`, the browser engine |
| `settings` | Client settings and their layers | `server`, `cli` |
| `commands` | `workspace/executeCommand` commands | `server` |
| `server` | The LSP main loop, its timers and worker threads | `main` |
| `batch` | Files analysed as one workspace without the event loop | `cli`, the browser engine |
| `cli` | `check`, `format` and `query` (not in the browser build) | `main` |
| `clock` | The time the server's timers read | `server`, `imports` |
| `ide`, `layout` | Empty: places for shared editor state and a shared layout engine, planned for later work | |

Outside `src/`:

| Path | What it is |
|---|---|
| `web/engine` | The browser engine: a `cdylib` over `batch` and `server::Embedded`, with its own `Cargo.lock` |
| `web/app` | The browser editor (`index.html`, `editor.js`) and the toolbox (`toolbox.html`, `app.js`) |
| `editors/vscode` | The VS Code extension (`src/extension.ts`, `src/server.ts`) and its grammars |
| `lsp/`, `plugin/`, `build.lua` | The Neovim plugin: the server config, file types, and the server download |
| `data/` | The vocabulary snapshot and the bundled JSON-LD contexts, compiled into the binary |
| `vendor/spareval` | spareval 0.2.7 with cancellation checks inside its joins (`PATCH.md`) |
| `tests/` | Rust integration tests: W3C suites (`tests/w3c`), oracles against Oxigraph's parsers, the chaos test, budgets |
| `test/` | Node end-to-end tests that drive the built server over stdio, and the Neovim checks |
| `bench/` | `perf.mjs`, the latency benchmark |
| `tools/` | The vocabulary generator, the corpus scripts, `check-vendor.sh` |

## One parse per document version

Every editor feature reads one `Analysis` per document version. The
workspace builds it once (`Analysis::new`), from the text, the prefixes
the project supplies and the document's URI, which is also the base IRI. It holds the
parse (`syntax::Parsed`), the line index and the per-IRI indexes (subjects,
references, labels, comments, types, superclasses). Features never parse
the text again; they take `&Analysis` and map positions with its helpers
(`offset`, `position`, `range`, in UTF-8 or UTF-16 as the client
negotiated).

A JSON-LD document produces the same `Parsed` through `jsonld`, so
knowledge, lints, SHACL and the command line need no JSON-LD-specific code.
A SPARQL document gets a `SparqlAnalysis` instead, with the same position
helpers; SPARQL documents never feed the knowledge index and RDF features
never run on them.

The knowledge index (`knowledge::Knowledge`) keeps each source's
contribution (`DocFacts`) and subtracts exactly that when the source
changes, so a keystroke costs the size of one document, not of the
workspace.

## The seams

Four interfaces let the same code run in the server, on the command line
and in the browser:

- **`batch`**: a set of files analysed as one workspace, without the event
  loop. `analyse`, `fix_all` and `query` (and their `_on_disk` forms, which
  load imports as the editor does) back `turtle-lsp check`, `format`,
  `query` and the browser toolbox.
- **`server::Embedded`**: the language server driven by its host instead of
  its own loop: the host hands over messages, and calls `tick` when a timer
  is due. The browser editor runs the server this way in a Web Worker; SHACL
  and imports run inline there.
- **`jsonld::ContextLoader`**: where remote `@context` documents come from.
  `DesktopLoader` reads the import cache, the bundled contexts and, when
  allowed, the network; `Bundled` is the browser's; `NoNetwork` serves
  tests.
- **`query::run`**: one SPARQL query over a set of analyses with `Limits`
  (time, rows, buffered rows, a cancel flag). `turtle-lsp query`, the
  "▶ Run query" lens and the browser's Run button all call it, and
  `tests/w3c_sparql_eval.rs` checks it against the W3C evaluation tests.

`diagnose::document` composes what a document shows (its own diagnostics,
then unresolved imports, then SHACL results) for the server, for workspace
diagnostics and for `batch`, so the editor and `check` cannot disagree
(`tests/diagnostics_agree.rs`).

## Where each feature family lives

| Family | RDF documents | SPARQL documents |
|---|---|---|
| Diagnostics and lints | `features::diagnostics`, `features::lint`, `features::literals`, `features::shacl_lint` | `sparql::features::diagnostics` |
| Quick fixes and refactorings | `features::quickfix`, `features::refactor` | `sparql::features::actions` |
| Completion | `features::completion` (ranking in `completion/ranking.rs`, JSON-LD in `completion/jsonld.rs`) | `sparql::features::completion` |
| Hover, inlay hints | `features::hover`, `features::inlay` | `sparql::features::display` |
| Navigation, rename | `features::navigation` (JSON-LD in `navigation/jsonld.rs`) | `sparql::features::navigation`, `sparql::features::rename` |
| Formatting | `features::format`, `jsonld::format` | `sparql::format` |
| Outline, folding, semantic tokens | `features::symbols`, `features::semantic` | `sparql::features::outline`, `sparql::features::display` |
| Code lenses | `features::lens` (reference counts) | `features::lens` ("▶ Run query"), run by `server::run_query` |
| SPARQL inside SHACL shapes | `features::embedded` | |
| SHACL validation | `features::shacl_validate` over `shacl` | |
| Type hierarchy | `features::hierarchy` | |

## Threads

The server's main loop owns all state and runs on a thread with a 64 MiB
stack. Long work runs on worker threads and comes back as messages: the
initial index, imports (`imports::worker`), JSON-LD context fetches, SHACL
validation, workspace diagnostics and query runs. Each job carries a
generation, and the main loop drops answers from superseded jobs. Every
handler and every phase runs under a guard that turns a panic into an error
report, and threads start through `server::worker::spawn_guarded`.

## Files most changes touch

`src/server.rs`, `src/workspace.rs`, `src/lib.rs`, `src/syntax/mod.rs`,
`src/settings.rs`, `Cargo.toml`, the root `package.json` and
`.github/workflows/` are shared by many features; changes that run in
parallel should not edit them at the same time. The dependency lock files
are `Cargo.lock`, `web/engine/Cargo.lock`, `package-lock.json` and
`editors/vscode/package-lock.json`; build with `--locked` so they never
change by accident.
