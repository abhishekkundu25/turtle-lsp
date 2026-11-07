# 03: Hover with proofs

**What to build:** hover on a resource, with reasoning on, adds an "Inferred" section after today's hover: each inferred `rdf:type` of the resource with its proof, steps naming their W3C rule, each base triple a `[file:line](uri#Lline)` link as "Described in" writes them (imports as `origin:line`). A type that holds through `owl:sameAs` shows the sameAs chain. The proof is computed through 01's read API with a 200 ms deadline: past it, the hover says the proof is still being worked out and the search finishes in the background so the next hover is instant. While a run holds the state, the hover says reasoning is running. Asserted types are not repeated as inferred. With reasoning off, hover is exactly today's.

**Blocked by:** 01.

**Status:** done (77b8503)

- [ ] `src/reasoning/proof.rs`: proof to markdown, unit-tested.
- [ ] New `test/reasoning-hover.test.mjs`: an inferred type with its proof and a link to the right line; a type through `owl:sameAs`; reasoning off gives today's hover.
- [ ] `test/knowledge.test.mjs` still passes.
