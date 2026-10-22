# 27b: Workspace effects, part 2: mutators return `Effects`

**What to build:** the server, not the workspace, owns the invalidation rules (audit
X4-05, design A of §4.2). It finds prefixes.ttl by name in four places
(`uri.ends_with(&format!("/{PREFIX_FILE}"))` at `src/server.rs:995`, `:1196` and
`:1492`, and `path.file_name() == PREFIX_FILE` at `:1102`). It calls `context_changed`
with a `disk` flag it must choose correctly (`:991`, `:1084`, `:1200`, `:1227`,
`:1507`). It follows `settings_prefixes` with `prefixes_changed(None)` (`:1397-1399`)
and `invalidate_open_prefixes` or `prefixes_changed` after a prefixes.ttl edit
(`:1491-1504`). It marks every open document dirty in four places (`:1116`, `:1408`,
`:1419`, `:1502`). The 90-line `watched_files_changed` (`:1045-1140`) decides per path
what a disk change means. Make every mutating `Workspace` method return one value,
`Effects`: which open documents are dirty (`One(uri)`, `AllOpen`, or none), whether
knowledge outside the document changed, whether SHACL is due, the indexed files to
parse again off the main loop (today's `stale` lists), and the disk paths workspace
diagnostics must forget. The mutators are `open_doc_as`, `change_doc`, `close_doc`, a
new `save_doc(uri)` (today's `on_save` checks), `set_settings_prefixes`,
`context_fetched(url)`, and a new `files_changed(&[(PathBuf, Change)])` that takes over
the body of `watched_files_changed`. Server-only reactions stay in the server: the
settings file (`:1064-1067`), `imports.retry()` for catalogs and RDF/XML (`:1076-1079`,
`:1203`), and spawning the re-parse (`reindex_in_background`). One server function,
`apply(fx: Effects)`, maps the value to `dirty`, `generation`, `display_stale`,
`wsdiag` and `validate_shacl`. `knowledge_changed` (`:1415-1427`) becomes its
knowledge branch. `Effects` is a plain value, not `#[must_use]`: `batch` and tests
ignore it. **No behaviour change**, except that the same rules now have unit tests.

**Files owned:** `src/workspace.rs`. In `src/server.rs`, only `notification`,
`watched_files_changed`, `prefix_file_changed`, `reindexed`, `on_save`,
`context_fetched`, `relayer_settings`, `knowledge_changed`, `merge_index`,
`after_edit`, the new `apply`, the `review_tests` module, and the
`use crate::workspace::{..}` statement (see below). Also
`src/server/workspace_diagnostics.rs` (only the `wsdiag_*` entry points the server
calls with paths) and `src/batch.rs` (only if a mutator's new signature needs it).
**Hands off:** `Server::request` and everything below it in `src/server.rs` (28a holds
that region in the same wave), `diagnostics_for`, `publish_dirty`, `pull_diagnostics`,
`src/ide/**`, `src/features/**`.

**Blocked by:** 27a. Runs in the same wave as 28a. Both edit the `src/server.rs` lock,
in separate regions: 27b edits `notification` through `after_edit` (today
`:947-1512`), 28a edits `request` and the helpers after it (`:1647-2197`, `:2280-2288`).
The one shared statement is `use crate::workspace::{..}` (`:41-44`): 27b removes what
its move leaves unused (`PREFIX_FILE`, and `is_turtle`, `is_sparql_path` if they move),
and 28a removes `path_to_uri`. Whichever lands second resolves that one-statement
conflict by keeping both removals, then reruns clippy.

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `rg -n "PREFIX_FILE|context_changed|prefixes_changed|invalidate_open_prefixes" src/server.rs`
      finds nothing, and `dirty.extend(` appears once, in `apply`.
- [ ] New unit tests on `Effects` in `src/workspace.rs`, each on a temp-dir workspace:
      an edit to a data file is `One(uri)` with no knowledge change; an edit to an open
      prefixes.ttl is `AllOpen` plus a knowledge change, with no re-parse until it is
      saved; saving it lists the indexed files it serves (and no others); closing a
      JSON-LD context file with unsaved edits marks its users and changes knowledge; a
      settings prefix change lists every indexed Turtle file; `files_changed` with a
      prefixes.ttl and a data file together parses the data file once (the case of
      `review_tests::prefixes_and_a_data_file_changed_together_parse_the_data_once_with_the_new_prefixes`);
      a deleted folder forgets the files below it.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib` (server
      `tests` and `review_tests`), `--test server_lifecycle`, `--test diagnostics_agree`;
      `test/project.test.mjs`, `test/prefix-import.test.mjs`,
      `test/prefix-rename.test.mjs`, `test/jsonld.test.mjs`,
      `test/settings-file.test.mjs`, `test/workspace-diagnostics.test.mjs`,
      `test/imports.test.mjs`, `test/shacl-validate.test.mjs` ("a burst of edits is
      validated once, after it ends"), `test/lsp.test.mjs`, `test/resources.test.mjs`;
      `web/test/host.test.mjs` (embedded: no disk, every file open); `cargo fmt
      --check`, clippy.
