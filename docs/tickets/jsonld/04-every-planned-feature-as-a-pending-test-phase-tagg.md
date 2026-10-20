# 04: JSON-LD: every planned feature as a pending test (phase-tagged)

**What to build:** the JSON-LD behaviour of both phases written as Node tests through the LSP protocol and the CLI, each marked `todo` with a tag (`jsonld:phase1` or `jsonld:phase2`), so they run and report without failing the build.
- **Phase 1:** JSON syntax errors with recovery, context errors, undefined keys, invalid IRIs and language tags, cross-file knowledge (Turtle sees JSON-LD terms and back), undescribed-term silence, SHACL results on the exact key, named graphs, `check` output and exit codes, the browser engine.
- **Phase 2:** completion of context and `@vocab` terms, hover with the expanded IRI, go to definition into `@context`, references, rename (including the `@context` entry), formatting (idempotent, graph-preserving), semantic tokens, outline.

Assertions use codes and ranges, never message text.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** owns new test files only.

- [ ] `npm test` reports the pending count per phase tag
- [ ] Each test documents its expected positions explicitly
- [ ] No existing test changes

Spec: docs/specs/jsonld.md (Testing Decisions: "Pending feature tests").
