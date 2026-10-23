# spargebra 0.4.7, patched

This folder holds spargebra 0.4.7 as published on crates.io (oxigraph commit
in `.cargo_vcs_info.json`, `lib/spargebra`), with one change to
`src/parser.rs`. Both `Cargo.toml` files use it through `[patch.crates-io]`. spargebra is licensed
MIT OR Apache-2.0; we use it under MIT (`LICENSE-MIT`, copied from
`vendor/spareval`, which has the same oxigraph licence text).

## Linear parse of nested calls

`left-factor-nested-calls.patch` is the diff of `src/parser.rs` against the
published crate, in oxigraph's tree (`a/lib/spargebra/src/...`), so the same
file can go upstream. It has not been sent yet, nor checked against
oxigraph's main branch.

- The grammar is a PEG without memoization. Unpatched, five rules have
  ordered alternatives that share a prefix holding an `Expression`. When the
  first alternative fails after that prefix, the next one parses it again,
  and so does every level inside it: the time doubles with each level of
  nesting. `SELECT (AVG(` + `REGEX(` x 20 + `DISTINCT ?x) AS ?y) {}` took
  7 s; at 223 levels it does not finish.
- `REGEX` and `SUBSTR` parse `( a , b` once and then an optional `, c`;
  `REPLACE` parses `( a , b , c` once and then an optional `, d`.
- Every aggregate, and the custom-function arm, parses an optional
  `DISTINCT` (the new rule `Distinct`) and then its `Expression` once;
  `GROUP_CONCAT` then parses an optional `; SEPARATOR = s`. `Distinct` is
  not the keyword when it starts a prefixed name, since SPARQL tokens match
  longest: `distinct1:x`, `DISTINCT:a` and `DISTINCT.a:b` are names.
  Unpatched, the keyword was tried first and the name second.
- `UnaryExpression`'s first arm (`!` followed by a unary expression) now
  starts only when an operator (`!`, `+`, `-`) follows the `!`. Unpatched,
  `!(x)` was parsed by that arm, rejected by its action without the
  `sparql-12` feature, and parsed again by the second arm.
- The same strings give the same algebra, checked by
  `parsed_queries_match_the_golden_record` in `tests/w3c_sparql.rs`, with one
  exception. With the `:` prefix declared, `COUNT(DISTINCT:a)` was read as
  `DISTINCT` followed by `:a`, and it is now the name `DISTINCT:a`.
- The tests are `timing_nested_builtins_parse_in_linear_time`,
  `timing_every_builtin_nests_in_linear_time` and
  `distinct_that_starts_a_prefixed_name_is_the_name` in
  `src/sparql/tests.rs`.

## Review rule for a patch

No failure after a recursive nonterminal may be followed by an alternative
that parses the same text again. Each failure point counts: the next
alternative of the same rule, an alternative of a calling rule that starts
at the same place, and a `{? }` action that returns `Err` after a recursive
parse. Without memoization each such point doubles the work per level of
nesting. Shared prefixes made only of terminals are fine.

## Removing the patches

Once a spargebra release has every change, delete this folder and the
`[patch.crates-io]` entries in `Cargo.toml` and `web/engine/Cargo.toml`, then
run `cargo update -p spargebra` in both places.
