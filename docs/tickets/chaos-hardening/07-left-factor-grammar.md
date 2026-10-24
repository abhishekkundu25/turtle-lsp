# 07: Left-factor the grammar (class E, root cause)

**What to build:** In vendor/spargebra/src/parser.rs, left-factor the five rules that are exponential in nesting depth (the review read every ordered choice in the grammar and confirmed these are the only ones): `RegexExpression` to `REGEX ( a , b ("," c)? )`; `SubstringExpression` the same; `StrReplaceExpression` to `REPLACE ( a , b , c ("," d)? )`; `GROUP_CONCAT ( DISTINCT? e (";" SEPARATOR "=" s)? )` and `DISTINCT?` for the other aggregates and the custom `iri ( DISTINCT? e )`; `UnaryExpression = "!" UnaryExpression / ("+" / "-")? PrimaryExpression` where the second alternative no longer accepts `!`, keeping the behaviour without the `sparql-12` feature (today alternative 1's action rejects `!` and the second arm accepts it). Same algebra for the same strings, proven by the golden from ticket 06. Write the review rule in PATCH.md: no failure after a recursive nonterminal may be followed by an alternative that parses the same text again (this covers a prefix shared across rules and a `{? }` action that fails after a recursive parse, not only two alternatives of one rule). List each change in PATCH.md and prepare the upstream diff as a file in vendor/spargebra/. Re-measure `MAX_NESTING` and the chain limits in src/sparql/diagnostics.rs against stack overflow, since the factored rules generate different frames, and update the comments with the new figures.

**Blocked by:** #18.

**Issue:** #26. **Wave:** 2. **Files (lock in bold):** vendor/spargebra/src/parser.rs; vendor/spargebra/PATCH.md and the upstream diff; src/sparql/diagnostics.rs (message tests, `MAX_NESTING` re-measurement); **src/sparql/tests.rs** (new test fns)

**Status:** proposed

- [ ] `tests/sparql_golden.rs` byte-identical before and after.
- [ ] src/sparql/tests.rs `timing_nested_builtins_parse_in_linear_time`: for `REGEX(`, `SUBSTR(`, `REPLACE(`, `GROUP_CONCAT(`, `GROUP_CONCAT(DISTINCT `, `!(`, `(!`, `!REGEX(`, `:f(REGEX(` at n = 200, open and closed with valid arguments, on a 2 MiB stack (`on_small_stack`): the open form gives a diagnostic, the closed form none, and the n = 200 time is under 4 x the n = 50 time plus 5 ms.
- [ ] src/sparql/tests.rs `timing_every_builtin_nests_in_linear_time`: every built-in and aggregate name from the lexer's table nested 64 deep with its arity inside `SELECT (... ?x) {}` parses within the same bound.
- [ ] The 107183 and 107745 inputs, generated in the test, give a diagnostic within the bound.
- [ ] `syntax_message` and `describe` unit tests in src/sparql/diagnostics.rs green (expected strings updated only where peg's "expected" set changed, each listed in the report).
- [ ] `tests/w3c_sparql.rs` at 425/426 or better; `tools/check-vendor.sh` passes with the new patch list.
- [ ] `MAX_NESTING` and the chain limits re-measured; the stack-overflow tests in src/sparql/tests.rs still pass on a 2 MiB stack.
