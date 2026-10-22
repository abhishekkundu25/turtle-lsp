# 26a: Layout engine, part 1: options and the token document

**What to build:** the Turtle and SPARQL formatters each carry a copy of the same
layout layer (audit X4-03), and three cores import their options from a feature module
(X4-02). Part 1 moves the shared half into `src/layout/`, today a two-line stub
(`src/layout/mod.rs`). First `Options` (`src/features/format.rs:33-60`), which
`src/sparql/mod.rs:82`, `src/sparql/format.rs:64`, `src/sparql/features/mod.rs:35`,
`src/jsonld/format.rs:12` and `src/query/render.rs:17` import from
`crate::features::format`. Then the token document with comment attachment:
`Comment`, `Doc` (`lead`, `trail`, `blank_before`, `lead_pref`, `trail_pref`),
`has_blank`, `comments_inside` and `leading` (`src/features/format.rs:145-263`, copied
at `src/sparql/format.rs:288-427`). The copies have drifted. `has_blank` is written two
ways (`features/format.rs:173-179` against `sparql/format.rs:316-318`). Only SPARQL's
`Doc::new` returns `Option`, and only SPARQL's `Doc` has `close`, the matching-bracket
table. Make `layout::Doc` generic over a small `LayoutToken` trait (span, is-comment)
with two adapters: `syntax::lexer::Token` and `sparql::lexer::Token`. SPARQL keeps its
bracket table beside the shared `Doc`, or the shared `Doc` builds it as an optional
pass. `Options::from_lsp` takes `lsp_types::FormattingOptions`, so it stays in
`src/features/format.rs` as an inherent `impl crate::layout::Options` block, and
`features::format` re-exports `Options`. With that, `src/batch.rs` and `src/server.rs`
need no edit, and `layout` depends on nothing above `syntax` and `sparql::lexer`.
**No behaviour change:** every formatter output stays byte-identical.

**Files owned:** `src/layout/mod.rs`, new `src/layout/doc.rs`, `src/features/format.rs`,
`src/sparql/format.rs`, `src/sparql/mod.rs` (the `Options` import and its uses only),
`src/sparql/features/mod.rs` (line 35 only), `src/jsonld/format.rs` (line 12 only),
`src/query/render.rs` (line 17 only: `Options` from `layout`; `features::format::format`
stays, because the renderer prints CONSTRUCT results with the Turtle formatter).
**Hands off:** `src/server.rs`, `src/batch.rs`, `src/lib.rs` (the module-index row for
`layout` is 32a's), the printer, term and width code of both formatters (26b),
everything else.

**Blocked by:** nothing in wave 3. First of the two layout slices; 26b follows.

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `src/layout/{mod,doc}.rs` hold `Options` and the token document. Neither
      formatter defines `Comment`, `Doc`, `has_blank`, `comments_inside` or `leading`
      any more. `grep -rn "features::format::Options" src/sparql src/jsonld src/query`
      finds nothing.
- [ ] `src/layout/` has no `use` of `crate::features`, `crate::server`,
      `crate::analysis` or `lsp_types`.
- [ ] New unit tests in `src/layout/doc.rs`: own-line against trailing comments, the
      blank line before a comment, a comment at the end of the file, each through both
      token adapters. `has_blank` on `"\n\n"`, `"\n  \n"`, `"\r\n\r\n"`, `"\n"` and
      `""` gives what both old versions gave. Check first that the two old versions
      agree on these inputs. If they ever disagree, stop and report; do not choose one.
- [ ] Byte-identical output: `tests/format.rs` (all of it, including
      `messy_fixtures_match_the_golden_files`,
      `every_fixture_formats_to_a_fixed_point_with_the_same_graph_and_comments`,
      `comments_between_every_token_survive`,
      `w3c_trig_and_ntriples_files_format_safely`, `jsonld_corpus_round_trip`), the
      tests in `src/sparql/format.rs` (`golden_files`, `comments_stay`, `tabs`,
      `long_query_is_fast` and the rest), the `jsonld::format` unit tests,
      `src/query/tests.rs`, `test/format.test.mjs`, `test/sparql-format.test.mjs`.
- [ ] Corpus diff. Before the change, copy `target/release/turtle-lsp` and every
      `.ttl .trig .nt .nq .rq .ru` file under `test/fixtures/` and `tests/w3c/` (and
      `test/corpus/` if present) to a folder under the default temp directory, then run
      the copied binary's `format` there. After the change, do the same on a fresh copy
      with the new binary. `diff -r` of the two trees prints nothing. Delete both
      folders when done.
- [ ] Green: `cargo fmt --check`, `cargo clippy --release --locked --all-targets -- -D
      warnings`, `cargo test --release --locked --lib`, `--test format`, `--test chaos`,
      `--test w3c_conformance`.
