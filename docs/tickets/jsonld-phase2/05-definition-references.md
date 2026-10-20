# 05: Definition, references and highlights on JSON-LD terms

**What to build:** on a term or alias: go to definition reaches its `@context` entry (this document or a workspace context file), falling back to the IRI's workspace description for bundled/remote terms; references list the definition, the uses in this document and the IRI's uses in the workspace; highlights mark definition (write) and uses (read).

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** lock on src/features/navigation.rs (06 follows).

- [ ] Promote: "go to definition on a key goes to its @context entry", "…into a context file", "references on a key list its @context entry…"
- [ ] The phase-1 definition and hover tests on `@vocab` keys still pass
