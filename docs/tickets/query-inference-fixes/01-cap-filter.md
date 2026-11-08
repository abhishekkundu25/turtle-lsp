# 01: Cap and filter the inferred triples, limits during conversion

**What to build:** spec items 1 (native part) and 2. In `reasoning::infer`,
keep an inferred triple only when its subject or object is a term of a
queried document; stop with "too many" once the kept count passes
`Inference::max_triples`; check the query's watch (time limit, cancel) every
64k inferred triples. A new `Inferred` case for "too many" (the limit), matched
by every renderer. The query graph build checks the clock (`check_now`).

**Blocked by:** none. **Status:** done (af3b10e)

- [ ] `src/query/tests.rs`: a transitive chain whose closure passes a small cap gives the "too many" note and the results of off; an `owl:sameAs` chain likewise; an import-only subclass chain adds no `rdfs:subClassOf` row about the import's classes while an import axiom still types a data individual; a time limit that passes during conversion stops near it (`timing_`).
- [ ] Every renderer prints the new case: CLI stderr (a `test/query-cli.test.mjs` case), "▶ Run query" line and `turtle/query` member (unit tests), browser engine member (compiles; `node web/build.mjs`).
- [ ] fmt, clippy, lib unit tests, the three query Node files, `web/test/engine.test.mjs`.
