# Fetch one import on request, and open cached imports

Status: in progress · 2026-10-18 · Branch: `fetch-import-quick-fix`

## Problem Statement

An `owl:imports` of a web ontology that is not in the workspace, a catalog or
the import cache gets an information diagnostic telling the user to write a
`catalog-v001.xml` entry or turn on `turtle.imports.network`. The setting is
off by default, and should stay off: with it on, opening any file makes the
editor send requests to every URL that file imports. So a new user gets no
imported terms until they find the setting, and turning it on to get one
ontology allows every ontology.

Once an import is cached, its analysis is named by its URL so that hover can
say where a term came from. Every location built from that analysis
(definition, type definition, implementations, references, type hierarchy,
SHACL related information) therefore points at an `http://` URI that editors
cannot open.

## Solution

1. The unresolved-import diagnostic on an `http(s)` import gets a quick fix,
   "Fetch <iri> and cache it". Choosing it fetches that one import (and only
   that one: its own imports follow the usual rules), caches it, and resolves
   it, with `turtle.imports.network` off.
2. An import loaded from the cache is named by its cache file, so every
   location into it opens the cached copy. Hover still names the URL it came
   from.
3. A cached copy opened in the editor is a read-only view: it gets no
   diagnostics and does not join the knowledge index a second time.

## User Stories

1. As a new user, I want one click on the unresolved-import diagnostic to load
   that ontology, so that its terms work without finding a setting.
2. As a cautious user, I want only the import I clicked to be fetched, so that
   no other URL in the workspace is contacted.
3. As a user whose fetch failed, I want the diagnostic to say why and to offer
   the fix again, and clicking it again to try again at once.
4. As a user, I want go-to-definition on a term from a cached import to open
   the cached file at that term.
5. As a user, I want hover to keep naming the URL a term came from, not a hash
   file name in a cache folder.
6. As a user who opens a cached copy, I want no warnings in it and no
   duplicate definitions in the rest of the workspace.

## Implementation Decisions

### Fetch on request (`src/imports.rs`)

- `Imports` keeps a session set of IRIs the user asked to fetch. It is not
  saved: once fetched, the cache serves the import in later sessions anyway.
- `Request` gains `fetch: Vec<String>` (sorted, unique). `load` allows a fetch
  when `req.network` is on **or** the import IRI (compared by `norm`) is in
  `req.fetch`. Nothing else about fetching changes: same size, time and
  redirect limits, same cache write.
- `pub fn fetch(&mut self, iri: &str)` adds the IRI and makes the next `sync`
  send a job (as `retry` does). The job also carries the IRI as `refetch`
  (outside `Request`, so equality is unchanged); the worker removes that URL
  from `memo.failed` before resolving, so a click retries at once instead of
  waiting out `RETRY_FETCH_AFTER`.
- `pub const FETCH_COMMAND: &str = "turtle.fetchImport"`, arguments
  `[iri]`.
- `pub fn fetch_actions(a, enc, params: &CodeActionParams) ->
  Vec<CodeActionOrCommand>`: for each diagnostic in `params.context.diagnostics`
  with code `unresolved-import`, find the `owl:imports` triple of `a` whose
  object range equals the diagnostic range; if its IRI is `http(s)`, return a
  `quickfix` CodeAction titled `Fetch <iri> and cache it`, with `diagnostics`
  set to that diagnostic and `command` = `FETCH_COMMAND` with `[iri]`. No edit.
  Respects `context.only` the way the existing quick fixes do.
- The unresolved message for web IRIs names the quick fix as well as the
  catalog and the setting.
- **Trust:** the fix is offered in untrusted VS Code workspaces too. It fetches
  one URL that the user chose by clicking an action whose title shows the URL,
  which is the same as following a link. The automatic setting stays
  restricted.

### Cached imports are named by their file (`src/analysis.rs`, `src/imports.rs`, `src/features/hover.rs`)

- `Analysis` gains `pub origin: Option<String>`: the URL an import's text came
  from, when `uri` names a cached copy. `None` everywhere else.
- A cache hit loads with `uri` = the cache file's `file://` URI and `origin` =
  the URL. A fresh fetch whose cache write succeeded does the same; if the
  write failed (or there is no cache directory) `uri` stays the URL and
  `origin` stays `None`, as today.
- `Status::Loaded` keeps the URL.
- Hover's "defined in" shows `origin` when set (with its line), else the file
  name, else the URI, as today.

### Cached copies opened in the editor (`src/workspace.rs`, `src/server.rs`)

- An open document whose path is inside `imports::cache_dir()/imports` is
  analysed as usual (tokens, folding, symbols, hover work) but is not
  `knowledge.set` as `Source::Open`, and `diagnostics_for` returns an empty
  list for it.

### Wiring (`src/server.rs`)

- Advertise `execute_command_provider` with `[FETCH_COMMAND]`.
- `CodeActionRequest` adds `imports::fetch_actions` to the result.
- `workspace/executeCommand` with `FETCH_COMMAND` calls `imports.fetch(iri)`,
  then the usual sync; replies `null` at once. The answer arrives through the
  existing results path, which republishes diagnostics. Unknown commands get
  an `InvalidParams` error.

## Testing Decisions

- Unit (in `src/imports.rs`): `fetch` makes a job with `network: false`
  fetch only the listed IRI (the other web import stays unresolved); a listed
  IRI with a fresh `memo.failed` entry is tried again when it is the job's
  `refetch`; cache hit sets `uri` to a `file://` path under the cache and
  `origin` to the URL; `fetch_actions` returns the action only for `http(s)`
  imports with a matching diagnostic.
- Unit (hover): a term from an analysis with `origin` shows the URL.
- End-to-end (`test/imports.test.mjs`, existing local HTTP server and
  `TURTLE_LSP_CACHE_DIR`): with the setting off, the code action is offered on
  the diagnostic; executing its command fetches exactly one URL (request log),
  the diagnostic clears and the term resolves; definition on that term returns
  a `file://` location under the cache dir; opening that file publishes no
  diagnostics and definition still returns one location; a failing URL keeps
  a diagnostic with the fetch error and the fix, and executing again makes a
  second request at once.
- Protocol (`test/protocol.test.mjs` or wherever capabilities are asserted):
  `executeCommandProvider.commands` contains `turtle.fetchImport`.

## Out of Scope

- Document links (`textDocument/documentLink`).
- Remembering fetch approvals across sessions, or a "fetch all" action.
- Changing the default of `turtle.imports.network`.

## Slices

| Slice | Files | Blocked by | Acceptance |
|---|---|---|---|
| 1 `import-core` | `src/imports.rs`, `src/analysis.rs`, `src/features/hover.rs` | — | Unit tests above; `cargo test --release --locked --lib` green; clippy clean |
| 2 `cache-docs` | `src/workspace.rs`, `src/server.rs` (only `diagnostics_for` and the open path) | — | Unit test: an open doc under the cache dir is not in the knowledge index and gets no diagnostics |
| 3 `wiring` | `src/server.rs`, `test/imports.test.mjs`, `test/protocol.test.mjs`, `README.md`, `editors/vscode/README.md`, `CHANGELOG.md` | 1, 2 | End-to-end tests above green; docs describe the fix |

Full validation (`npm test`, `npm run bench`, clippy, idle CPU and no stray
server processes) runs once all three have landed.
