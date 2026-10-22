# 28b: `ide::Session`, part 2: display

**What to build:** the second group of request arms moves from `Server::request` into
`src/ide/display.rs`, as `Session` methods taking a URI and a position or range (audit
X4-01). The arms today: hover (`src/server.rs:1733-1744`); document symbols, workspace
symbols and folding ranges (`:1896-1917`); type hierarchy prepare, supertypes and
subtypes, where supertypes and subtypes look up the asking document from the item's
data (`:1999-2021`); semantic tokens full and range (`:2022-2042`); inlay hints with
the `inlayHints.labels` mode (`:2043-2054`); code lenses with the
`codeLens.references` setting, where an RDF document gets reference counts and a SPARQL
document gets "▶ Run query" (`:2055-2062`); and code lens resolve (`:2063-2070`). These
line numbers move once 28a lands; find the arms by method. The settings these arms
project come from the `&Settings` that 28a put in `Session`, so `src/ide/mod.rs` is not
edited. **No behaviour change.**

**Files owned:** `src/ide/display.rs`. In `src/server.rs`, only the arms above and the
`use crate::features::{..}` and `use crate::features::hierarchy` statements (remove
what the move leaves unused).
**Hands off:** `src/ide/mod.rs`, `src/ide/navigation.rs`, `src/ide/edit.rs`, the
completion, code action and formatting arms (28c), the `doc!` and `open!` macros (28c
deletes them as their last user), the top of `src/server.rs` above `pub struct Server`
and `register_watchers` (31 holds them in this wave), `src/features/**`,
`src/sparql/features/**`.

**Blocked by:** 28a. Runs in the same wave as 31, in separate regions of the
`src/server.rs` lock: 28b edits only the display arms of `request` and the two
`use crate::features` statements. 31 edits the `use crate::workspace::{..}`
statement, the file-kind items above `WATCHED_FILES`, and `register_watchers`. The
`use crate::settings` and `sparql_features` lines between the two `use` statements stay
untouched by both, so the hunks do not touch.

**Status:** done (type hierarchy prepare left on `doc!`; moved by 28c)

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] Each display arm in `request` is decode, one `Session` call, encode; no
      `crate::features::{hover,inlay,semantic,symbols,hierarchy,lens}` path remains in
      `src/server.rs`.
- [ ] New in-process tests in `src/ide/display.rs`: `hover` routes to the RDF and the
      SPARQL implementation by document kind; `inlay_hints` follows the
      `inlayHints.labels` setting (`off` gives none); `code_lenses` gives reference
      counts for Turtle and the run lens for a `.rq` file, and no reference lenses with
      `codeLens.references` off; `supertypes` reads the asking document from the item.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib`, `--test chaos`;
      `test/display.test.mjs`, `test/lens.test.mjs`, `test/hierarchy.test.mjs`,
      `test/shacl-sparql-editor.test.mjs`, `test/knowledge.test.mjs`,
      `test/sparql.test.mjs`, `test/nquads.test.mjs`; `web/test/lsp.test.mjs`,
      `web/test/editor-hierarchy.test.mjs`; `cargo fmt --check`, clippy.
