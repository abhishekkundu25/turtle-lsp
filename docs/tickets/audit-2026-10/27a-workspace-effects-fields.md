# 27a: Workspace effects, part 1: private fields

**What to build:** `Workspace` exposes its state. `roots`, `open`, `disk`,
`settings_prefixes` and `rescan_sparql` are `pub` (`src/workspace.rs:106-143`), and the
struct comment's promise that knowledge is "kept current by every method that changes
either" (`:111-112`) holds only while callers do not write them (audit X4-05). Outside
the module they are read at these sites: `src/server.rs:365` (`rescan_sparql`), `:498`
(`settings_prefixes`, the index thread), `:1059` and `:1367` (`roots.first()`),
`:1082` (`open.contains_key`), `:1116`, `:1408`, `:1419` and `:1502`
(`dirty.extend(ws.open.keys())`), `:1214` and `:1868` (`roots.clone()`), `:1547`
(`open.get(..).version`), `:1397` (an assignment to `settings_prefixes`), the server
tests at `:2609`, `:2658` and `:2824` (`disk`), `src/server/run_query.rs:167`,
`src/server/workspace_diagnostics.rs:368`, `:390`, `:446-454`, `:535` and `:563`, and
`src/batch.rs:750` (`open.get(uri).map(|d| d.kind)`). Make the five fields private and
give the callers read accessors named for what they ask: `roots()`, `is_open(uri)`,
`open_uris()`, `version(uri)`, `doc_kind(uri)`, `is_indexed(uri)`,
`settings_prefixes()`, and `set_rescan_sparql(bool)` at construction. Keep the one
assignment as a `set_settings_prefixes(map)` that does exactly what the server does
today: assign, nothing more. 27b folds the invalidation into it. Test-only counts
(`disk.len()`) get a `#[cfg(test)]` accessor. **No behaviour change.**

**Files owned:** `src/workspace.rs` (struct visibility and the new accessors only),
`src/server.rs` (the field reads listed above, nothing else),
`src/server/run_query.rs:167`, `src/server/workspace_diagnostics.rs` (the five sites
above), `src/batch.rs:750`.
**Hands off:** every `Workspace` method body other than the new accessors, the server's
invalidation logic (27b), `src/features/**`, `src/ide/**`.

**Blocked by:** nothing in wave 3. First holder of the `src/server.rs` and
`src/workspace.rs` locks in wave 3.

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `rg -n "pub (roots|open|disk|settings_prefixes|rescan_sparql)\b" src/workspace.rs`
      finds nothing. `cargo build --release --locked --all-targets` compiles
      `web/engine`, `examples/` and `tests/` unchanged, or the ticket lists each call
      site it had to move.
- [ ] No `ws.open`, `ws.disk`, `ws.roots` or `ws.settings_prefixes` field access
      remains outside `src/workspace.rs` (grep).
- [ ] Unit test in `src/workspace.rs`: the accessors agree with the state after
      `open_doc`, `close_doc` and `add_disk` (an open document that is also on disk is
      `is_open` and not `is_indexed`, as `open_doc_as` arranges at `:583`).
- [ ] Green, no behaviour change: `cargo test --release --locked --lib` (including the
      server tests and `review_tests`), `--test server_lifecycle`,
      `--test diagnostics_agree`; `test/lsp.test.mjs`, `test/project.test.mjs`,
      `test/workspace-diagnostics.test.mjs`, `test/query-lsp.test.mjs`,
      `test/settings-file.test.mjs`, `test/cli.test.mjs`; `cargo fmt --check`, clippy.
