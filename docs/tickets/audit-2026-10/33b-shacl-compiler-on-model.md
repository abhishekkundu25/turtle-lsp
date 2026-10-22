# 33b: SHACL model, part 2: the validator compiles from the model

**What to build:** after 33a, the lint reads `shacl::model`, but the validator still
finds shapes on its own. `validate::Compiler` (`src/shacl/validate.rs:653-700` and the
`build` code after it) reserves a shape per node, builds pending shapes without
recursion (so a chain of thousands of `sh:node` references compiles without
overflowing the stack), and reads targets, paths and parameters straight from the
`Graph`. Its vocabulary table `V` (`:107-276`) duplicates `shacl::vocab`. Make the
compiler build its constraints from `model` shapes. The model says which nodes are
shapes, their kind, path, targets and parameters. The compiler keeps what is
validation-only: severities, messages, `sh:deactivated`, the `Budget`, the
SHACL-SPARQL components, and lazy compilation as targets and parameters reach a shape.
Keep the non-recursive build: if the model is built eagerly, it must not recurse on
`sh:node` chains either. Replace `V` with `shacl::vocab`. **No behaviour change**:
every validation report stays the same, failures included (recursion cut at 64, an
ill-formed `sh:path` as an error, the step and result budgets).

**Files owned:** `src/shacl/validate.rs`, `src/shacl/model.rs` and
`src/shacl/vocab.rs` (additions the compiler needs).
**Hands off:** `src/features/shacl_lint*` (33a's reader stays as it is),
`src/knowledge.rs` (33c), `src/shacl/sparql.rs`, `src/shacl/literal.rs`,
`src/features/shacl_validate.rs`.

**Blocked by:** 33a.

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `validate.rs` has no vocabulary table of its own and no code that decides
      whether a node is a shape. Both come from `shacl::model` and `shacl::vocab`.
- [ ] Existing validator unit tests pass unchanged, including the long `sh:node` chain
      (`test/resources.test.mjs`: "a long chain of sh:node references validates without
      crashing") and the recursion, budget and ill-formed-path tests in
      `src/shacl/validate.rs`.
- [ ] Green, no behaviour change: `--test w3c_shacl` (the same pass count as before,
      Core at least 98, SPARQL at least 23), `cargo test --release --locked --lib`,
      `test/shacl-validate.test.mjs`, `test/shacl-sparql.test.mjs`,
      `test/shacl-lint.test.mjs`, `test/resources.test.mjs`, `test/cli.test.mjs`
      (`check` with shapes); `web/test/engine.test.mjs`; `cargo fmt --check`, clippy.
