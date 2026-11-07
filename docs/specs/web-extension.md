# VS Code for the Web: the extension in vscode.dev and github.dev

Status: done. Issue #52.

## Problem Statement

The extension does not load in vscode.dev or github.dev: VS Code shows "not
supported". It starts the native `turtle-lsp` binary as a process, which a
browser cannot do, so the manifest has no browser entry and declares virtual
workspaces unsupported. Someone who browses an ontology repository on GitHub
and presses `.` gets plain text: no highlighting from the server, no
problems, no hover, no go to definition, no formatting, no SHACL.

The RDF Editor (`web/`) already runs the whole server in the browser: the
engine (`web/engine`, WebAssembly) holds the embedded server, and the
editor's host (`web/app/lsp-host.js`) drives it. The extension does not use
any of it.

## Solution

The extension gets a browser entry. In vscode.dev and github.dev it starts
the engine in a web worker and connects VS Code's language client to it, so
Turtle, TriG, N-Triples, N-Quads, JSON-LD and SPARQL files get the server's
features as on the desktop. The extension reads the workspace's RDF files
through VS Code's file system API (which covers GitHub repositories on
github.dev) and hands them to the server, so cross-file features work:
definitions in other files, references, completion of terms declared
elsewhere, `prefixes.ttl`, SHACL over the workspace. The workspace graph
view works too. What the browser cannot do (Run query, fetching imports) is
left out and said so.

The desktop extension behaves exactly as before.

## User Stories

Opening and activation
1. As a user on github.dev, I want the extension to install and activate, so that Turtle files in a repository get language features without cloning it.
2. As a user on vscode.dev, I want the same for a folder I open from my machine or a remote repository, so that I can check RDF anywhere.
3. As a user, I want the extension's page in the Marketplace to install in the browser without a warning, so that I know it is supported there.
4. As a user, I want activation to happen when the workspace holds an RDF or SPARQL file, or when I open one, as on the desktop, so that unrelated repositories pay nothing.
5. As a user, I want the editor to stay responsive while the engine loads and the workspace is read, so that I can read files at once.

Single-file features
6. As a user, I want diagnostics (syntax errors, undeclared prefixes, lints) in an open Turtle file, so that I see what is wrong.
7. As a user, I want completion of prefixes, vocabulary terms and terms of my files, so that I type less.
8. As a user, I want hover documentation, so that I can read what a term means.
9. As a user, I want rename, go to definition, find references, document symbols and folding, so that I can move around and change a model.
10. As a user, I want formatting (document and range), so that files stay tidy.
11. As a user, I want quick fixes (add a prefix, fix a typo), so that problems are one click away.
12. As a user, I want semantic highlighting and inlay hints (labels), so that the file reads as on the desktop.
13. As a user, I want SHACL results for my data, so that validation works in the browser too.
14. As a user, I want SPARQL files to get their diagnostics, completion and hover, so that queries are checked.
15. As a user, I want JSON-LD files to be checked, so that every RDF format I keep is covered.
16. As a user, I want an untitled Turtle file to get the features too, so that I can try snippets.

Workspace
17. As a user on github.dev, I want definitions and references across the repository's files, so that a multi-file ontology works as on the desktop.
18. As a user, I want `prefixes.ttl` to supply prefixes to the files beside and below it, so that headerless files are not full of errors.
19. As a user, I want problems in files I have not opened listed in the Problems panel, so that I see the state of the whole repository.
20. As a user, I want my unsaved edits to count for the other files (their diagnostics, references), so that the workspace reflects what I am typing.
21. As a user, I want closing a file to put its saved text back into the workspace, not remove it, so that cross-file features keep working.
22. As a user, I want creating, deleting and renaming files to update the workspace, so that it never goes stale.
23. As a user, I want a `turtle-lsp.json` at the workspace root to apply in the browser as it does on the desktop, so that my project's settings travel with it.
24. As a user, I want my `turtle.*` settings to apply, and changes to them to take effect without a reload, so that I configure it once.
25. As a user with a large repository, I want the extension to read a bounded number of files and tell me when it stopped, so that the browser does not stall or run out of memory.

Views and commands
26. As a user, I want the workspace graph view, so that I can see the model in the browser too.
27. As a user, I want Restart Server to restart the engine with my open files, so that I can recover.
28. As a user, I want the Run query lens absent in the browser rather than failing, so that nothing offered is broken.
29. As a user, I want a crash of the engine to restart it with my files and tell me, so that I keep working.

Packaging and docs
30. As a user, I want one extension that works on the desktop and in the browser, so that I install it once and settings sync.
31. As a user, I want the README to list what works in the browser and what does not, so that I know what to expect.
32. As a maintainer, I want a headless test of the web extension in CI, so that the browser build cannot break unnoticed.
33. As a maintainer, I want the release to publish a web `.vsix` beside the platform ones, so that the Marketplace serves the browser.
34. As a maintainer, I want the desktop extension's behaviour and its tests unchanged, so that the change is safe for current users.
35. As a maintainer, I want the browser code to reuse the RDF Editor's host, so that a fix to one fixes both.

## Implementation Decisions

- **One extension, two entries.** The manifest gains a `browser` entry beside
  `main`. VS Code desktop keeps running `main`; vscode.dev and github.dev
  run `browser`. Virtual workspaces become supported, "limited", with a
  description of what is off.
- **Shared activation.** What both entries do (the restart command, the
  references command, the graph command and its context key, the start on
  first RDF file, the settings sent to the server, the configuration
  middleware) moves into one platform-neutral module that takes a factory
  for the language client. The desktop entry supplies a factory that finds
  the binary and starts it; the browser entry supplies one that starts the
  worker. The client handle and the settings functions move out of the
  module that finds the binary (which needs Node's `path`) into a
  platform-neutral one. The graph view drops its Node imports (the nonce
  comes from `crypto.getRandomValues`) and types the client as the common
  base client.
- **The server worker.** A web worker bundles the engine loader and the RDF
  Editor's host (`EmbeddedServer`, `plan`) unchanged, and adds a JSON-RPC
  adapter: messages from the language client arrive by `postMessage`;
  notifications go to the server, requests are answered with the client's
  own ids, cancellations drop queued requests as the RDF Editor's worker
  does, and a burst of messages becomes one engine call. Server-to-client
  requests (refreshes) are forwarded to the client; server notifications
  (diagnostics, progress, log) go through as they are. The engine's
  `.wasm` ships in the extension and is fetched relative to the worker.
- **URIs.** The server derives a file's kind, folder and `prefixes.ttl`
  scope from `file:` URIs. The worker maps each workspace folder's URI
  (`vscode-vfs://github/owner/repo`, `vscode-test-web://mount`, …) to
  `file:///workspace/<n>/` going in, and back coming out, on every URI the
  protocol carries (fields and the keys of workspace edits). Untitled
  documents keep their URIs. The mapping is a pure module.
- **Disk and editor documents.** In the browser the server's workspace is
  the documents it has open. The worker keeps two layers per URI: the disk
  text the extension read, and the editor's document when the client has it
  open. The server sees the union, the editor's text winning. Opening a
  file already known from disk becomes a full-text change; closing it puts
  the disk text back (a change) instead of closing it; a disk change to a
  file open in the editor changes nothing until it closes; a deleted file
  is closed only when the editor does not have it open. This is a pure
  module: a state and the messages each event yields, so it is tested
  without the engine. Restarting replays the union.
  Alternatives weighed: the extension opening every file through the
  client (the client owns `didOpen` and would close them again), and the
  embedded server indexing a virtual disk (a change to the server's main
  loop, which the workspace split reshapes). The worker layer needs no
  change to the server.
- **Workspace files.** The browser entry finds the workspace's files with
  the extension's own glob (the one `workspaceContains` uses, matching
  `src/workspace/kinds.rs`), excluding `node_modules`, reads them with
  `vscode.workspace.fs`, and sends them to the worker as a custom
  notification (`turtle/web/files`: changed `{uri, text}` and deleted
  URIs), which the worker handles itself and never passes to the server.
  A file system watcher keeps them current. At most 2,000 files and 50 MB
  of text are read; past either, the extension says how many it read and
  which setting-free limit stopped it.
- **Settings.** The browser entry reads `turtle-lsp.json` at the first
  workspace root (if any) and sends it under the user's settings, as the
  native server layers them; a file that does not parse is reported once
  and ignored. `turtle.server.path` does not apply in the browser.
- **Run query.** The Run query lens and command need a disk (results are
  written to the cache folder); in the browser the client drops that lens.
  The `turtle/query` request works in the worker and stays available to
  other extensions.
- **Packaging.** The extension builds three bundles: the desktop entry, the
  browser entry (`webworker` platform) and the server worker. The release
  builds a `web` target `.vsix` (no native binary, the engine's `.wasm`
  inside) beside the platform `.vsix` files, and publishes it with them.

## Testing Decisions

- A good test drives the extension or the worker from outside: messages in,
  messages out; a workspace on disk, what the editor shows. No test reads a
  module's private state.
- The URI mapping and the disk/editor layers are pure modules tested in
  Node with `node:test`, as `test/find-server.test.mjs` tests the binary
  search: every event of the layer table (open over disk, edit, close back
  to disk, disk change while open, delete while open, delete while closed,
  restart replay), and round trips of every URI field and edit key.
- The worker adapter is tested in Node against the real `engine.wasm`, as
  `web/test/lsp.test.mjs` and `web/test/host.test.mjs` test the RDF
  Editor's host: initialize, a didOpen with diagnostics, hover, a request
  cancelled while queued, a cross-file definition over a disk file, a
  crash replaying the union.
- The headless web test runs the extension in VS Code for the Web
  (`@vscode/test-web`, Chromium) on a fixture folder of several files under
  `test/fixtures/`: activation, diagnostics on an open file, hover,
  formatting, a definition in another file, `prefixes.ttl`, a problem in an
  unopened file, a closed file still found by references, and no Run query
  lens. It runs in CI's Browser job, which already builds the engine.
- The desktop extension's suites (`test/*.test.mjs`, the headless desktop
  suite `test/run.mjs`) pass unchanged, apart from import paths of moved
  functions.
- `test/manifest.test.mjs` checks the browser entry, the virtual workspace
  capability and that the browser glob is the desktop one.

## Principles

- **redesign-from-first-principles:** shared activation with a client
  factory, rather than branching the desktop entry on `typeof process`.
- **exhaust-the-design-space:** the disk/editor layer, with the two
  alternatives weighed above.
- **foundational-thinking, model-the-domain:** the layers as one state with
  an event table, not flags beside the open-document map.
- **boundary-discipline:** the worker boundary (protocol messages, URIs)
  and the file system reads (size caps, undecodable text, a bad
  `turtle-lsp.json`) validate; the layer module trusts its input.
- **make-operations-idempotent:** a restart or a repeated file
  notification converges to the same server state (the union, replayed).
- **experience-first:** the graph view in scope, Run query hidden rather
  than broken, the cap explained to the user.
- **encode-lessons-in-structure:** the manifest test ties the browser glob
  to the desktop one; the headless test guards the browser build.
- **separate-before-serializing-shared-state:** slices own disjoint files;
  the manifest and the lockfile have one owner per wave.
- **explain-the-number:** the indexing time recorded with what bounds it.

## Non-functional budgets

- The native server and the desktop extension do not change behaviour:
  every desktop suite passes. The one Rust change is in `Embedded` (the
  browser build only): once the server has ended, it reports no timer due.
- The engine's gzipped size budget (`web/build.mjs`) does not move.
- In the headless web test, the fixture workspace's diagnostics appear
  within 10 s of opening the first file, on CI.
- Reading the 201-file bench workspace through the browser entry and
  publishing its diagnostics takes under 15 s in headless Chromium on this
  machine; the close-out records the figure and what bounds it.
- The web `.vsix` is under 8 MB.

## Measured

At the close-out, on an Apple M6 with 12 cores and a load average of 1.8
to 2.7 from desktop apps, VS Code for the Web stable through test-web and
headless Google Chrome, each run with a new Chrome profile.

- **The 201-file bench workspace: 1.36 s** from page load to the last
  diagnostics published (median of 5 runs; min 1.35 s, max 1.49 s), well
  under the 15 s budget. The workspace is `bench/perf.mjs`'s shared one: 200
  files of 200 subjects and `big.ttl` of 5,000 subjects, 6.6 MB of Turtle.
  The phases (medians, from the page load): the engine loaded at 0.79 s;
  the extension active, client running, at 0.82 s; the 201 files found,
  read with `vscode.workspace.fs` and sent at 0.89 s (about 65 ms of
  reading, then one 6.6 MB message that the worker has within 1 ms); the
  diagnostics of `f199.ttl`, never opened, in the Problems panel at 1.01 s
  (the engine analyses the 200 `didOpen`s of the first notification in one
  call of 100 to 116 ms); the last diagnostics at 1.36 s. Every file read
  showed its `unused-prefix` hint, no editor was open, and the bytes sent
  equal the workspace's 6,619,137.
  What bounds it: 0.79 s goes to VS Code's own start (workbench, extension
  host, worker and engine load) before the extension does anything. Of the
  0.55 s after that, 300 ms is a timer: the 201 files change the set of
  described terms, so the server waits `DESCRIBED_DEBOUNCE` (300 ms,
  `src/server.rs`) and diagnoses every file again (45 to 55 ms, a second
  `publishDiagnostics` for each file). The rest is the engine's analysis on
  the worker's single thread. The test mount serves the files from a local
  server; on github.dev the reads go over the network and were not
  measured.
- **The web `.vsix`: 3.40 MB** (budget 8 MB), nearly all of it the engine;
  the darwin-arm64 `.vsix` is 5.95 MB.
- **The engine: 3.05 MB gzipped**, within the unchanged budget of 3.10 MB
  (`WASM_GZIP_BUDGET` in `web/build.mjs`); the extension adds no Rust code
  to it beyond the `Embedded` timer fix.
- **First diagnostics in the headless web test: about 30 ms** after opening
  the first file (budget 10 s), as the server has the fixture files from
  disk before the editor opens one.

## Out of Scope

- Run query in the browser (it writes its results to the cache folder); a
  later change can show `turtle/query` results instead.
- Fetching `owl:imports` with `fetch()`, and the fetch-import quick fix:
  imports stay unresolved in the browser, as offline ones are.
- A lower `shacl.maxTriples` for the browser, unless the headless runs
  show the default failing.
- Workspace folders added after activation get their files on the next
  restart only.
- Publishing to Open VSX (#50).

## Further Notes

- `web/app/lsp-host.js` and `web/app/engine.js` stay in `web/app`; the
  extension's bundler includes them. A fix to either reaches both hosts.
- The RDF Editor's tests are unaffected: its files do not change.

## Departures

What the implementation did differently from the plan above.

- Shared activation: "served" is `vscode.languages.match` against the document selector, so a wider browser selector also widens the start on open and the graph refresh.
- The worker's protocol code is in `src/web/handler.ts`, testable without a Worker; `src/web/worker.ts` is 27 lines.
- One Rust change: `Embedded::due_in_ms` returns `None` once the server has ended, and `EmbeddedServer` gained `stop()` (`web/app/lsp-host.js`), so the host stops ticking after `shutdown`.
- VS Code for the Web runs an extension's worker as a `blob:` worker, so the browser entry's first message to the worker is the engine's URL.
- The browser client drops the pull-diagnostics capability, so the server pushes the problems of files never opened.
- The extension, not the server, reads `turtle-lsp.json` in the browser, and copies the native rules for it (`settingsFile` in `src/client.ts`).
- Hover's "Described in" links carried the server's `file:///workspace/<n>/` URIs; `src/web/uris.ts` now maps URIs inside prose coming out and leaves document content alone.
- The headless test serves VS Code for the Web with test-web (browser "none") and opens installed Google Chrome through Playwright's `chrome` channel; `PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD` (workflow-wide in `ci.yml`) and `allowScripts` keep Playwright from downloading browsers.
- `.vscodeignore` needed no change; a platform `.vsix` drops `out/web/engine.wasm` after the build. The release's `web` job restores no Rust cache, and a smoke step unpacks the web `.vsix`, checks its files and loads the engine in Node.
- `turtle/query` (#74) without `files` found no `prefixes.ttl` in the browser: the browser build has no file paths, so the server had no root to look the query up at. The server now keeps the workspace folders' URIs from `initialize`, and the request falls back to the first one.

## Follow-ups

- `src/web/handler.ts` repeats the queue and batching of `web/app/lsp-worker.js`; one queue module in `web/app` could serve both.
- After a crash that ends "dropped" or "off", the layers still count those documents as on the server.
- Files read from disk open with languageId `""`, so a language mode chosen later in the editor does not reach the server.
- `Embedded` could take `turtle-lsp.json` as `Layer::File`; then `settingsFile` in `src/client.ts` goes.
- The files of a deleted workspace folder stay on the server until a restart.
- The file system watcher is not tested in the browser: the test-web mount reports no changes.
- The wasm build warns about thread-only code of `turtle/query` (#74).
- `tests/w3c_sparql.rs` `iri_name_and_less_than_boundaries_agree_with_spargebra` prints "skipped" without `test/corpus` and then fails; without the corpus it should skip unless `CI=true`.
