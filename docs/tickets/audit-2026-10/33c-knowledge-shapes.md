# 33c: SHACL model, part 3: the completion index's shapes follow

**What to build:** `knowledge::Shape` and `PropertyConstraint` (`src/knowledge.rs:31-56`)
feed completion ranking: `shapes_targeting` (`:900`) ranks the properties a typed
subject's shapes ask for. They are built per document at each `Knowledge::set` by
`DocFacts::shapes` (`:339-466`), with their own predicate table `Preds` (`:174-233`).
This model is narrower: only a node shape that targets classes counts, with
`sh:targetClass`, or implicitly a class that is also a `sh:NodeShape`. Since 33a and
33b, the lint and the validator read `shacl::model`, so "what is a shape" can still
drift between completion and validation (audit X4-15, A5 R6). Build `knowledge::Shape`
from `shacl::model` over the document's triples, keeping the narrower filter as an
explicit, documented step on top of the model. Replace `Preds` with `shacl::vocab`
where the IRIs are SHACL, RDF or RDFS. The per-edit cost must stay O(document): if a
one-document `Graph` plus the model costs more than the current builder within the
latency tests, keep the builder and only document the narrower definition on
`knowledge::Shape` (X4-15's other option; owner decision 8). The report says which way
it went, with the timings of the tests below. **No behaviour change**: completion
ranking is identical.

**Files owned:** `src/knowledge.rs` (the shape builder, `Shape`, `PropertyConstraint`,
`Preds`, their tests), `src/shacl/model.rs` and `src/shacl/vocab.rs` (additions it
needs).
**Hands off:** `src/shacl/validate.rs`, `src/features/shacl_lint*`,
`src/features/completion/**` (they read `shapes_targeting`, whose signature stays),
the rest of `src/knowledge.rs`.

**Blocked by:** 33b (`src/shacl/model.rs`).

**Status:** done via the fallback in owner decision 8: the builder stays (building from the model costs 2.2-3.8x per edit); a test pins it to the model's shapes that target classes.

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks: the cost check is the tests below. No new
dependencies. Leave the changes uncommitted.

- [ ] `knowledge.rs` decides what a shape is only through `shacl::model` plus the
      documented narrower filter, or (fallback) the doc comment on `knowledge::Shape`
      states the narrower definition and why it is kept.
- [ ] The completion-ranking unit tests in `src/knowledge.rs` and
      `src/features/completion/ranking.rs` pass unchanged. A new test: a property shape
      reached through `sh:property [ ]` on a class-targeting node shape and one on a
      shape without a target give the same `shapes_targeting` answer as before (pin
      the old answer first).
- [ ] Green, no behaviour change: `cargo test --release --locked --lib`,
      `test/completion-ranking.test.mjs`, `test/knowledge.test.mjs`,
      `test/shacl-lint.test.mjs`, `timeout 120 npm run test:latency`
      (`test/latency.test.mjs`); `cargo fmt --check`, clippy.
