# Seams and a more capable language server

Status: done in 0.7.0.

## Problem Statement

turtle-lsp is fast and correct while you edit one file, but three things a
user of a large RDF project notices are missing or awkward:

- In a project whose `owl:imports` can't be loaded (no network, or a remote
  ontology the project doesn't vendor), SHACL results that could change once
  the import resolves are shown as warnings with a note. That is the right
  default, but a project that validates without its imports on purpose (its
  own tooling never loads them, as pySHACL doesn't) wants the full severity.
  Letting the import load is not the answer: the imported vocabulary then
  becomes data, and the project's own library rules start reporting errors on
  terms it doesn't own.
- A definition gives no hint of how much it is used. Finding out means asking
  for references every time.
- Problems in files that are not open stay invisible until you open them.
  `turtle-lsp check` finds them, but only from a terminal.

Two places in the code also make the next features harder than they need to
be:
- The adapter that lets the SPARQL engine (`spareval`) read our data lives
  inside the SHACL code, so SHACL owns the engine and nothing else can use it.
- `workspace/executeCommand` is handled inline in the server's request
  dispatch, with argument checking mixed into the server.

## Solution

1. Two refactors with no behaviour change: the graph and its SPARQL dataset
   adapter move to a shared module; server commands get their own module
   that parses a command into a typed value before the server acts on it.
2. A setting that treats unresolved imports as empty.
3. Reference counts above definitions, as code lenses.
4. Opt-in diagnostics for every workspace file, open or not.

None of it may slow typing: the keystroke rows of `bench/perf.mjs` stay where
they are (p50 and p95, measured with nothing else running).

## User Stories

1. As an ontology engineer whose project validates without its imports, I want a setting that treats unresolved imports as empty, so that SHACL results keep their full severity.
2. As a user of that setting, I want the unresolved-import information diagnostic to stay, so that I still know the import isn't loaded.
3. As a user who leaves the setting off, I want today's behaviour unchanged, so that results that may change keep their note.
4. As a VS Code user, I want the setting in the Settings UI with a description, so that I can find it.
5. As a Neovim or CLI user, I want the same setting in `turtle-lsp.json` and settings, so that every client behaves alike.
6. As someone reading a vocabulary, I want "N references" above each subject a document describes, so that I see how much each term is used.
7. As a VS Code user, I want clicking the lens to open the references view, so that I can go to the uses.
8. As a Neovim user, I want running the lens to list the references, so that it works there too.
9. As a user of a large file, I want the counts to cost nothing until the editor shows them, so that opening and typing stay fast.
10. As a user who doesn't want lenses, I want a setting to turn them off, so that the editor stays quiet.
11. As a user of a project with many files, I want an opt-in setting that reports problems for files that aren't open, so that the Problems panel shows the whole project.
12. As a user with that setting on, I want those diagnostics computed in the background after indexing, so that startup and typing aren't slowed.
13. As a user with that setting on, I want a closed file's problems to update when the file changes on disk or when a file it depends on changes, so that the list stays true.
14. As a user who opens a file that already had workspace diagnostics, I want the open file's live diagnostics to take over, so that I never see two sets.
15. As a user who turns the setting off, I want the closed files' diagnostics cleared, so that nothing stale remains.
16. As a maintainer, I want the SPARQL dataset adapter outside SHACL, so that other features can query workspace data.
17. As a maintainer, I want server commands parsed into a typed value in their own module, so that adding a command doesn't grow the server's dispatch.

## Implementation Decisions

- **Shared RDF data module.** The validation graph (today `shacl::graph`) and
  the `spareval` dataset adapter (today `Index`, `STerm` and the
  `QueryableDataset` implementation in `shacl::sparql`) move to a new
  top-level module, `rdf`, as `rdf::graph` and `rdf::dataset`. SHACL keeps
  everything specific to SHACL-SPARQL: pre-binding, query checks, `$PATH`,
  the batching engine. The `$shapesGraph` named graph stays a SHACL concern:
  the adapter takes an optional extra named graph instead of knowing about
  shapes. A pure move plus import updates; no logic change, no speed change.
- **Command module.** A new module defines every command name the server
  advertises and parses `ExecuteCommandParams` into an enum (today one
  variant: fetch an import). Bad arguments become `InvalidParams` errors with
  the same messages as now. The server matches the enum and performs the
  effect. Later commands (show references, run query) add a variant.
- **Unresolved imports.** New setting `imports.unresolved`, values `"soften"`
  (default, today's behaviour) and `"ignore"`. With `"ignore"`, SHACL results
  are reported at their full severity with no note; the unresolved-import
  information diagnostic and its fetch quick fix stay. Read like every other
  setting (initialization options, `workspace/didChangeConfiguration`,
  `turtle-lsp.json`, CLI); changing it revalidates.
- **Reference lenses.** `textDocument/codeLens` returns one lens per subject a
  document describes (its definitions), without a count;
  `codeLens/resolve` computes the count from the existing reference search
  and sets the title "1 reference" / "N references". The lens command is
  `turtle.showReferences` with the document URI, the position and the
  locations. The VS Code extension maps that command to
  `editor.action.showReferences`; the Neovim plugin registers it to fill the
  quickfix list with the locations. Setting `codeLens.references` (default
  true). Turtle, TriG, N-Triples, N-Quads and JSON-LD; not SPARQL. Files over
  a size limit get no lenses (same threshold style as other large-file
  guards).
- **Workspace diagnostics.** Setting `diagnostics.workspace` (default false).
  When on, after the workspace index is built the server computes diagnostics
  for every workspace file that is not open, on a background thread at low
  priority, and publishes them with `textDocument/publishDiagnostics`. A file
  changed on disk (watched files) or a change that affects other files
  recomputes the affected closed files, debounced; work is cancelled when an
  edit arrives and resumed after. Opening a file switches it to live
  diagnostics; closing it hands it back to the background set. Turning the
  setting off publishes empty diagnostics for every closed file it reported.
  The results equal what `turtle-lsp check` reports for the same file
  (same diagnostics function).
- **Performance budget.** Nothing new runs on the keystroke path. Lenses are
  computed on request, counts on resolve. Workspace diagnostics run only with
  the setting on, never block a request, and yield to edits.

## Testing Decisions

- Good tests check behaviour at the protocol and CLI level: diagnostics
  published, lens titles and commands, setting effects. Not internals.
- Refactors: the existing Rust, Node and web suites pass unchanged, and the
  benchmark's keystroke rows don't move; no test is edited except imports.
- Unresolved imports: a Rust test in the SHACL validation feature (prior art:
  the "may change once these imports resolve" tests) and a Node LSP test with
  the setting on and off; a CLI test through `turtle-lsp.json`.
- Lenses: Node LSP tests (prior art: test/lsp.test.mjs, code action tests) for
  titles after resolve, the command and its arguments, the setting, and no
  lenses for SPARQL; the VS Code suite (editors/vscode/test/suite.cjs) checks
  that the command opens references; the Neovim spec (test/nvim/spec.lua)
  checks the quickfix list.
- Workspace diagnostics: Node LSP tests with the setting on: closed files get
  the same diagnostics as `check`; opening, editing and closing a file; a
  file changed on disk; turning the setting off clears them; with the setting
  off, nothing is published for closed files.
- Performance: `bench/perf.mjs` before and after for slices that touch the
  server, with nothing else running.

## Out of Scope

- Pull diagnostics (`textDocument/diagnostic`, `workspace/diagnostic`).
- Lenses for SPARQL files (the query lens is in docs/specs/workspace-query.md).
- Checking Turtle and SPARQL inside Markdown code blocks.
- Changing the SHACL validation units or how imports load.

## Further Notes

- The comparison with pySHACL on a real design model found identical results
  except for the softened severities; `imports.unresolved: "ignore"` is the
  setting that makes them match without loading imports.
