# RDF Toolbox editor mode (iteration 2)

Status: done; merged to main and published with the browser editor in 0.5.1.

## Problem Statement

The toolbox (iteration 1) checks and formats files, but working on an
ontology or a dataset means editing it: moving between files, following a
term to its definition, renaming it everywhere, fixing problems as they
appear. People do that in VS Code with the turtle-lsp extension. They want
the same in a browser tab, with nothing to install and nothing uploaded,
and with the files of a whole folder.

## Solution

An editor page (`editor.html`, linked from the toolbox) laid out like VS
Code (the "Code" layout chosen from mock-ups): an activity bar, an
explorer with a folder tree, editor tabs with breadcrumbs, the Monaco
editor, a bottom panel with the problems, and a status bar. The full
turtle-lsp language server runs in a Web Worker as WebAssembly and speaks
LSP to the editor, so hover, completion, go to definition, references,
rename, quick fixes, formatting, semantic colouring, outline, folding and
label hints are the server's own. Files come in from the disk (files, a
folder, drag and drop, or a folder opened for editing in place in browsers
that allow it) and go out as a file, a zip of the workspace, or saved back
in place. The workspace is kept in the browser between visits.

## User Stories

1. As a user, I want the editor to open on a sample workspace with folders, so that I see what it does at once.
2. As a user, I want a folder tree like VS Code's explorer, so that I can find files in a project.
3. As a user, I want to create, rename and delete files and folders in the tree, so that I can organise the workspace.
4. As a user, I want to open files from my disk into a chosen folder, so that I can add to the workspace.
5. As a user, I want to open a whole folder with its subfolders, so that a project keeps its structure.
6. As a user, I want to drop files and folders on the tree, so that importing needs no dialogs.
7. As a user, I want to open a folder for editing in place (where the browser allows), so that Ctrl+S saves to my disk.
8. As a user, I want to download the current file, so that I can keep it.
9. As a user, I want to export the whole workspace as a zip with its folders, so that I can take my work elsewhere.
10. As a user, I want the workspace kept in this browser between visits, so that a reload loses nothing.
11. As a user, I want to start over with the sample or an empty workspace, so that I can clear it.
12. As a user, I want tabs with a marker for unsaved changes, so that I know what is edited.
13. As a user, I want problems underlined as I type, with the counts in the status bar, so that I see mistakes at once.
14. As a user, I want a Problems panel listing every problem of every file, SHACL included, so that I can work through them.
15. As a user, I want clicking a problem to open its file at its place, so that I can fix it.
16. As a user, I want quick fixes from the lightbulb and Ctrl+., so that I can fix a problem in one step.
17. As a user, I want a "Fix all" command, so that I can apply every preferred fix.
18. As a user, I want hover to show a term's label, comment and where it is defined, so that I understand it.
19. As a user, I want completion of prefixes, terms and keywords, so that I type less and misspell nothing.
20. As a user, I want go to definition, peek and find references across files, so that I can navigate the model.
21. As a user, I want to rename a term across all files with F2, so that refactoring is safe.
22. As a user, I want Format Document, so that files share one layout.
23. As a user, I want an outline and breadcrumbs, so that I can move within a large file.
24. As a user, I want a labels toggle that shows rdfs:label next to IRIs, so that I can read a model by its names.
25. As a user, I want SPARQL files edited with the same help (completion, diagnostics, rename of variables, formatting), so that queries are first-class.
26. As a user, I want the editor commands in the command palette (F1), so that I find them by name.
27. As a user, I want the page to keep working if the engine crashes (it restarts with my files), so that one bad input does not lose my work.
28. As a user, I want nothing sent anywhere, so that I can edit private data.
29. As a maintainer, I want the browser to run the same server code as the editors, so that the two never disagree.
30. As a maintainer, I want the native server's behaviour unchanged, so that editors are not affected.

## Implementation Decisions

- **The server, driven step by step.** The server's main loop is split into
  a batch step (handle the messages queued, then publish diagnostics, sync
  imports, refresh) and timer events (the described-set pause, SHACL after
  typing). The native loop keeps waiting on its channels and calls the same
  step. A new embedded driver owns a server on an in-memory connection: the
  host hands it messages, it runs one step and returns what the server sent;
  `tick` fires due timers; it says when the next timer is due. In the
  embedded driver the workspace has no roots and is indexed at once, SHACL
  validation runs inline, and imports are resolved inline without loading
  anything (an import is present when the workspace declares it, otherwise
  unresolved with a note saying imports are not followed here).
- **Clock.** Timers read a clock that is the system's natively and the
  page's (`Date.now()`) in the browser build, where the system clock is
  missing.
- **Engine operations.** The engine gains `lsp_start` (initialize params →
  the initialize response), `lsp` (messages → messages the server sent, and
  when to tick next) and `lsp_tick`. The analyse/fix_all operations of the
  toolbox stay.
- **Editor page.** Monaco 0.52.2 from the npm package, copied next to the
  page at build time (no CDN). A bridge of Monaco providers maps each
  feature to its LSP request; diagnostics become markers; server requests
  (refreshes) are answered and acted on. Document URIs are
  `file:///work/<path>`.
- **Workspace model.** Files by path (folders are path prefixes, plus empty
  folders kept explicitly), each with text, saved text and an optional
  file handle when opened in place. Kept in IndexedDB as one record per
  file (with a revision) and one for the layout (folders, tabs, settings);
  only what changed is written, 300 ms after a change to a file or the
  layout, and `body[data-saved]` says when that is done. One tab owns the
  workspace (a Web Lock); a second tab shows it read-only with a notice,
  and takes over when the first closes.
- **Content Security Policy.** Both pages carry one in a `<meta>`: scripts
  from the page's own origin (the two inline scripts of the editor page by
  hash), workers from the origin, no objects, no base URL; the editor page
  may fetch an import from any http(s) address (the quick fix).
- **Export.** One file as a download; the workspace as a zip written in the
  page (stored, no compression).

## Testing Decisions

- **Engine LSP parity** (Node): the same documents and requests sent to the
  native server and to the embedded server in the built module give the
  same answers (URIs mapped): diagnostics (SHACL included), hover,
  completion, definition, references, rename, formatting, code actions,
  document symbols, folding, semantic tokens, inlay hints, highlights.
- **Browser** (Playwright, installed Chrome): the editor opens on the sample
  tree with no console errors and no outside requests; editing shows
  problems; hover, completion, quick fix, rename and format work through
  Monaco; Problems panel jumps; explorer create/rename/delete; open a
  folder; export a file and a zip (the zip's entries checked); reload keeps
  the workspace; the labels toggle shows hints.
- Native suites unchanged and green.

## Out of Scope

- Running SPARQL, the SHACL grid and "why" view, the graph view, graph
  diff and the health report from the mock-ups (iteration 3).
- Convert.
- Shareable links (rejected).
- Phone layout for the editor (the toolbox stays the phone page).

## Departures from the plan

- Embedded, opening or closing a document schedules SHACL validation (as a
  pause in typing does): the open documents are the whole workspace, with
  no disk index to validate after.
- Hover and completion name a definition's file from its URI when there is
  no file path (the browser build), so they read "core.ttl:18" as natively.
- The note on an unloaded import is shared by the toolbox and the editor.
- Format Document uses the file's indent as Monaco detects it, as VS Code
  does.
- "Open Folder in Place" adds the folder to the workspace instead of
  replacing it, so nothing kept in the browser is lost. Its file handles are
  not kept across a reload: after one, its files are browser copies.
- Monaco's word pattern must not match an empty string (it hangs the page);
  noted at the pattern.

## Verification (2026-10-19)

- LSP parity (Node, the built module against the native server): 12/12 —
  diagnostics with SHACL, hover, completion and resolve, definition,
  references, highlights, rename, formatting, code actions, outline,
  folding, semantic tokens, label hints, type hierarchy, errors.
- Editor (Playwright, installed Chrome): 10/10, no console errors, no
  outside requests (Monaco is served from the page's own copy).
- Toolbox and engine: 11/11 and 24/24.
- Native: `cargo test` 195 passed, clippy clean on both crates, Node
  end-to-end 490/490 (Neovim included). Benchmark: the server before and
  after the main-loop split, run alternately, measure the same.
- Engine 8.3 MB uncompressed; Monaco 14 MB, copied at build time.
- Resource hygiene: no processes or temp directories left by the work.
