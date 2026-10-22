# 13: Server isolation and index consistency

**What to fix:** the "each request is isolated" promise and the disk index.

Isolation:
- `guard` wraps handlers, but a `didChange` only clears the cached analysis; the parse
  happens later in `step()` → `check_described()` → `ws.knowledge()`, outside any guard,
  as do `sync_imports`, `validate_shacl` scheduling, `merge_index` → `add_disk` →
  `DocFacts::new` and `context_fetched` (`src/server.rs:631-668, 663, 709, 958,
  1178-1195`). A parser panic on an edited or indexed file ends the process (exit 101).
- The imports worker has no `catch_unwind` and `Imports` keeps its own `results_tx`,
  so a dead worker leaves `busy()` true forever (progress never ends); the initial-index
  thread's panic becomes "0 files" with no message; the JSON-LD fetch thread likewise
  (`src/imports.rs:239-272, 405-413`; `server.rs:389-394, 1178-1195`;
  `src/jsonld/loader.rs:372-381`). wsdiag, SHACL and run-query do it right.
- The main loop runs on the process main thread with the OS default stack (1 MiB on
  MSVC Windows); a 255-level SPARQL document is within 2× of overflowing it
  (`server.rs:371-413`, `main.rs:24`). An overflow cannot be caught.
- Timers and worker channels are spread over a dozen fields with six disconnect
  policies (`server.rs:538-600, 671-698`).

Index consistency (`src/workspace.rs`, `server.rs:868-913`):
- A folder delete/rename leaves its files indexed (the watcher skips non-file paths).
- Watched events bypass `SKIP_DIRS`, hidden dirs, `MAX_FILES` and the roots.
- Closing a file outside the roots re-reads it from disk and keeps it indexed.
- Changes before the initial index merges are undone by the merge.
- The watcher glob omits `.rq/.sparql/.ru` and `.json` contexts.
- `prefix_cache` is keyed by directory, and any `prefixes.ttl` save re-parses every disk
  file synchronously on the main loop.
- `shutdown` without `exit` joins a reader blocked on stdin forever.
- Inline mode (browser) runs SHACL with cancellation `|| false` (ticket 06 leaves the
  `server.rs:1832-1848` call site to this ticket).

**Files owned:** `src/server.rs`, `src/main.rs`, new `src/server/timers.rs`, new
`src/server/worker.rs`, `src/workspace.rs` (`indexable(path)`, `remove_under(dir)`,
touched-path record before merge, prefix cache keyed by file path + mtime, scoped
re-parse off the main loop), `src/imports.rs` (spawn site and `results_tx` only),
`src/jsonld/loader.rs` (spawn site only), new `tests/server_lifecycle.rs`,
`test/lsp.test.mjs`, `test/resources.test.mjs`, `test/protocol.test.mjs`.
**Hands off:** `src/server/run_query.rs` bodies, `src/features/**`, `src/cli/**`.

**Blocked by:** wave 1. First holder of the `src/server.rs` and `src/workspace.rs` locks
in wave 2.

**Status:** ready-for-agent

- [ ] `main_loop` and `cli::run` run on a spawned thread with a 64 MiB stack, joined
      from `main`. Test: a 255-level SPARQL document answers `documentSymbol` with
      `ulimit -s 512` (skipped on Windows).
- [ ] The post-batch phase of `step()` is guarded, or `Workspace::knowledge()`/
      `analysis()` catch a per-document panic and store an "internal error" analysis
      that shows as a diagnostic. Test: an injected analysis panic (cfg(test) hook)
      keeps the server alive, via `Embedded` in `tests/server_lifecycle.rs`.
- [ ] One `spawn_guarded` helper for the index, imports, SHACL, wsdiag, query and
      context threads; one disconnect rule; `Imports` drops its own `results_tx`; a dead
      worker ends progress with a message; the index thread reports failure instead of
      0 files. `Timers` keyed by an enum used by `main_loop` and `Embedded`.
- [ ] `Workspace::indexable(path)` (under a root, no skipped/hidden component, ≤
      `MAX_FILE_BYTES`) used by the scan, `reindex_disk_file`, `close_doc` and the
      watcher; `remove_under(dir)` on a Deleted folder event; a watcher for folders;
      pre-merge touched paths dropped or re-parsed in `merge_index`; glob adds
      `*.rq,*.sparql,*.ru,*.json`; prefix cache by file; a `prefixes.ttl` change
      re-parses only files it serves, off the main loop.
- [ ] After the shutdown timeout, exit the process instead of joining the reader.
      Inline SHACL gets a budget (the `Budget` from ticket 06).
- [ ] `tests/server_lifecycle.rs` drives `Embedded` for: watched folder delete,
      `node_modules` event ignored, close outside roots leaves no entry, pre-merge
      delete, settings layering, guard behaviour. `test/resources.test.mjs` (idle CPU,
      exit paths), `test/lsp.test.mjs`, `test/protocol.test.mjs`,
      `test/workspace-diagnostics.test.mjs`, `web/test/host.test.mjs` green.
