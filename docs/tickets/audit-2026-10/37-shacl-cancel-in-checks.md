# 37: SHACL checks honour cancellation and the step budget

**What to fix:** found by ticket 33b. When `sh:node` (or another shape-valued
constraint) is reached many times along a chain deeper than 64, the checks cut by the
depth limit are never cached (correctly, since a cut check has no answer), so the work
grows exponentially. No path step is charged on that route, so the step budget never
runs out and `cancelled` is never polled. Reproduced with the pre-33b validator: a
3,000-shape `sh:node` chain read as an overlay over itself (every triple doubled) ran
past 60 s with a cancel closure that returned true after 15 s. In the server, that SHACL
worker cannot be stopped and newer jobs queue behind it.

**Files owned:** `src/shacl/validate.rs` (`check`, `conforms`, the step/cancel helpers,
its tests).
**Hands off:** everything else, notably `src/shacl/model.rs` and `src/shacl/vocab.rs`
(33c), `src/knowledge.rs` (33c), `src/features/shacl_validate.rs`.

**Blocked by:** 33b.

**Status:** done. Departure: the 40M step budget now also counts shape checks (one per check that misses the cache).

- [ ] Every `check()` (and `conforms()` that does not hit the cache) charges one step
      against `Budget.steps` and polls `cancelled` on the same cadence as the closure
      code (every few thousand steps). Exhausting the budget there is a `Cut` failure,
      as for paths; results after it follow the existing "dropped after the budget ran
      out" rule.
- [ ] Test: the doubled 3,000-shape `sh:node` chain (build it in the test) with a cancel
      flag set after a counted number of polls returns `None` (stopped) promptly: assert
      on the poll count, not wall time. A second test: with a small step budget, the
      same graph ends with a `Cut` failure and no false results.
- [ ] No other change: W3C SHACL Core 98/98 and SPARQL 23/23, every `shacl::validate`
      unit test, `test/shacl-validate.test.mjs`, `test/performance-a.test.mjs`,
      `test/resources.test.mjs` green.

Rules: test runs under 2 minutes (timeout 120); never point TMPDIR inside the repo; no
benchmarks; no new dependency; changes left uncommitted.
