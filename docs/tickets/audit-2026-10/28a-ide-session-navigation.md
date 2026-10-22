# 28a: `ide::Session`, part 1: navigation

**What to build:** the IDE layer lives inside `Server::request` (`src/server.rs:1647-2076`,
430 lines), so the composition across document families can be tested only over
JSON-RPC (audit X4-01, design B of §4.1). Create the session in `src/ide/mod.rs`, today
a two-line stub: `pub struct Session<'w>` over `&'w mut Workspace`, the `Encoding`,
`&'w Settings` and the client's `resolve_edits`. It gets one method per request, taking
the URI and position the caller has. Routing by kind becomes `Session::doc(uri) ->
Option<Doc>`, replacing the `Open` enum (`:2193-2197`) and the bodies of the `doc!` and
`open!` macros (`:1657-1685`). Part 1 moves the navigation arms into
`src/ide/navigation.rs`: definition, type definition, implementation, references,
document highlight, prepare rename and rename (`:1745-1895`). It also moves the helpers
they use: `prefix_file_for` (`:2176-2190`), which reads prefixes.ttl from disk when it
is not open; `sparql_others` (`:2162-2174`), which reads the SPARQL documents only
when the cursor is on a term; and `rdf_term_at` (`:2280-2288`). The composition moves
with them: references append `sparql_features::term_references` (`:1796-1799`), and
rename builds a `PrefixScope` whose `file_of` closure walks the disk, then appends
`rdf_term_rename` (`:1859-1890`). The two families also get one error convention for
prepare rename (the rest of X4-12): RDF returns `Result<Option<_>, String>`
(`features/navigation.rs:365-369`) and SPARQL returns `Option<_>`
(`sparql/features/mod.rs:107-111`). The session method returns `Result<Option<_>,
String>` for both, and SPARQL's "nothing to rename" stays `null` on the wire. After
this slice, each moved arm in `request` decodes its params, calls one `Session` method,
and encodes the answer. The other arms keep the macros, which now call
`Session::doc`, and match on `ide::Doc` instead of `Open`, until 28b and 28c move
them. Create empty `src/ide/display.rs` and
`src/ide/edit.rs` (with their `mod` lines), so 28b and 28c never edit `src/ide/mod.rs`.
**No behaviour change.**

**Files owned:** `src/ide/mod.rs`, new `src/ide/navigation.rs`, new empty
`src/ide/display.rs` and `src/ide/edit.rs`. In `src/server.rs`, only `request` (the
macros, the navigation arms, a `fn session(&mut self)` placed just above it), the
helpers after it listed above, and the `use` lines its move leaves unused.
**Hands off:** `src/features/**` and `src/sparql/features/**` (call them, do not change
them), `src/workspace.rs`, `src/server.rs` from `notification` to `after_edit` (27b
holds it in this wave), `src/lib.rs`.

**Blocked by:** 27a (the `roots()` accessor; `src/server.rs` lock). Runs in the same
wave as 27b, in separate regions of `src/server.rs`. 27b's ticket names the regions and
the one shared `use crate::workspace::{..}` statement: 28a removes `path_to_uri` from
it, and the second to land keeps both removals.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`, and tests that read prefixes.ttl or SPARQL files from disk make
their folders under the default temp directory. No performance benchmarks. No new
dependencies. Leave the changes uncommitted.

- [ ] `src/server.rs` has no `prefix_file_for`, `sparql_others`, `rdf_term_at`, no
      `enum Open`, and no `match` on the document kind in the moved arms. Each moved arm
      is decode, one `Session` call, encode.
- [ ] New in-process Rust tests in `src/ide/navigation.rs`, on a workspace in a temp
      directory with RDF and SPARQL files:
      `references` from an RDF term includes the uses in a closed `.rq` file;
      `references` on a SPARQL variable does not read the other SPARQL files (the
      `sparql_others` rule);
      `rename` of an RDF term rewrites its uses in a SPARQL file;
      `rename` in a SPARQL document with a prefixes.ttl on disk and not open uses that
      file's prefix;
      `rename` of a JSON-LD term renames the term, not its IRI;
      `prepare_rename` on a non-renamable position gives `Ok(None)` for SPARQL and the
      `Err` message for RDF, as the wire did before.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib`,
      `--test server_lifecycle`, `--test chaos`; `test/lsp.test.mjs`,
      `test/project.test.mjs`, `test/sparql-navigation.test.mjs`,
      `test/sparql-rename.test.mjs`, `test/prefix-rename.test.mjs`,
      `test/resolve-rename.test.mjs`, `test/jsonld.test.mjs`, `test/trig.test.mjs`;
      `web/test/lsp.test.mjs`; `cargo fmt --check`, clippy.
