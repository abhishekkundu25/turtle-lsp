# 02: `files`: every other document reasons

**What to build:** spec item 3. `turtle/query` with `files` passes every
workspace document not in `files`, and every external import, to the core as
reasoning-only input.

**Blocked by:** none. **Status:** done (17d06bc)

- [ ] `test/query-request.test.mjs`: `data.ttl` imports an ontology that `onto.ttl` in the workspace declares; `files: [data.ttl]` with `reasoning: "default"` infers `ex:p1 a ex:Device`; a triple only `other.ttl` holds is not in the results.
- [ ] fmt, clippy, lib unit tests, `test/query-request.test.mjs`.
