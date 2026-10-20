# 08: JSON-LD: expansion and toRdf into the shared parse model

**What to build:** expansion and deserialization to RDF. A JSON-LD document produces the same parse model as Turtle: triples with subject, predicate and object spans; references with roles; errors; named graphs as TriG does. Covers lists, `@reverse`, `@nest`, `@included`, `@json` and value objects.

**Blocked by:** 07

**Status:** ready-for-agent

**Parallel slices:** owns the expansion and toRdf modules.

- [ ] W3C toRdf and expand pass, except a short documented known-fail list
- [ ] The corpus oracle passes on every corpus file
- [ ] Each triple's spans point at the key, `@id` or value that produced it (spot-checked by tests)

Spec: docs/specs/jsonld.md ("Engine parts").
