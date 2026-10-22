# 04: Query stop semantics

**What to fix:** an ASK stopped by the time limit or a cancellation can return a wrong
boolean as a finished answer. The patched joins return `Cancelled` once; `MINUS` drops
its right side's errors (`filter_map(Result::ok)`), so the right side is silently cut
short. The `Boolean` arm (`src/query/mod.rs:344`) checks nothing, and the final guard
(`:377-382`) checks only `kept_too_many()`. Reproduced: `ASK { VALUES ?d { ex:s2999 }
MINUS { ?a ?b ?c . ?d ?e ?f } }` over 9,000 triples with `--timeout 0.2` prints `true`
(exit 0); the real answer is `false` (exit 1). Three related limits: query planning is
about cubic in patterns or path steps and never checks the clock (a 1,000-step path with
`--timeout 1` stopped after 22 s; `:305-317`); `query.maxBufferedRows` counts rows, so a
`GROUP_CONCAT` cell grows without bound (3.4 GB reproduced by the slice; the README
promises "about 1 GB"); a run replaced by a newer one answers success with the string
"cancelled: a newer run replaced it" (`src/server/run_query.rs:204-207`, ticket 13's
file: only note it here).

**Files owned:** `src/query/mod.rs`, `src/query/tests.rs`, `src/query/render.rs`,
`tests/query_budget.rs`, `tests/query_cancel.rs`.
**Hands off:** `vendor/spareval/**` (patch changes need their own decision),
`src/server/run_query.rs`, `src/cli/query.rs`.

**Blocked by:** nothing.

**Status:** ready-for-agent

- [ ] After `prepared.execute` and after each result loop, if `watch.stopped()` or the
      token is cancelled, return `Err(Timeout)` / `Err(Cancelled)`; never a boolean or a
      row set that depends on a dropped error.
- [ ] Tests beside `a_stop_whose_error_is_dropped_is_not_a_partial_answer`: ASK with
      `MINUS` over an in-memory cross product under a time limit and under a cancel
      flag both error, for `Boolean`, `Solutions` and `Triples`.
- [ ] A document-level cap before `prepare`: BGP size and path length above N (choose
      N from a measurement; record it) give `QueryError::TooComplex` at once. Test: a
      1,000-step path with a 1 s limit returns within the limit plus a small margin.
- [ ] `render::markdown` and `render::table` cap a cell's length (note the truncation);
      `Triples::omitted` doc says "approximately".
- [ ] `tests/query_*.rs`, `src/query/tests.rs`, `test/query-cli.test.mjs`,
      `test/query-lsp.test.mjs` green.
