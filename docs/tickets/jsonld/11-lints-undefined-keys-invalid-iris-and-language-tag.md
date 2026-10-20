# 11: JSON-LD: lints (undefined keys, invalid IRIs and language tags, vocabulary terms)

**What to build:**
- **`jsonld-undefined-term`** (warning): a key the active context does not define, which JSON-LD would silently drop.
- **`invalid-iri`** and **`invalid-language-tag`**, at the value.
- **The existing `unknown-term` and `deprecated-term`** apply to vocabulary IRIs reached through the context, with "did you mean".

**Blocked by:** 08

**Status:** ready-for-agent

**Parallel slices:** owns a JSON-LD lint module. It does not touch SHACL (10) or workspace code (09).

- [ ] Each code appears at the exact position in the editor and in `check`
- [ ] No warnings on the whole corpus except true positives (each documented)
- [ ] Phase-1 pending lint tests are promoted

Spec: docs/specs/jsonld.md ("Diagnostics").
