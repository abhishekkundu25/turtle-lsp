# spareval 0.2.7, patched

This folder holds spareval 0.2.7 as published on crates.io (oxigraph commit
`0e81a29d`, `lib/spareval`), with one change to `src/eval.rs`. Both
`Cargo.toml` files use it through `[patch.crates-io]`. spareval is licensed
MIT OR Apache-2.0; we use it under MIT (`LICENSE-MIT`, from the same commit).

## The change

`cancellation-in-joins.patch` is the full diff against the published crate.

- Unpatched, the evaluation checks its `CancellationToken` only when it reads
  the dataset. A join combines rows it has already read without reading
  anything, so a cross product, or two property paths joined without a
  shared variable, can run for minutes after the token is canceled.
- The patched cross product, hash join and hash left join check the token
  for each probe row and every 1024 rows they combine. When the token is
  canceled, they return `QueryEvaluationError::Cancelled` once and then end.
- `CancellationToken::with_check` makes a token that is also canceled once a
  function returns `true`. turtle-lsp uses this for its time limit. It works
  in the browser build too, which has no thread to cancel the token at the
  deadline.
- `CancellationToken::with_max_kept_tuples` also cancels once the evaluation
  holds more than a number of tuples in memory at once: the cross product,
  hash join, hash left join and MINUS builds, ORDER BY, DISTINCT and GROUP BY
  groups count theirs until the buffer is dropped. `kept_too_many()` stays
  true once the limit was passed. turtle-lsp uses it for
  `query.maxBufferedRows`.
- Once the token is canceled, a dataset read returns the cancellation once
  and ends, instead of an error for every remaining quad (which GROUP BY,
  ORDER BY and join builds kept).
- ORDER BY returns the cancellation instead of sorting what it gathered once
  the token is canceled (sorting tens of millions of rows took seconds).

## Removing the patch

Once a spareval release has the same checks, delete this folder and the
`[patch.crates-io]` entries in `Cargo.toml` and `web/engine/Cargo.toml`, then
run `cargo update -p spareval` in both places. The tests that cover this are
`a_join_in_memory_stops_*`, `a_hash_join_in_memory_stops_at_the_time_limit`
and `a_query_that_keeps_too_many_rows_stops` in `src/query/tests.rs`,
`tests/query_budget.rs`, `tests/query_cancel.rs`, and the in-memory join
test in `web/test/engine.test.mjs`.
