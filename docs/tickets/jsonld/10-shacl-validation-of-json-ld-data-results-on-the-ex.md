# 10: JSON-LD: SHACL validation of JSON-LD data, results on the exact key

**What to build:**
- Turtle (or JSON-LD) shapes validate JSON-LD data.
- Each result sits on the key or value that breaks the shape. A missing property sits on the node's `@id`, or on its opening brace.
- JSON-LD named graphs follow the TriG validation-unit rules, including the dataset de-duplication.
- Shapes written in JSON-LD are used.

**Blocked by:** 08

**Status:** ready-for-agent

**Parallel slices:** owns the SHACL validation feature.

- [ ] Phase-1 pending SHACL tests are promoted
- [ ] A JSON-LD dataset mentioning a shared node does not repeat shared results
- [ ] SHACL-SPARQL batching works for JSON-LD focus nodes

Spec: docs/specs/jsonld.md ("Workspace, SHACL, CLI and web").
