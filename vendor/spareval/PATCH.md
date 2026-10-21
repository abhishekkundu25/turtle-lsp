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

## Removing the patch

Once a spareval release has the same checks, delete this folder and the
`[patch.crates-io]` entries in `Cargo.toml` and `web/engine/Cargo.toml`, then
run `cargo update -p spareval` in both places. The tests that cover this are
`a_join_in_memory_stops_*` in `src/query/tests.rs` and the in-memory join
test in `web/test/engine.test.mjs`.
