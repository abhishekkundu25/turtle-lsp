# 28c: `ide::Session`, part 3: edits and diagnostics

**What to build:** the last group of request arms moves into `src/ide/edit.rs`, and
`Server::request` becomes decode, call, encode (audit X4-01). The arms today:
completion, with the rule that `?` and `$` trigger completion only inside a SHACL
query literal (`src/server.rs:1690-1722`); completion resolve, routed by `"sparql":
true` in the item's data (`:1723-1732`); code actions, which combine
`quickfix::code_actions` filtered by `context.only`, `refactor::code_actions` and
`imports::fetch_actions` for RDF, and `sparql_features::code_actions` for SPARQL
(`:1922-1949`); code action resolve, routed by the URI in the action's data
(`:1970-1989`); and formatting with `format.lineWidth` (`:1990-1998`). The routing part
of `diagnostics_for` (`:1515-1539`) becomes `Session::diagnostics(uri, &DiagnosticInputs)`
over `diagnose::document`, so `batch` can call the same method later. The guard,
`DiagnosticInputs` (import statuses, SHACL results) and the cached-import rule stay in
the server, which owns that state. `ExecuteCommand` and `DocumentDiagnostic` stay in
the server: they act on server state (imports, query runs, result ids). This slice
deletes the `doc!` and `open!` macros, whose last users it moves. The `text`
parameters X4-12 named are already gone: `complete(a, k, enc, pos)` (`:1716`),
`document_symbols(a, enc)`, `quickfix::code_actions(a, k, enc, &p)` (`:1937`).
**No behaviour change.**

**Files owned:** `src/ide/edit.rs`. In `src/server.rs`, only the arms above,
`diagnostics_for`, the `doc!`/`open!` macros, and the `use` lines the move leaves
unused.
**Hands off:** `src/ide/mod.rs` (the session fields 28a set), `src/ide/{navigation,display}.rs`,
`src/features/**`, `src/sparql/features/**`, `src/diagnose.rs`, `src/batch.rs`.

**Blocked by:** 28b (the `src/server.rs` lock; the last user of the macros). Runs in
the same wave as 34b, which edits no server file.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `Server::request` has no `doc!`/`open!` macro and no match on the document kind.
      Carried over from 28b: the type hierarchy prepare arm still uses `doc!` (its only
      user); switch it to the existing, tested `Session::prepare_type_hierarchy`, and drop
      `use crate::features::hierarchy;` from `src/server.rs`.
      `rg -n "sparql_features::|crate::features::(completion|quickfix|refactor|format|embedded)" src/server.rs`
      finds nothing. Report the line count of `request` before and after.
- [ ] New in-process tests in `src/ide/edit.rs`: completion with trigger `?` outside a
      SHACL query literal gives `None`, and inside one gives the same list as
      `completion::complete` at that position; code actions with
      `only: ["quickfix"]` hold no refactorings and with no `only` hold all three
      sources; resolving an action whose data names a SPARQL URI goes to the SPARQL
      resolver; formatting uses `format.lineWidth` from the settings;
      `Session::diagnostics` on an open document equals `diagnose::document` on the
      same analysis.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib`, `--test chaos`,
      `--test diagnostics_agree`, `--test server_lifecycle`;
      `test/completion-ranking.test.mjs`, `test/sparql-completion.test.mjs`,
      `test/refactor.test.mjs`, `test/sparql-actions.test.mjs`,
      `test/format.test.mjs`, `test/sparql-format.test.mjs`,
      `test/unknown-term.test.mjs`, `test/prefix-import.test.mjs`,
      `test/workspace-diagnostics.test.mjs`, `test/shacl-sparql-editor.test.mjs`,
      `test/lsp.test.mjs`; `web/test/lsp.test.mjs`, `web/test/engine.test.mjs`;
      `cargo fmt --check`, clippy.
