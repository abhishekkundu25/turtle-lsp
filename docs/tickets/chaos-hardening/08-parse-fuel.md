# 08: Parse fuel in the vendored grammar (class E, backstop)

**What to build:** In vendor/spargebra/src/parser.rs, `ParserState` carries a fuel counter; `Expression`, `GroupGraphPattern` and `PrimaryExpression` decrement it through a `{? }` predicate and the parse fails with a distinct error ("query too complex") when it reaches zero. Fuel is `k x input bytes`, with `k` measured as the largest rule visits per byte over the W3C and corpus queries on the factored grammar, times a margin of at least 4, recorded in the constant's comment with the measurement. src/sparql/diagnostics.rs maps the error to the existing "too complex" diagnostic. This replaces the static construct table proposed in the first draft (rejected in review: it fired only on constructs the linearity test already covers, and its unit multiplied by the whole document's token count). The depth guard `too_complex` stays as it is. No change to `check_tokens`'s two parses (the token-spaced reparse places errors on token boundaries) and no removal of `algebra()` in src/sparql/format.rs (it is the check that catches our lexer and spargebra disagreeing).

**Blocked by:** #26.

**Issue:** #19. **Wave:** 3. **Files (lock in bold):** vendor/spargebra/src/parser.rs (`ParserState` fuel); src/sparql/diagnostics.rs (the "too complex" message wiring); **src/sparql/tests.rs** (new test fns); test/shacl-sparql-lint.test.mjs; test/cli.test.mjs; web/test/engine.test.mjs

**Status:** proposed

- [ ] src/sparql/tests.rs: with the measured fuel, every W3C and corpus query parses with no "too complex" diagnostic, and so does every input of ticket 07's linearity tests; with fuel forced to a small value (a test-only constructor), a 200-token query is refused with the "too complex" message and a span.
- [ ] A test that re-introduces the old `RegexExpression` shape in a test-only grammar copy is not required; instead the fuel constant's comment names the measured visits per byte and the test asserts the measured corpus maximum is under 1/4 of the fuel.
- [ ] test/shacl-sparql-lint.test.mjs: a `.ttl` whose `sh:select` holds `FILTER(` + `REGEX(` x 200 + `?x)` makes `check` exit 1 with one diagnostic within the test timeout, and the server indexes it with no stall (a `documentSymbol` answered while indexing).
- [ ] test/cli.test.mjs: `query -e` with the same filter returns an error at once.
- [ ] web/test/engine.test.mjs: the browser build checks the same query in bounded time.
- [ ] tests/chaos.rs "Pathological inputs": `survive("nested regex", ...)`, `survive("nested not", ...)`, `survive("nested custom call", ...)` under the existing time bound (ticket 12 may move them; whichever lands first adds them).
