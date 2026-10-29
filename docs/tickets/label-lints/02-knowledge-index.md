# 02: Knowledge: term rule, label and definition index, own deprecations

**What to build:** in `src/knowledge.rs`, per workspace source (never External), computed in `DocFacts::new` and added/subtracted in `apply` like the other facts:

- The term rule: `Knowledge::is_term(iri)`: some workspace document types the IRI `owl:Class`, `rdfs:Class`, `owl:ObjectProperty`, `owl:DatatypeProperty`, `owl:AnnotationProperty`, `rdf:Property` or `skos:Concept`, or makes it the subject of `rdfs:subClassOf` or `rdfs:subPropertyOf`.
- A label index: (label trimmed and case-folded, language tag lowercased or none) -> terms, from `rdfs:label` and `skos:prefLabel` triples with an IRI subject. `Knowledge::labelled(label, lang) -> Vec<Iri>`: the terms (by the rule) carrying that label, sorted, unique; it folds its input itself.
- A definition index on the same design: a term's definition is its `skos:definition`, or its `rdfs:comment` when no workspace document gives it a `skos:definition`. `Knowledge::defined_as(definition, lang) -> Vec<Iri>`.
- Own deprecations: `Knowledge::deprecated_here(iri) -> bool`: some workspace document says `owl:deprecated` with a boolean true (`true`, `"true"^^xsd:boolean`, JSON-LD `true`), and none says false.
- Invalidation: bump `described_generation` when a change can alter another document's curation diagnostics: a label or definition key gains or loses a term while it has two or more terms before or after, an IRI becomes or stops being a term, or the deprecated set changes. An ordinary keystroke inside one label that collides with nothing must not bump it.

**Blocked by:** 01.

**Status:** ready-for-agent

- [ ] Unit tests in `knowledge.rs`: term rule (each type, subClassOf, instance data not a term, External source not counted); `labelled` folds case and trims, keeps languages apart, untagged separate; removal and replacement subtract exactly; `defined_as` with the comment fallback; `deprecated_here` for the three true forms, false, and false winning; the generation bumps on a new collision and not on a non-colliding label edit.
- [ ] The existing knowledge tests and `tests/chaos.rs` pass.
