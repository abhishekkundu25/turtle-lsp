# 29b: One execution mode: `Spawn::Thread` / `Spawn::Inline`

**What to build:** running work on a thread or inline (the browser build, through
`server::Embedded`) is a real seam with two adapters, but it is an `inline: bool`
tested at about a dozen sites (audit X4-11). The field is at `src/server.rs:228`. It is
read in `configure` (`:351`, `:362`, `:366`, `:414-418`), `use_network` (`:446-457`),
`step` (`:787`, shutdown), `workspace_changed_inline` (`:1038-1043`),
`reindex_in_background` (`:1162`), `relayer_settings` (`:1404`) and `validate_shacl`
(`:2117`, the inline run with its 10 s limit). It is also read in
`src/server/run_query.rs:197-199` and `src/server/workspace_diagnostics.rs:353`, and
mirrored in `Imports::new_inline` (`src/imports/mod.rs:248`, `:303-309`). Threads start
three ways: `worker::spawn_guarded` (index, reindex, imports at
`src/imports/mod.rs:440`, the JSON-LD loader at `src/jsonld/loader.rs:560`); a bare
`std::thread::Builder::spawn` that panics when the thread cannot start, in
`shacl_validate::Worker::start` (`src/features/shacl_validate.rs:153-192`, `.expect("spawn
the SHACL thread")`) and the workspace-diagnostics worker
(`src/server/workspace_diagnostics.rs:198-223`); and a bare spawn for a query run
(`src/server/run_query.rs:224`). Add `enum Spawn { Thread, Inline }` in
`src/server/worker.rs`, held by the
server and handed to each job's constructor. `Spawn::Thread` starts through
`spawn_guarded`, and a failed start is reported like any other worker failure. Each
job keeps its own inline meaning: imports resolve without loading, SHACL runs with
`INLINE_SHACL_LIMIT`, workspace diagnostics are off, a query run shows its summary as a
message, reindexing parses at once, and contexts get no background loader. `Spawn` only
replaces the bool. **No behaviour change**, except that a SHACL or
workspace-diagnostics thread that cannot start is reported instead of panicking: SHACL
validation ends with "Validation stopped", and workspace diagnostics stop with a
warning (owner decision 4 in the README).

**Files owned:** `src/server.rs` (the sites above), `src/server/worker.rs`,
`src/server/run_query.rs`, `src/server/workspace_diagnostics.rs`, the files 29a created
under `src/server/` (where they read the mode), `src/imports/mod.rs` (the constructor
and the spawn site only), `src/features/shacl_validate.rs` (`Worker::start` only).
**Hands off:** `src/jsonld/loader.rs` (its `Network::Inline` means "fetch while the
document waits", which is another axis), `src/ide/**`, `src/workspace.rs`, the
`Embedded` interface (`start`, `handle`, `tick`, `due_in_ms` keep their signatures).

**Blocked by:** 29a.

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `rg -n "inline: bool|self\.inline|new_inline" src` finds nothing;
      `rg -n '\.expect\("spawn' src` finds nothing; the SHACL, workspace-diagnostics,
      query, imports, index and reindex workers all start through `Spawn`. Thread
      starts that are not server jobs stay as they are: the stdin reader in
      `server::run`, `src/main.rs`, `src/fetch.rs`, `src/sparql/format.rs` and the
      tests.
- [ ] New unit tests in `src/server/worker.rs`: `Spawn::Inline` runs the job before it
      returns and delivers its answer; `Spawn::Thread` delivers a panic as a message
      (as `a_panic_is_delivered_as_a_message_and_a_result_as_itself` does). A SHACL
      worker whose start fails ends the validation progress with a message (an
      injected start failure under `cfg(test)`).
- [ ] Green: `cargo test --release --locked --lib`, `--test server_lifecycle`;
      `test/resources.test.mjs` (idle CPU, no orphan servers, exit paths),
      `test/query-lsp.test.mjs`, `test/shacl-validate.test.mjs`,
      `test/imports.test.mjs`, `test/workspace-diagnostics.test.mjs`; `web/test`
      (`host.test.mjs`, `engine.test.mjs`, `lsp.test.mjs`, `editor-query.test.mjs`)
      after `node web/build.mjs`; `cargo fmt --check`, clippy.
