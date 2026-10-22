# 29a: Server state groups

**What to build:** `Server` is one struct of about 45 fields (`src/server.rs:137-240`).
Its submodules add `impl Server` blocks that read and write those fields directly
(`src/server/run_query.rs:154`, `src/server/workspace_diagnostics.rs:350`), so the file
split gives no interfaces (audit X4-11). Waves 1 and 2 keyed the timers
(`src/server/timers.rs`) and gave threads one start (`src/server/worker.rs:32`,
`spawn_guarded`). After 27b and 28a-c, the workspace owns invalidation and
`ide::Session` owns the requests, so what is left is server state. Group it into owned
structs with methods, each in its own file under `src/server/`:
- **progress** (`progress.rs`): `work_done_progress`, `index_progress`,
  `imports_progress`, `shacl_progress`, `progress_pending`, `progress_live`, with
  `begin`, `accepted` and `end` (`progress_begin`, `progress_accepted`,
  `progress_end`, `send_end` and `send_progress` today, `:1263-1339`).
- **SHACL scheduling** (`shacl.rs`): `shacl`, `shacl_feed`, `shacl_generation`,
  `shacl_diagnostics`, with `validate_shacl`, `shacl_outcome` and `shacl_gone`
  (`:2078-2160`, `:743-750`), plus `shacl_stopped` and `violation_summary`
  (`:2290-2351`) and their test.
- **refresh** (`refresh.rs`): `pull`, `refresh_support`, `generation`, `reported`,
  `refresh_needed`, `display_refresh`, `display_stale`, with the rules from
  `knowledge_changed`, `refresh_if_needed` and the result id of `pull_diagnostics`.
- **settings layers** (`layers.rs`): `file_settings`, `file_settings_error`,
  `init_options`, `config`, `settings`, `config_pull`, `config_request`, with
  `set_config`, `reload_settings_file` and a `relayer` that returns what changed
  (prefixes, network, anything), so the server acts on the change.

The structs send to the client through a small `Client` handle (the connection's
sender and `next_id`), not through `&mut Server`. Line numbers move as 27b and 28a-c
land; find the code by function name. **No behaviour change.**

**Files owned:** `src/server.rs`, new `src/server/{progress,shacl,refresh,layers}.rs`,
`src/server/run_query.rs` and `src/server/workspace_diagnostics.rs` (field accesses
that move into the groups).
**Hands off:** the `inline: bool` sites (29b), `src/ide/**`, `src/workspace.rs`,
`src/features/**`, `src/imports/**`.

**Blocked by:** 27b and 28c (the seams this builds on: `Effects` and `ide::Session`),
and 31 (`src/server.rs` lock). Runs alone.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `pub struct Server` has at most 20 fields; each group is a struct with private
      fields and methods; no submodule writes another group's fields. Report the field
      count before and after.
- [ ] New unit tests: progress (an `end` before the client accepts the token gives
      `begin` then `end` once accepted; a refused token sends nothing); refresh (a
      generation bump asks for a refresh only after a report went out; the pulled
      result id changes with the generation and the described generation); settings
      layers (a configuration section replaces the initialization options, which
      `review_tests::a_configuration_section_replaces_the_initialization_options`
      checks over the wire; `relayer` reports a prefix change).
- [ ] Green, no behaviour change: `cargo test --release --locked --lib` (server
      `tests`, `review_tests`), `--test server_lifecycle`; `test/resources.test.mjs`,
      `test/protocol.test.mjs`, `test/protocol-errors.test.mjs`, `test/lsp.test.mjs`,
      `test/shacl-validate.test.mjs`, `test/workspace-diagnostics.test.mjs`,
      `test/settings-file.test.mjs`, `test/query-lsp.test.mjs`, `test/imports.test.mjs`;
      `web/test/host.test.mjs`, `web/test/lsp.test.mjs`, `web/test/engine.test.mjs`;
      `cargo fmt --check`, clippy.
