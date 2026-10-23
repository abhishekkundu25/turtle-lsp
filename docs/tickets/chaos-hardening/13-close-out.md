# 13: Close-out: 20 runs, replays, budget from measurement, CHANGELOG, docs

**What to build:** On a quiet machine (load under 1, nothing else building): 20 consecutive runs with `CHAOS_ITERS=200000` and a fresh seed from the clock, each format's shards as their own command under `timeout 120` (`-- mutated_turtle`, `-- mutated_sparql`, `-- mutated_jsonld`, same seed), each logged with its time and five slowest iterations. From the slowest iteration over all 20 runs, set `ITERATION_BUDGET` in tests/chaos.rs to that time times a margin of at least 10, with the measurement in the comment. Every saved input in the README table replayed with its seed and `CHAOS_ONLY`. The full validation from AGENTS.md (unit, timing, integration, doc, Node, web, VS Code suites, fmt, clippy, msrv). `bench/perf.mjs` three times on this branch and three on main; medians compared per row. Then CHANGELOG `## Unreleased` entries (one line per class in user-facing wording, including that `format --check` now fails on files with `; # c` before `]`, `}` or `.`, that declarations may move to right after the last declaration when data shares its line, and that SPARQL "Add prefix" inserts below a leading comment), AGENTS.md's chaos sentence updated with `CHAOS_ONLY`, `CHAOS_ITERATION_BUDGET_MS` and the "no exemptions" rule, README's testing paragraph if it names the chaos test, and the README table of this ticket folder filled with the results. Trigger the nightly workflow by hand twice after the branch is merged (`gh workflow run ci.yml`) and record the run ids.

**Blocked by:** #15 to #28.

**Issue:** #22. **Wave:** 4. **Files (lock in bold):** CHANGELOG.md; AGENTS.md (chaos sentence); README.md; this README (results); tests/chaos.rs (the budget constant only)

**Status:** proposed

- [ ] 20 of 20 runs pass per format; no iteration over the budget; the slowest iteration of each run recorded; the budget constant set from them with its margin.
- [ ] Every replay in the README table passes.
- [ ] Benchmark: the median of three runs on every keystroke-latency row and `big-file-peak` within 5% of main's median; recorded in the README.
- [ ] `tests/w3c_sparql.rs` 425/426 or better; `tests/sparql_golden.rs` green; W3C Turtle, TriG, SHACL suites green.
- [ ] `grep -n "#\[ignore\]\|todo(" tests/chaos.rs` is empty; no `#[ignore]`, `todo` or exemption added anywhere in the branch (`git diff main` grep in the report).
- [ ] CHANGELOG, AGENTS.md, README updated; departures logged in the ticket README.
- [ ] Two nightly runs green after merge (run ids in the README).
