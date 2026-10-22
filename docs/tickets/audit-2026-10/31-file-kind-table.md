# 31: One file-kind table

**What to build:** what kind of file a path names is decided in several places, with
different rules (audit X4-09's root cause; backlog B15):
- `Syntax::from_extension` (`src/syntax/mod.rs:51-62`, any letter case);
- `is_sparql_name` and `is_sparql_path` (`src/workspace.rs:69-73`, `:1473-1475`,
  lower case only);
- `DocKind::detect` (`src/workspace.rs:80-92`);
- `is_turtle` (`src/workspace.rs:456-458`), which means "an RDF file of any syntax"
  (X4-14);
- in `src/server.rs`, `is_import_source` (`:68-73`: `catalog-v001.xml`, `.rdf`, `.owl`
  in any case), `is_watched_file` (`:77-84`, which adds `.json`) and the watcher glob
  `WATCHED_FILES` (`:89`), which lists the same extensions again by hand.

Write one table in a new `src/workspace/kinds.rs`. It is a child module of
`workspace.rs` (`pub mod kinds;`), so neither `src/lib.rs` nor `src/syntax/mod.rs` is
edited. It has one row per extension or file name, giving the kind (RDF with its
`Syntax`, SPARQL, import source, catalog, JSON context or settings, watched or not) and
today's letter-case rule. `is_sparql_name`, `is_import_source`, `is_watched_file` and
`DocKind::detect` read it. `is_turtle` is renamed `is_rdf_file`. `Syntax::from_extension`
stays in `syntax` (the syntax core cannot depend on the workspace), and a test checks
it against the table's RDF rows. `WATCHED_FILES` stays a `const`, and a test checks it
equals the glob built from the table. After 27b the watched-file classification lives
in `src/workspace.rs` (`files_changed`), so follow it there. `src/features/refactor.rs:361`
(document suffixes stripped from an ontology IRI to derive a prefix name) is about
IRIs, not files, and stays. **No behaviour change**: every path keeps the kind and case
rule it has today (owner decision 6).

**Files owned:** new `src/workspace/kinds.rs`, `src/workspace.rs` (the items above, the
`is_turtle` callers at `:643`, `:1131`, `:1147`, the `is_sparql_path` callers, the
`mod` line, the unit tests that name them), `src/server.rs` (the items above
`pub struct Server`, the `use crate::workspace::{..}` statement, `register_watchers`),
`src/server/workspace_diagnostics.rs` (`:44`, `:302`, `:397-402`),
`src/server/run_query.rs` (`:35`, `:167`), `src/cli/files.rs` (`:8`, `:278-301`,
`:382`).
**Hands off:** `Server::request` and the display arms (28b holds them in this wave),
`src/syntax/**`, `src/features/**`, `src/imports/**` (`imports::load::format_of`
already reads `Syntax::from_extension` first, `src/imports/load.rs:531-537`), the
`SyntaxError` construction in `src/workspace.rs` `failed_analysis` (`:421-435`, 34a's in
this wave).

**Blocked by:** 27b (the watched-file logic moved into `src/workspace.rs`; `src/workspace.rs`
and `src/server.rs` locks). Runs in the same wave as 28b and 34a. With 28b it shares
the `src/server.rs` lock in separate regions: 31 edits the top of the file, the
`use crate::workspace` statement and `register_watchers`; 28b edits the display arms
and the `use crate::features` statements. With 34a it shares `src/workspace.rs`: 34a
edits only `failed_analysis`, which 31 does not touch.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] Outside `src/workspace/kinds.rs` and `Syntax::from_extension`, no code compares
      an extension or a file name with a literal to decide a file's kind
      (`rg -n '"(rq|sparql|ru|rdf|owl|json|jsonld|ttl|trig|nt|nq)"' src` lists only the
      table, `from_extension` and tests). `rg -n "is_turtle\b" src` finds nothing.
- [ ] Unit tests in `src/workspace/kinds.rs`: every row against `from_extension`,
      `DocKind::detect` and the old predicates' answers (copied into the test as a
      table of path to expected kind, including `a.TTL`, `a.RQ`, `a.json`,
      `catalog-v001.xml`, `x.OWL`, `turtle-lsp.json`); `WATCHED_FILES` equals the
      glob built from the table.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib`,
      `--test server_lifecycle`; `test/nquads.test.mjs`, `test/trig.test.mjs`,
      `test/imports.test.mjs`, `test/sparql.test.mjs`, `test/query-lsp.test.mjs`,
      `test/cli.test.mjs`, `test/query-cli.test.mjs`,
      `test/workspace-diagnostics.test.mjs`, `test/settings-file.test.mjs`;
      `(cd editors/vscode && timeout 120 node --test test/manifest.test.mjs)`;
      `cargo fmt --check`, clippy.
