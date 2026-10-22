# 06: SHACL bounds and fail-closed validation

**What to fix:** the engine passes every W3C test, but has no resource bounds and fails
open in four places. Reproduced unless noted.

Bounds:
- Path closures are recomputed per node, never memoized: one `sh:zeroOrMorePath` is
  O(N²) per shape (5k-node chain 0.48 s, 20k 7.6 s); nested closures are O(N^depth)
  (four levels on 300 triples: 17 s). `cancelled` is polled only between focus nodes, so
  a slow node cannot be stopped and newer jobs queue behind it
  (`src/shacl/validate.rs:973-1028`, poll `:1673-1684`;
  `src/features/shacl_validate.rs:144-147`).
- SHACL-SPARQL collects every solution, builds every `ValidationResult` and formats
  every diagnostic before the 1000-per-file cap (`src/shacl/sparql.rs:336-341`,
  `validate.rs:1399-1423`, `shacl_validate.rs:271-285`): a cartesian `sh:select` over
  5k triples took 17 s and 11.7 GB (slice measurement; not re-run).
- The SPARQL token checks only the 10 s deadline, not the job generation
  (`sparql.rs:297-306`); in inline mode cancellation is `|| false`.
- `Equals` uses `Vec::contains` where `Disjoint` uses a set (`validate.rs:1200-1207`).

Fail-open:
- When the 64-level depth limit cuts a check short, `conforms` caches `(shape, focus)
  → true` (`validate.rs:1067-1086`); later checks read the poisoned entry; the real
  limit is ~32 data hops; results depend on order. 100-node chain: 7 of 71 violations.
- `has_shapes` (`shacl_validate.rs:214-225`) ignores `sh:target`: a workspace whose only
  shapes are untyped SPARQL-target shapes is never validated. `validate::has_targets`
  is right and has no callers.
- Every SHACL-SPARQL `Failure` except a timeout is dropped (`shacl_validate.rs:415`);
  the validator rejects MINUS/VALUES/SERVICE in *target* queries that the lint
  deliberately allows (`sparql.rs:155-157` vs `shacl_lint/sparql.rs:129-152`), so such a
  target is silently discarded: no lint, no diagnostic, exit 0.
- A property path deeper than 64 becomes `Path::Invalid` and the shape is skipped with
  no diagnostic (`src/shacl/shapes.rs:330-334`, `validate.rs:522-526`): exit 0.

**Files owned:** `src/shacl/validate.rs`, `src/shacl/sparql.rs`, `src/shacl/shapes.rs`,
`src/shacl/mod.rs` (exports), `src/features/shacl_validate.rs`,
`src/features/shacl_lint/sparql.rs` (the role/pre-bound set only), `tests/w3c_shacl.rs`
(only if a helper is needed), `test/shacl-validate.test.mjs`,
`test/shacl-sparql.test.mjs`, `test/shacl-sparql-lint.test.mjs`.
**Hands off:** `src/shacl/literal.rs`, `src/server.rs` (inline budget: ticket 13),
`src/sparql/**` (ticket 02), `src/features/shacl_lint.rs` body.

**Blocked by:** nothing. Owner decisions assumed: recursion past the limit is
reported; targets are not held to the pre-binding restrictions; skipped shapes are
errors.

**Status:** ready-for-agent

- [ ] Memo `values(path, node, inv)` per `Run` (`Rc<[Id]>`); collapse nested closures
      in `parse_path`/`well_formed`; poll `cancelled` every few thousand closure steps;
      `Equals` via `FxHashSet`. Tests: the 20k chain under 1 s (release); the nested
      shape terminates; cancellation inside one focus node is honoured.
- [ ] A `Budget { results, steps }` passed through `report_with` into `Run`;
      `Engine::run`/`run_batch` stream the solution iterator and stop at the cap; the
      placer truncates by severity and emits one "results not shown" diagnostic. Test:
      the cartesian-product shape finishes under a fixed RSS (assert on result count and
      time, not RSS) with the note.
- [ ] `Engine` takes the job's cancel check and the token checks it beside the deadline.
      Test: a superseded run stops inside one long query.
- [ ] `check` returns `Truncated`; truncated results are never cached; truncation is a
      `Failure` placed on the shape ("recursion deeper than 64 at …"). Test: the 100-node
      chain gives 33 results plus a failure (or 71 if the limit is raised).
- [ ] One `acts_on(...)` in `shacl::validate` (includes `sh:target`) used by
      `report_with` roots and by `has_shapes`; delete or use `has_targets`; narrow the
      `pub` surface to `validate_report`. Test: the untyped SPARQL-target shape is
      validated.
- [ ] Every `Failure` surfaces as a warning on its `sh:select`/`sh:ask` with code
      `shacl-failure` (generalise `Placer::timed_out`); `restrictions(query, Role)` is one
      walker used by validator and lint, with `Role::Target` skipping pre-binding rules.
      Tests: MINUS target is validated; a `?failure true` solution is a diagnostic.
- [ ] `Path::Invalid` is a `shacl` error on the shape; `check` exits 1. Test: 10-step
      path validates, 20,000-step path errors.
- [ ] W3C SHACL Core 98/98 and SPARQL 23/23 unchanged; `batched_sparql_matches_one_by_one`
      and the `shacl_validate` unit tests green; the three Node files green.
