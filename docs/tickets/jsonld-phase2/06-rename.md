# 06: Rename in JSON-LD

**What to build:** prepare-rename and rename for terms and keyword aliases (definition key plus every use resolving to that definition; workspace context files renamed with their users; bundled/remote refused with a message; invalid names refused), and IRI rename from any syntax writing each JSON-LD use in its own form (compact, `@vocab`-relative or its fallback, absolute without brackets, or the term's definition IRI once).

**Blocked by:** 05

**Status:** ready-for-agent

**Parallel slices:** lock on src/features/navigation.rs; the JSON-LD edit logic in a new src/jsonld/rename.rs.

- [ ] Promote: the three rename tests
- [ ] Added: bundled-term rename refused; invalid new name refused; term renamed from a workspace context file; IRI rename leaving `@vocab` becomes compact; every edited JSON-LD document keeps its graph apart from the renamed IRI (toRdf compare)
