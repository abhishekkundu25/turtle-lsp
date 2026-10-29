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
| `sparql` | SPARQL documents: tokens, structure, checks, formatting; the SPARQL language features in `sparql::features` | `ide`, `analysis`, `query`, `shacl` |
| `jsonld` | JSON-LD 1.1: a fault-tolerant JSON parser, context processing, expansion and conversion to the same parse as Turtle | `analysis` |
| `analysis` | One document's parse, positions and per-IRI indexes | `features`, `ide`, `server` |
| `workspace` | Open documents, indexed files and project prefixes; the file-kind table | `server`, `ide`, `batch`, `cli`, `features`, `diagnose`, `imports`, `jsonld` |
| `knowledge` | What the workspace says about each IRI | `features` |
| `vocab` | The embedded well-known vocabularies (`data/vocab.tsv`) | `knowledge`, `features` |
| `imports` | `owl:imports` resolution, catalogs, RDF/XML conversion and the import cache | `workspace`, `server`, `batch` |
| `fetch` | Web requests: redirects, the address rule, size, time and request budgets, the cache file layout (not in the browser build) | `imports`, `jsonld` |
| `features` | Completion, hover, diagnostics, formatting and the other editor features of RDF documents | `server`, `ide`, `batch` |
| `diagnose` | The diagnostics composer: one document's diagnostics, the same for the editor and `check` | `ide`, `server`, `batch` |
| `shacl` | The shape model, the SHACL vocabulary, SHACL Core and SHACL-SPARQL validation | `features`, `knowledge` |
| `rdf` | The union of documents' triples as a graph and as a SPARQL dataset | `query`, `shacl` |
| `query` | Running SPARQL queries and rendering the results | `cli`, `server`, `batch`, the browser engine |
| `settings` | Client settings and their layers | `server`, `cli` |
| `commands` | `workspace/executeCommand` commands | `server` |
| `server` | The LSP main loop, its state groups, its timers, the stdio transport and the worker threads other modules start | `main`, the browser engine |
| `batch` | Files analysed as one workspace without the event loop | `cli`, the browser engine |
| `cli` | `check`, `format` and `query` (not in the browser build) | `main` |
| `clock` | The time the server's timers read | `server`, `query`, `shacl` |
| `ide` | `ide::Session`: every editor request over one workspace, routed by document kind | `server` |
| `layout` | What the Turtle and SPARQL formatters share: the layout options, the token document, the triple terms and the printer | `features`, `sparql`, `ide`, `batch`; `jsonld` and `query` take its options |

Seven modules are private to the crate: `commands`, `diagnose`, `fetch`,
`ide`, `layout`, `vocab` and `workspace`. The others are public because a
consumer uses them: the browser engine, the examples, the integration tests
or `src/main.rs`. The "Public interface" section of the crate doc lists
every path those consumers use, one per line, as a doctest, so
`cargo test --release --locked --doc` fails when one of the paths stops
being public. An item that no consumer names is `pub(crate)`.

Inside the larger modules, these files hold one part each:

| Path | What it holds |
|---|---|
| `src/ide/{navigation,display,edit}.rs` | `ide::Session`'s requests in three groups: navigation and rename; what the editor shows (hover, symbols, folding, type hierarchy, semantic tokens, inlay hints, lenses); what it changes and reports (completion, code actions, formatting, diagnostics) |
| `src/layout/doc.rs` | The token document: the tokens a formatter lays out, and each comment attached to a token |
| `src/layout/terms.rs` | The triple terms (`Term`, `Obj`, `Po`) and their widths on one line |
| `src/layout/printer.rs` | `Printer`, which both formatters write through; `Style` holds the two places where their triple layouts differ |
| `src/syntax/splice.rs` | The statement splice: the parse after one edit, from the parse before it and the statements around the edit, or a `Fallback` that says why the caller parses in full |
| `src/features/kept.rs` | `Kept`: the findings an open document keeps between edits, for the checks that keep them, and what each update checks again |
| `src/oracle.rs` | The incremental oracle: a document edited through the workspace must give what a fresh workspace opened on the final text gives (public, hidden from the docs, for the tests) |
| `src/workspace/kinds.rs` | The file-kind table (`ROWS`): which extensions and file names are RDF, SPARQL, import sources, catalogs or JSON, and in which letter case |
| `src/shacl/model.rs` | The shapes in a graph, with their paths, targets and parameters |
| `src/shacl/vocab.rs` | The SHACL, RDF, RDFS and OWL IRIs the SHACL code compares against, interned once |
| `src/server/{client,progress,shacl,refresh,layers,index}.rs` | The server's state groups (see [Threads](#threads)) |
| `src/server/worker.rs` | `Spawn`, `spawn_guarded` and `Feed`: how jobs start and how their answers come back |
| `src/server/wire.rs` | The stdio transport and the `initialize` handshake |
| `src/server/timers.rs` | The server's one-shot timers, keyed by what they are for |
| `src/server/{run_query,workspace_diagnostics}.rs` | "▶ Run query", and the diagnostics of files that are not open |

## One parse per document version

Every editor feature reads one `Analysis` per document version. The
workspace builds it once (`Analysis::new`), from the text, the prefixes
the project supplies and the document's URI, which is also the base IRI. It holds the
parse (`syntax::Parsed`), the line index and the per-IRI indexes (subjects,
references, labels, comments, types, superclasses). Features never parse
the text again; they take `&Analysis` and map positions with its helpers
(`offset`, `position`, `range`, in UTF-8 or UTF-16 as the client
negotiated). The server keeps the parse's tokens. `batch` (the command line
and the browser toolbox) drops them, and `Analysis::tokens` lexes the text
again the first time a feature asks for them.

A JSON-LD document produces the same `Parsed` through `jsonld`, so
knowledge, lints, SHACL and the command line need no JSON-LD-specific code.
The Turtle-family parser does not call `jsonld`: JSON-LD has its own front
end, and `syntax::parse_with_base` reads only Turtle, TriG, N-Triples and
N-Quads. A SPARQL document gets a `SparqlAnalysis` instead, with the same
position helpers; SPARQL documents never feed the knowledge index and RDF
features never run on them.

The knowledge index (`knowledge::Knowledge`) keeps each source's
contribution (`DocFacts`) and applies only the difference when the source
changes, so a keystroke costs the size of one document, not of the
workspace. Each change leaves a change set (`knowledge::Changes`): the IRIs,
label and definition texts and namespaces whose entries other documents'
checks read.

## Edits between analyses

An open Turtle, TriG, N-Triples or N-Quads document keeps what the next
analysis builds on, in the `Analyzed` state of its workspace entry: after
`didChange`, the last analysis, the client's changes since it collapsed into
one `syntax::splice::Edit`, and the line starts of the text now (moved
along with each change, not found again). The next analysis splices the
statements around the edit into the old parse (`syntax::splice`). Any case
the splice does not cover falls back to a full parse, and so does every
analysis with the setting `parse.incremental` off, the first one, a change
of the whole text, and a change of the prefixes or contexts in effect.
SPARQL and JSON-LD documents, the command line and the browser engine
always parse in full.

The diagnostics then check again only what can have changed
(`features::kept`). Each check in `features::diagnostics` is a `Check`, and
`Check::reads` says what it reads besides the statement it reports in
(`Reads`): its statement alone, the statements about the same subject, the
knowledge index's entries for the IRIs and texts written there, the whole
document, or the workspace beyond the change set. The matches are
exhaustive, so a new check does not compile until it declares its class.
`Check::kept` picks the checks whose findings an open document keeps: only
those that read the statement, the subject or the index, so that the
splice's window and the index's change set say when a finding is stale.
Everything else runs in full on every analysis. Declare a new check by
what it reads, not by what it reads today in the common case: a class too
narrow keeps a stale finding.

The oracle (`oracle::check`) is the proof: it applies batches of changes
through the workspace, as a client sends them, and compares the parse,
errors included, the analysis, the knowledge index and the composed
diagnostics with a fresh workspace opened on the final text. The chaos
tests (`tests/chaos.rs`) run it on every iteration of their edit mode, and
`tests/corpus_edits.rs` runs it on the public corpus.

## The seams

Five interfaces let the same code run in the server, on the command line
and in the browser:

- **`ide::Session`**: every editor request over one workspace. A request
  names a document by URI, and the session routes it by kind: RDF documents
  to `features`, SPARQL documents to `sparql::features`. Where one family
  reaches the other's documents, the session composes the answers:
  references and rename of an RDF term include its uses in SPARQL files. The
  server builds a session per request and only decodes the params and
  encodes the answer.
- **`batch`**: a set of files analysed as one workspace, without the event
  loop. `analyse`, `fix_all` and `query` (and their `_on_disk` forms, which
  load imports as the editor does) back `turtle-lsp check`, `format`,
  `query` and the browser toolbox. `Options` names what a caller reads:
  `check` asks for no formatter verdict, `format` for no diagnostics (and
  so no imports); the browser asks for both.
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
(`tests/diagnostics_agree.rs`). The one difference is the number of syntax
errors. The editor and the browser toolbox show the first 1,000 of a
Turtle-family document and one error that says there are more
(`features::diagnostics::MAX_SYNTAX_ERRORS`); `check` reports all of them.

Every `Workspace` method that changes what the workspace holds (a document
opened, edited, saved or closed, files changed on disk, the settings'
prefixes, a context fetched) returns `Effects`: which open documents to
diagnose again, whether knowledge outside one document changed, whether
SHACL is due, and which indexed files to parse again. The workspace decides
what a change means; `Server::apply` acts on it in one place.

One table in `src/workspace/kinds.rs` says what kind of file a path names,
for the workspace, the server and the command line's file scan
(`src/cli/files.rs`). The file-watcher glob (`server::WATCHED_FILES`) lists
every row, and a test checks it. `Syntax::from_extension` keeps its own
list, because `syntax` cannot read `workspace`, and a test checks it
against the table's RDF rows.

The shape lint and the validator read one shape model (`shacl::model`) and
one vocabulary (`shacl::vocab`). The completion index (`knowledge`) uses
the same vocabulary but keeps its own, narrower shape builder, because
building from the model costs two to four times as much per edit. A test in
`src/knowledge.rs` checks that the builder finds the model's shapes that
target classes.

## Where each feature family lives

`ide::Session` routes each request to one of these, by document kind:

| Family | RDF documents | SPARQL documents |
|---|---|---|
| Diagnostics and lints | `features::diagnostics`, `features::lint`, `features::literals`, `features::shacl_lint` | `sparql::features::diagnostics` |
| Quick fixes and refactorings | `features::quickfix`, `features::refactor` | `sparql::features::actions` |
| Completion | `features::completion` (ranking in `completion/ranking.rs`, JSON-LD in `completion/jsonld.rs`) | `sparql::features::completion` |
| Hover, inlay hints | `features::hover`, `features::inlay` | `sparql::features::display` |
| Navigation, rename | `features::navigation` (JSON-LD in `navigation/jsonld.rs`) | `sparql::features::navigation`, `sparql::features::rename` |
| Formatting | `features::format` over `layout`, `jsonld::format` | `sparql::format` over `layout` |
| Outline, folding, semantic tokens | `features::symbols`, `features::semantic` | `sparql::features::outline`, `sparql::features::display` |
| Code lenses | `features::lens` (reference counts) | `features::lens` ("▶ Run query"), run by `server::run_query` |
| SPARQL inside SHACL shapes | `features::embedded` | |
| SHACL validation | `features::shacl_validate` over `shacl` | |
| Type hierarchy | `features::hierarchy` | |
| Workspace graph (`turtle/graph`) | `features::graph` | |

The workspace graph is the server's one custom request, listed under
`experimental` capabilities. `features::graph` reads the knowledge index
and changes nothing: the query, the filters, the cap and the Barnes-Hut
layout. The client passes back the positions it has, so the server keeps
no graph state. The native server answers it on a thread of its own over
a copy of the index; the browser engine answers inline. The drawing is
`web/app/graph/`, one module the browser editor mounts and the VS Code
extension bundles into its webview (`editors/vscode/src/graph.ts`).

## Threads

The server's main loop owns all state and runs on a thread with a 64 MiB
stack (`server::MAIN_STACK`). Its state is in groups, each a struct that the
server drives: `progress` (work-done progress), `shacl` (validation
scheduling and results), `refresh` (pulled-diagnostics result ids and
refresh requests), `layers` (the settings and their layers) and `index` (the
workspace index as it arrives). The groups send to the client through
`client::Client`, never through the server.

Long work runs on worker threads and comes back as messages: the initial
index, imports (`imports::worker`), JSON-LD context fetches, SHACL
validation, workspace diagnostics and query runs. `worker::Spawn` says how
a job runs: `Spawn::Thread` on a thread of its own in the stdio server,
`Spawn::Inline` on the caller's thread when the server is embedded in a
host with no threads. Threads start through `server::worker::spawn_guarded`,
which turns a panic into a message. A SHACL or workspace-diagnostics thread
that cannot start is reported to the client as a warning, and the server
goes on. Each job carries a generation, and the main loop drops answers
from superseded jobs. Every handler and every phase runs under a guard that
turns a panic into an error report.

Two more threads carry the connection (`server::wire`). One reads stdin and
queues the messages, so the server takes them a batch at a time. The other
writes stdout, in the order the server sent. A request's result goes out as
JSON text, never as a `serde_json::Value` tree, with the bytes lsp-server
would write. Embedded, the host gets `lsp_server::Message` values instead.

## Files most changes touch

In wave 3 of the 2026-10 audit (21 code commits), the files changed most
often were `src/server.rs` (10 commits), `src/workspace.rs` (7),
`src/server/workspace_diagnostics.rs` (6), `src/syntax/parser.rs` (5) and
`tests/chaos.rs` (5). This command counts them again for any range:

```sh
git log --format= --name-only 0e4b411..HEAD -- src tests test web bench Cargo.toml package.json .github | sort | uniq -c | sort -rn | head
```

`src/lib.rs` (the module index and the public-interface doctest),
`src/settings.rs`, `Cargo.toml`, the root `package.json` and
`.github/workflows/` are shared by many features too. Changes that run in
parallel should not edit these files at the same time. The dependency lock
files are `Cargo.lock`, `web/engine/Cargo.lock`, `package-lock.json` and
`editors/vscode/package-lock.json`; build with `--locked` so they never
change by accident.
