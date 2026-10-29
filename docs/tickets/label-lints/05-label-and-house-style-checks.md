# 05: `duplicate-label`, `multiple-labels` and the house-style checks

**What to build:** in `features::curation`, using 02's `is_term`, `labelled` and `defined_as`:

- `duplicate-label` (on): each `rdfs:label`/`skos:prefLabel` literal of a term in this document whose (folded, language) key other terms carry. The message names the other terms by the prefixed name this document can write (else `<iri>`), at most a few then "and N more". A term never collides with itself.
- `multiple-labels` (on): each label of a term that has more than one value of the same property in the same language within this document.
- With `house_style` only (each reported only when enabled, the composer filters): `missing-label`, `missing-definition` on the term's subject where this document describes it (once per term per document, the first description); `multiple-definitions`, `duplicate-definition`, `lowercase-definition` on the literals; `missing-ontology-title` / `-description` / `-license` on each `owl:Ontology` subject (title: `dcterms:title`, `dc:title`, `rdfs:label`; description: `dcterms:description`, `dc:description`, `rdfs:comment`; licence: `dcterms:license`, `cc:license`, both `http://creativecommons.org/ns#license` and `http://web.resource.org/cc/license`).
- A "missing" check for a term described in several documents looks at all of the workspace's descriptions, not only this document's (use `Knowledge::descriptions`).
- Remove the `#[allow(dead_code)]` lines 01 left.

**Blocked by:** 02, 04.

**Status:** blocked

- [ ] Unit tests in `curation.rs` through a small `Workspace`: duplicate across two files, language and case rules, instance data skipped, self-collision, External source skipped, message names; multiple-labels per language and per property; each house-style check on and off through `Settings` (`lint.enable`, ignore wins) via `diagnose::document`; header alternatives.
- [ ] `test/label-lints.test.mjs`: a duplicate across two open files, and an edit to one that resolves it clears the other file's warning (after the described pause); `lint.enable` from the file turns on `missing-label`.
- [ ] `tests/chaos.rs` and the lib tests pass.
