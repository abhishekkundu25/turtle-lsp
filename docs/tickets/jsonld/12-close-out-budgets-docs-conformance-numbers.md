# 12: JSON-LD phase 1 close-out: budgets, docs, conformance numbers

**What to build:**
- JSON-LD rows in the benchmark, with release-only budgets: open-to-diagnostics within 3× the Turtle equivalent, and keystroke-to-diagnostics under 10 ms with cached contexts.
- README and CHANGELOG sections stating what is supported, with conformance numbers from the expected-status manifest.
- A final pass so every phase-1 pending test is promoted or explicitly re-tagged with a reason.

**Blocked by:** 02, 03, 04, 06, 09, 10, 11

**Status:** ready-for-agent

**Parallel slices:** last; touches docs and the benchmark.

- [ ] The budgets hold on the synthetic 30,000-line file
- [ ] The README's numbers match the suite's output
- [ ] Zero phase-1 pending tests remain untagged; phase-2 tests remain pending

Spec: docs/specs/jsonld.md.
