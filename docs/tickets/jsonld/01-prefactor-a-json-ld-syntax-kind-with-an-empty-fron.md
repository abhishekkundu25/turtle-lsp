# 01: JSON-LD prefactor: a JSON-LD syntax kind with an empty frontend

**What to build:** `.jsonld` files (and the `jsonld` language id) are recognised as a new RDF syntax everywhere a syntax is chosen: the server, the workspace index, the CLI's file collection and the browser engine. The frontend is a stub that returns an empty parse with no errors. It defines the public engine interface the later tickets fill in: parse with a base and a context loader, producing the shared parse model. This is the one wide change, every exhaustive match over syntaxes, so it lands first and alone.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**Parallel slices:** lock ticket (the syntax enumeration and document-kind detection). Nothing else runs alongside it.

- [ ] Opening a `.jsonld` file starts no Turtle parse errors and publishes empty diagnostics
- [ ] `turtle-lsp check` lists `.jsonld` files as checked (with no problems yet)
- [ ] The engine interface (parse entry point, context loader trait, span-carrying result) exists and is documented
- [ ] All existing suites pass unchanged; the web engine still builds for wasm32

Spec: docs/specs/jsonld.md (Implementation Decisions: "One parse model", "New syntax kind").
