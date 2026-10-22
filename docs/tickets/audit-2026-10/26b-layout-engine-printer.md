# 26b: Layout engine, part 2: printer and terms

**What to build:** the second half of the shared layout engine (audit X4-03, design A
of §4.3). Both formatters still carry their own printer primitives. Turtle has
`write/indent/flush/line/top_line/space/leading/after/tok/punct` at
`src/features/format.rs:783-886`, the widths `tok_w/term_w/obj_w/objs_w/pol_w/fits` at
`905-968`, and the layout of terms, object lists and predicate-object lists
(`term/obj/pol/pol_from/objects`) at `969-1093`. SPARQL has the same primitives at
`src/sparql/format.rs:631-735`, the same widths at `782-862` and the same term layout
at `1759-1880`. Both also define the `Term`, `Obj` and `Po` types with
`first/last/is_group/simple/flat` (`features/format.rs:265-378`, `sparql/format.rs:428-506`).
Move these into `src/layout/printer.rs` (a `Printer` core over `layout::Doc` and
`layout::Options`) and `src/layout/terms.rs` (`Term`, `Obj`, `Po` generic over the
token index). Each formatter keeps its own CST walk and its language-only rules: Turtle's
`Cst`, `document`, `statement`, `graph` and `hoisted_order`; SPARQL's keyword-driven
methods (`select`, `pattern`, `values`, `inline`, `fill` and the others). Reconcile the
drifted pending comment: a `Vec<u32>` popped in Turtle (`features/format.rs:743`, with
`one_pending` keeping it to one, `:897`) and an `Option<u32>` in SPARQL
(`sparql/format.rs:534`). Prove with the goldens that one representation gives both
outputs. **No behaviour change:** every formatter output stays byte-identical, and the
formatter stays linear in tokens.

**Files owned:** `src/layout/mod.rs`, new `src/layout/printer.rs`, new
`src/layout/terms.rs`, `src/layout/doc.rs` (only what the printer needs from it),
`src/features/format.rs`, `src/sparql/format.rs`.
**Hands off:** `src/server.rs`, `src/batch.rs`, `src/lib.rs`, `src/jsonld/format.rs`
(its JSON tree layout stays its own, X4 §2 (d)), `src/query/**`, `src/sparql/mod.rs`,
everything else.

**Blocked by:** 26a.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] Neither formatter defines `write`, `indent`, `flush`, `line`, `top_line`,
      `space`, `after`, `tok`, `punct`, the width functions, or `Term`/`Obj`/`Po`: they
      live in `src/layout/{printer,terms}.rs`. Report how many lines leave each
      formatter file (X4 estimated about 500 in total).
- [ ] One pending-comment representation, documented in `layout::Printer`. A unit
      test in `src/layout/printer.rs` covers a trailing comment followed by a second
      trailing comment on the next token, for both token adapters.
- [ ] New unit tests in `src/layout/terms.rs`: `flat` and the width of a nested
      `[ ]`, a `( )` list, an annotation `{| |}` and a multi-line string, which counts
      by its first line.
- [ ] Byte-identical output: `tests/format.rs` (all of it), the tests in
      `src/sparql/format.rs` (`golden_files`, `narrow_width_breaks_projection_and_object_list`,
      `comments_stay`, `long_query_is_fast` and the rest), `src/query/tests.rs`,
      `test/format.test.mjs`, `test/sparql-format.test.mjs`, and the format round trips
      in `tests/chaos.rs`.
- [ ] The corpus diff of 26a, repeated against the binary from before 26b: `diff -r`
      prints nothing.
- [ ] Green: `cargo fmt --check`, clippy (`--all-targets -D warnings`),
      `cargo test --release --locked --lib`, `--test format`, `--test chaos`.
