# 33a: SHACL model, part 1: one model and one vocabulary, read by the lint

**What to build:** "what is a shape" is written several times (audit A5 R6, X4-15).
`src/shacl/shapes.rs` (450 lines) builds `Shapes` from one document's triples: node
and property shapes, paths, targets and parameters by triple index. Only the shape lint
uses it (`src/features/shacl_lint.rs:18`, `:83`, `:116-125`, `:252`). The validator
compiles its own shapes straight from the graph (`validate::Compiler`,
`src/shacl/validate.rs:653-700`) and shares only `Path` with it (`validate.rs:31`,
`use super::Path`). The completion index has a third, narrower model
(`knowledge::Shape`, `src/knowledge.rs:31-56`, built at `:339-466`). And there are four
interned vocabulary tables: `shapes::Vocab` (`shapes.rs:18-100`), `validate::V`
(`validate.rs:107-276`), `shacl_lint/sparql.rs` `V` (`:27-59`) and `knowledge::Preds`
(`knowledge.rs:174-233`). Part 1 adds `src/shacl/model.rs`: the shape model (node,
kind, path, targets, parameters, each with the index of the triple it came from, so
the lint keeps its spans) built over `rdf::graph::Graph`. The lint builds a
one-document `Graph` (`Graph::new(&[&a.parsed.triples])`) and reads the model. Part 1
also adds `src/shacl/vocab.rs`, one table for the SHACL, RDF, RDFS and OWL IRIs the
three SHACL users intern. It then deletes `src/shacl/shapes.rs` and ports its unit
tests to `model.rs`. `Path` moves to `model`, `validate.rs` imports it from there, and
`shacl::mod` re-exports what it re-exported before. The validator's compiler moves onto
the model in 33b, and `knowledge` in 33c. Neither is in this slice. **No behaviour
change**, and the lint's per-keystroke cost stays within `many_constraints_lint_fast`.

**Files owned:** new `src/shacl/model.rs`, new `src/shacl/vocab.rs`,
`src/shacl/shapes.rs` (deleted), `src/shacl/mod.rs`, `src/shacl/validate.rs` (the
`Path` import only), `src/features/shacl_lint.rs`, `src/features/shacl_lint/sparql.rs` (its `V`
table), and `tests/w3c_shacl.rs` and `examples/shacl.rs` only if a re-export they use
moves.
**Hands off:** `validate::Compiler` and the constraint code (33b), `src/knowledge.rs`
(33c), `src/shacl/sparql.rs`, `src/shacl/literal.rs`, `src/features/shacl_validate.rs`.

**Blocked by:** nothing in wave 3 (the wave-1 SHACL bounds work, ticket 06, has landed).

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks: the keystroke check is the
`many_constraints_lint_fast` unit test. No new dependencies. Leave the changes
uncommitted.

- [ ] `src/shacl/shapes.rs` is gone; `rg -n "shacl::shapes|struct Vocab|struct V \{" src`
      finds only `src/shacl/vocab.rs`, plus `validate::V` and `knowledge::Preds`,
      which 33b and 33c remove.
- [ ] The ported `shapes` unit tests pass in `src/shacl/model.rs`. A new test: the
      model of a document with blank-node property shapes, `sh:node`, `sh:and`/`sh:or`
      members and a `sh:qualifiedValueShape` finds the same shapes, kinds, paths and
      triple indexes that `shapes::Shapes` found. Pin the old answers in the test before
      deleting the file.
- [ ] Green, no behaviour change: `test/shacl-lint.test.mjs` (16 tests, including "the
      project shapes produce no shacl diagnostics"), `test/shacl-sparql-lint.test.mjs`,
      `test/shacl-validate.test.mjs`, `test/shacl-sparql.test.mjs`;
      `cargo test --release --locked --lib` (with
      `features::shacl_lint::sparql::tests::many_constraints_lint_fast`),
      `--test w3c_shacl` (the Core and SPARQL counts it asserts, at least 98 and 23,
      with the same pass count as before); `cargo build --release --locked --examples`;
      `cargo fmt --check`, clippy.
