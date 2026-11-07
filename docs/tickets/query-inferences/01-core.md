# 01: The query core reasons over its snapshot

**What to build:** the one entry point every query path calls (`query::run`)
takes the rule set for inferred triples (the existing `settings::Reasoning`:
`Off`, `Default`, `Complete`), the documents reasoning may read besides the
queried ones (imports), and the size limit. With `Off` it does exactly what
it does today and builds nothing. Otherwise it builds the `reasoning`
module's base over the queried documents plus the imports, runs TurbOWL with
the matching switches, and adds only the inferred triples
(`Reasoner::inferred`) to the query graph's one default graph before
evaluating. Over the size limit it adds none. The query graph gains a way to
take triples that are not from a document's parse, with blank nodes named as
the base names them so they are the graph's own blank nodes.

The run reports what it added as one sum type, beside the `Answer` (the
`Answer` enum itself stays as it is, so renderers that ignore the note keep
compiling): none (off), added (rules, inferred count, findings count), or
skipped (base size, limit). Reasoning time counts against the timeout: the
watch is checked before and right after reasoning, and a stopped query does
not evaluate.

Also here, so the path tickets edit disjoint files: the `query.reasoning`
setting (`off` default, `default`, `complete`; settable in `turtle-lsp.json`)
in `Settings`, and the batch query passing `Settings`' value and
`turtle.reasoning.maxTriples` into the core (no imports there). The two
editor extensions pass `Off` and ignore the note, nothing more.

**Blocked by:** none.

**Status:** open

- [ ] Query unit tests with `Default`: a class inferred through a range, a subclass chain, an inverse, a transitive property, a property chain and an `owl:sameAs` merge each match; a pattern joining a written and an inferred triple through a blank node matches; an imported axiom infers while the imported document's own triples are not in the results; CONSTRUCT includes inferred triples; ASK answers over them.
- [ ] With `Off`, the existing query tests pass unchanged and the note is none.
- [ ] Over the size limit: the note is skipped with the base size and the limit, and the results equal `Off`'s.
- [ ] A timeout that passes during reasoning stops the query before it evaluates.
- [ ] A workspace with a contradiction still answers, and the note counts its findings.
- [ ] Settings unit test: `query.reasoning` parses from settings and from `turtle-lsp.json`, defaults to `off`, and an unknown value is ignored.
- [ ] `cargo fmt --check`, clippy, the lib unit tests, the doc test, and `node web/build.mjs` (the engine still compiles) pass.
