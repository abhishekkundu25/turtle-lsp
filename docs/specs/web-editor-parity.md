# RDF Editor: VS Code parity (iterations 3–5)

Status: batches 1, 2 and 3 done; merged to main and published with the browser editor in 0.5.1.
Builds on `docs/specs/web-editor.md`.

## Problem Statement

The RDF Editor runs the full turtle-lsp server in the browser and looks like
VS Code, but people who live in VS Code hit missing basics within minutes:
no search across files, no jump to a symbol by name, `prefixes.ttl` ignored,
files that cannot be moved, no settings, unsaved-change handling only half
there, one editor at a time, no diff, no type hierarchy, imports that cannot
be fetched. Large files lag because every keystroke sends the whole text.
Some paths (folder drop, saving in place, crash recovery, browsers other than
Chrome) have never been checked.

## Solution

Close the gaps in three batches, each shipped and fully verified before the
next starts.

- **Batch 1 (day-one parity):** search and replace across files, go to
  symbol in the workspace and in the file, `prefixes.ttl` honoured,
  incremental text sync, move files by dragging, Copy Path, folding in
  Problems.
- **Batch 2 (settings and saving):** a Settings view for the turtle-lsp and
  editor settings; VS Code's save semantics for files opened in place; a
  split editor.
- **Batch 3 (views and coverage):** a diff view, a type hierarchy view, the
  fetch-import quick fix, and the untested paths and browsers checked.

## User Stories

Batch 1
1. As a user, I want Ctrl+Shift+F to open a Search view, so that I find text in every file.
2. As a user, I want case, whole-word and regex options, so that searches are exact.
3. As a user, I want results grouped by file with the matching line shown, so that I can scan them.
4. As a user, I want clicking a result to open the file at the match, so that I can act on it.
5. As a user, I want to replace one match or all matches, so that I can make a change everywhere.
6. As a user, I want a replace to be undoable per file, so that a mistake costs nothing.
7. As a user, I want results to follow my edits, so that the list is never stale.
8. As a user, I want Ctrl+T to find a term defined anywhere in the workspace by name, so that I jump to it.
9. As a user, I want Ctrl+Shift+O to find a symbol in the current file, so that I move within a large file.
10. As a user, I want a `prefixes.ttl` in a folder to supply prefixes to the files under it, as in VS Code, so that my project behaves the same in both.
11. As a user, I want editing `prefixes.ttl` to update the other files' problems, so that the effect is visible at once.
12. As a user, I want typing in a large file to stay smooth, so that the editor is usable for real data.
13. As a user, I want to drag a file or folder onto another folder to move it, so that I can reorganise.
14. As a user, I want a moved file to keep its tab, text and problems, so that nothing is lost.
15. As a user, I want Copy Path and Copy Relative Path, so that I can paste a file's location.
16. As a user, I want to fold a file's problems in the Problems panel, so that I can focus on one file.

Batch 2
17. As a user, I want a Settings view (Ctrl+,), so that I can change how the editor and checks behave.
18. As a user, I want the turtle-lsp settings VS Code offers (format line width, SHACL on and on typing, label hints opaque/all/off, extra prefixes, the undescribed-term check), so that I get the same behaviour.
19. As a user, I want editor settings (tab size, insert spaces, word wrap, minimap, font size), so that it feels like my editor.
20. As a user, I want settings kept between visits, so that I set them once.
21. As a user, I want a dot on tabs of files whose text differs from the disk (opened in place), so that I know what is unsaved.
22. As a user, I want Save All (Ctrl+K S) and Revert File, so that I can save or discard everything.
23. As a user, I want a warning before closing a tab with unsaved changes, so that I do not lose work.
24. As a user, I want a folder opened in place to stay linked after a reload, so that Ctrl+S keeps saving to disk (the browser may ask permission once).
25. As a user, I want to split the editor (Ctrl+\) and see two files side by side, so that I can compare and edit both.
26. As a user, I want each side to have its own tabs, so that the split works like VS Code's groups.

Batch 3
27. As a user, I want to compare a file with its version on disk, so that I can review before saving.
28. As a user, I want to preview formatting as a diff before applying it, so that I can trust it.
29. As a user, I want a Type Hierarchy view of a class's super- and subclasses, so that I can see where it sits.
30. As a user, I want the "Fetch and cache" quick fix on an unresolved import, so that the imported ontology's terms become known.
31. As a user, I want a fetched import to appear as a file in the workspace, so that I can see what was loaded.
32. As a user, I want the editor to work in Safari and Firefox (Open Folder in Place only where supported), so that my browser does not matter.
33. As a user, I want a crash to restart the engine with my files, so that I keep working.

Throughout
34. As a maintainer, I want every batch verified in full before the next, so that regressions are caught where they start.
35. As a maintainer, I want the native server's behaviour unchanged, so that editors are not affected.

## Implementation Decisions

- **Search** works on the editor's models in the page (each file has one),
  using Monaco's own find on each model: case, whole word and regex as in
  VS Code. Replace edits the models with undoable edits, so the language
  server hears it as typing.
- **Go to symbol:** the workspace symbol request the server already answers
  feeds a Ctrl+T picker; the file symbol picker is Monaco's quick outline
  over the existing document-symbol provider, bound to Ctrl+Shift+O.
- **`prefixes.ttl` in the browser:** where a document has no file path, the
  workspace finds the nearest `prefixes.ttl` by walking up the document's
  URI among the open documents. The native lookup (by path, on disk) is
  unchanged. Opening, editing or closing a `prefixes.ttl` re-resolves the
  others as natively.
- **Incremental sync:** the editor sends each change's range and text; the
  page's host applies the same edits to its copy of the open documents so
  it can replay them after a crash. The server already accepts ranged
  changes.
- **Moving files:** drag and drop inside the explorer moves a file or folder
  (the move already used by rename: close and reopen under the new URI,
  tabs and view state kept). Dropping from the desktop still imports.
- **Settings view:** a page of the editor bound to two groups: turtle-lsp
  settings sent as `workspace/didChangeConfiguration` (the keys the server
  reads), and editor options applied to every editor. Kept with the
  workspace.
- **Save semantics:** a file is dirty when it has a disk handle and its text
  differs from the disk text. The directory handle is kept in browser
  storage; after a reload the first save asks the browser for permission.
  Closing a dirty tab asks in the page (Save, Don't Save, Cancel).
- **Split editor:** up to two editor groups, each with its own tabs, sharing
  the one model per file.
- **Diff view:** Monaco's diff editor over the file and either its disk
  text or its formatted text (from the formatting request); Apply takes the
  right side.
- **Type hierarchy view:** a panel tree over the type hierarchy requests the
  server answers.
- **Fetch import:** the server's existing fetch quick fix is a command; in
  the browser the page runs it by fetching the import with `fetch()`
  (Turtle, N-Triples, TriG; others are refused with a reason) and adds the
  result as a file under an `imports/` folder, which makes the import
  present in the workspace. Sites that refuse cross-origin requests get a
  message saying so.

## Testing Decisions

- Tests check behaviour from outside: what the user sees and what the
  server answers. Three seams, all existing:
  1. **The editor in a real browser** (Playwright, installed Chrome): every
     user-visible feature of every batch.
  2. **LSP parity** (Node, the built module against the native server):
     server-side changes, i.e. `prefixes.ttl` resolution and ranged
     changes, must answer as natively.
  3. **The page's LSP host in Node with a stand-in engine** that crashes on
     demand: crash recovery replays the open documents.
- Per slice: only the slice's own tests (targeted). Per batch: everything —
  web suites, native Rust and Node end-to-end, clippy on both crates, the
  benchmark compared with the previous commit, resource hygiene.
- Batch 3 adds runs in Playwright's WebKit and Firefox for the editor tests
  that do not need Chrome-only APIs.

## Out of Scope

- The RDF-specific views (running SPARQL, SHACL grid and "why", graph view,
  graph diff, health report) and Convert: the iteration after these.
- Multi-root workspaces, extensions, terminals, git.
- Shareable links (rejected).

## Further Notes

- Batch 1 slices are file-disjoint where possible; `editor.js` is the lock
  most slices need, so its slices land one at a time.
- Each batch ends with one commit per slice on `web-showcase` and a
  verification record in this file.

## Batch 1 record (2026-10-19)

Slices: `prefixes-uri` (agent; workspace and LSP parity tests), `search`
(agent; Search view module, its styles and tests, small wiring), and the
editor.js lock work done by the orchestrator (ranged sync, symbol search,
drag to move, Copy Path, Problems folding, crash replay test). All three
landed without conflicts.

Departures from the plan:
- Ctrl+T cannot open symbol search in Chrome (the browser keeps it). As in
  VS Code's Quick Open, Go to File takes `#` (workspace symbols) and `@`
  (file symbols); Ctrl+T is bound for browsers that pass it through, and
  Ctrl+Shift+O is Monaco's outline.
- Natively only the nearest `prefixes.ttl` applies (a parent folder's is not
  added to a subfolder's); the browser does the same.
- Closing a `prefixes.ttl` in the browser drops its prefixes (there is no
  disk copy to fall back to), as deleting it would natively.
- Copy Path copies the document URI (`file:///work/...`); Copy Relative
  Path the workspace path.
- Crash recovery is tested in Node with the real engine wrapped to crash on
  demand (planned for batch 3; it fell out of the ranged-sync work).

Verification on the merged branch:
- Web: engine 24, toolbox 11, LSP parity 13, host 1, editor 15, search 6:
  70/70.
- Native: `cargo test` 195 passed, clippy clean on both crates, fmt clean,
  Node end-to-end 490/490 (Neovim included); benchmark in line with the
  previous runs (didOpen 18.8 ms, keystroke 4.3 ms, documentSymbol 11.2 ms).
- Resource hygiene: no worktrees, slice branches, processes or temp
  directories left.

## Batch 2 record (2026-10-19)

Slices: `settings` (agent; Settings module, its styles and tests, wiring)
and the editor.js lock work done by the orchestrator (save behaviour for
folders opened in place, split editor). The settings slice conflicted in
editor.js (state, persistence, start-up) and was resolved by keeping both.

Departures from the plan:
- A file's on-disk state comes from the folder opened in place (its handle
  is kept with the workspace), not from a handle per file; saving finds or
  creates the file's handle. A file moved or renamed within such a folder
  is new at its new path when saved; the old file stays on disk, and
  deleting a file removes it from the workspace only.
- The status-bar Labels button names the mode (Opaque / All / Off) and
  switches between All and the other chosen mode; the Settings select and
  the button stay in sync.
- After an engine crash the host replays the last settings with the
  documents (found in review), and Ctrl+S sends `didSave` for every file,
  so SHACL validates on save when "Validate While Typing" is off.
- Ctrl+\ is also handled by the page (headless Chrome does not deliver it
  to Monaco's binding); splitting twice only refocuses the second group.
- Enter after naming a new file no longer types a newline into it (found
  by the Save All test).
- The Search and Settings tests focus a group's editor (the single
  `#editor` container became groups).

Verification on the merged branch:
- Web: engine 24, toolbox 11, LSP parity 13, host 1, editor 20, search 6,
  settings 7: 82/82. One split-test failure seen once while another slice
  ran Chrome in parallel; 7 later runs green.
- Native: no Rust or engine change in this batch (diff against the batch 1
  record is empty), so the batch 1 native results stand.
- Resource hygiene: no worktrees, slice branches, processes or temp
  directories left.

## Batch 3 record (2026-10-19)

Slices: `diff` (agent; diff view module, styles, tests), `hierarchy`
(agent; Type Hierarchy view module, styles, tests) and the orchestrator's
part (fetch import, folder drop test, WebKit and Firefox runs). Both slices
conflicted only on adjacent new lines (imports, a stylesheet link).

Departures from the plan:
- Crash recovery was already covered in batch 1 (host test).
- A fetched import gets a first line declaring its ontology IRI, which is
  what makes the import present in the browser; RDF/XML and other formats
  are refused with the reason, as are fetches the site does not allow.
- Folder drop is tested with a stand-in for the browser's FileSystemEntry
  objects (Playwright cannot drag a real folder); this exercises the page's
  walk, not the browser's.
- Preview Formatting's Apply refuses when the file changed after the
  preview (found in review), so it never writes formatting over newer
  edits. Compare with Saved does not refresh its disk side after a save;
  the diff view and Settings both cover the editor area and can stack.
- The diff editor has inlay hints, semantic and occurrence highlighting
  off (Monaco reports cancelled requests of a closed diff editor as page
  errors).
- Firefox: Playwright's Firefox build does not start inside the sandboxed
  shell used for this work (exits with code 1); running it outside the
  sandbox was not permitted, so the Firefox run is left to
  `npm run web:test:browsers` on a normal shell. WebKit runs here.
- Saving to a folder in place needs showDirectoryPicker (Chromium only);
  those tests skip in other browsers, where the page hides the button.

Verification on the merged branch:
- Chrome: engine 24, toolbox 11, LSP parity 13, host 1, editor 23, search
  6, settings 7, hierarchy 4, diff 2: 91/91.
- WebKit: toolbox 11/11, editor 19 passed and 4 skipped (folder in place).
- Native: no Rust or engine change in batches 2 and 3; batch 1's native
  results stand (cargo test, clippy, Node end-to-end 490/490, benchmark).
- Resource hygiene: no worktrees, slice branches, test processes or temp
  directories left.
