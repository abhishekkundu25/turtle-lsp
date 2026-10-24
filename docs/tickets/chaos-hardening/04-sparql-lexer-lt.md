# 04: SPARQL lexer: `<` after an operand is the operator (class D precondition)

**What to build:** In src/sparql/lexer.rs, `<` opens an IRI only where an IRI can start; after an operand (a variable, a literal, a number, `)`, `]`, a prefixed name, a keyword that ends an expression) it is the `<` or `<=` operator, as spargebra reads it. Today the lexer reads `<` as an IRI opener whenever a `>` follows later with no `bad_in_iri` character between (lexer.rs around lines 195 and 384), so `?a<=ex:b&&?c>?a` is one IRI token to us and three tokens to spargebra; `check` then reports the prefix unused and `check --fix` deletes it. The two lexers must agree on every token boundary, because `token_edits` (ticket 01) and `algebra()` in src/sparql/format.rs both rely on it.

**Blocked by:** nothing.

**Issue:** #25. **Wave:** 1. **Files (lock in bold):** src/sparql/lexer.rs

**Status:** proposed

- [ ] lexer tests: `?a<=ex:b&&?c>?a` lexes as `?a`, `<=`, `ex:b`, `&&`, `?c`, `>`, `?a`; `?a<<http://e/b>` lexes as `?a`, `<`, IRI; `{ <http://e/s> <http://e/p> ?o }` still lexes three IRIs; `FILTER(?x < ?y && ?y > ?z)`; `<<` and `>>` RDF-star tokens unchanged.
- [ ] A test that tokenizes every query in the W3C SPARQL corpus and `test/corpus` and compares the boundaries of IRI tokens against spargebra's parse (no IRI in our stream that spargebra parses as an operator, and the reverse), run under `timeout 120`.
- [ ] `check` on `PREFIX ex: <http://e/> SELECT * { ?s ?p ?a FILTER(?a<=ex:b&&?c>?a) }` reports no unused prefix; `check --fix` leaves the query unchanged.
- [ ] The highlighting and completion tests in src/sparql/ unchanged apart from cases that asserted the old reading, each listed in the report.
