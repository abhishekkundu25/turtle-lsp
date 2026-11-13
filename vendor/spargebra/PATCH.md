# spargebra 0.4.7, patched

This folder holds spargebra 0.4.7 as published on crates.io (oxigraph commit
in `.cargo_vcs_info.json`, `lib/spargebra`), with five patches,
applied in this order. Both `Cargo.toml` files use it through `[patch.crates-io]`. spargebra is licensed
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

## Parse fuel

`parse-fuel.patch` (on top of the first, same tree layout) bounds the work
of a parse, the backstop for a rule that parses the same text again and
that the linearity tests miss.

- `ParserState` counts the visits of `Expression`, `GroupGraphPattern` and
  `PrimaryExpression` (the rule `fuel`, a `{? }` predicate). Past
  `FUEL_PER_BYTE` times the input's length in bytes, every further visit
  fails, so the parse ends in time linear in the input, and fails with the
  error `error at LINE:COLUMN: expected query too complex`. The rule
  `refuel` resets the count at the start of each pass (peg parses again to
  report an error).
- `FUEL_PER_BYTE` (exported in `src/lib.rs`) has the measurement it is set
  from in its comment.
  `SparqlParser::with_fuel_per_byte` (hidden) replaces it, for tests.
- The tests are `a_quarter_of_the_fuel_parses_every_known_query`,
  `the_fuel_parses_nested_calls_left_open` and
  `a_query_out_of_fuel_is_too_complex` in `src/sparql/tests.rs`, which maps
  the error to the "too large to check" diagnostic.

## Longest prefixed name

`longest-prefixed-name.patch` (on top of the second, same tree layout)
makes a prefixed name the longest text it can be, as SPARQL tokens are.

- `PrefixedName` tried `PNAME_LN` and then `PNAME_NS`. When `PNAME_LN`
  matched but its action failed, because the prefix and the local name do
  not make an IRI, `PNAME_NS` took the prefix alone and left the local name
  to be read as the next term. With `PREFIX : <http://www.example.org>`,
  `{ ::p ?o . }` was read as `:` `:p` `?o` and accepted, although `::p` is
  one name (the local name `:p`) and `http://www.example.org:p` is not an
  IRI (issue #34).
- `PNAME_NS` as a term now has to be followed by something that cannot
  continue a local name (`!PN_LOCAL()`), so such a name is an error.
- The tests are `a_local_name_that_starts_with_a_colon_is_one_name` in
  `src/sparql/tests.rs` and `iri_name_and_less_than_boundaries_agree_with_spargebra`
  in `tests/w3c_sparql.rs`, which now also replaces each prefixed name with
  an IRI.

## Characters past U+FFFF in names

`supplementary-chars.patch` (on top of the third, same tree layout) adds
`'\u{10000}'..='\u{EFFFF}'` to `PN_CHARS_BASE`, the last range of SPARQL
1.1's production 164.

- Unpatched, the rule stopped at `#xFFFD`, so `ex:😀a`, `?😀` and `_:😀`
  were syntax errors. The lexer (`syntax/names.rs`) has the full range, so
  "Shorten IRI" turned `<http://example.org/😀a>` into a name the query then
  failed to parse (issue #109).
- oxigraph's main branch has the full range in its new lexer.
- The test is `names_may_use_characters_past_the_basic_multilingual_plane`
  in `src/sparql/tests.rs`.

## A keyword that starts a prefixed name

`keyword-boundary.patch` (on top of the fourth, same tree layout) makes a
keyword match only where a prefixed name could not go on from it, since
SPARQL tokens match longest.

- `i(literal)`, the rule every keyword goes through, matched the keyword's
  letters whatever followed. With `a:` declared, `GRAPHa:g { ... }` was read
  as `GRAPH` and `a:g`, a GRAPH pattern nobody wrote, and passed; so did
  `GRAPH:g` with `:` declared, `FROMa.b:g` and the others. The fourth patch
  widened it to names past U+FFFF (`GRAPH😀:g`).
- The new rule `keyword_end` fails when the text after a keyword continues
  a PN_PREFIX up to a `:`, the check `Distinct` had alone (the first
  patch). `i` ends with it, and so do `a`, `true` and `false`, which the
  grammar matches as plain strings because they are case-sensitive: with
  `:` declared, `?s a::p` was `?s rdf:type :p` although `a::p` is one name.
  `Distinct` no longer repeats the check. Text after a keyword that cannot
  make a name (`LIMIT 1`, `FILTER(`, `DISTINCT-1`, `a<x>`) is unchanged.
- The test is `a_keyword_that_starts_a_prefixed_name_is_the_name` in
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
