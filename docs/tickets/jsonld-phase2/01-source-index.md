# 01: JSON-LD source index

**What to build:** expansion records, on the JSON-LD document, every key, `@type` value and `@id` value with its span, expanded IRI and written kind (keyword, alias, term, compact with prefix length, `@vocab`-relative, absolute); every term definition with its key span, IRI span and the document it is written in (this one, a workspace context file's URL, or none for bundled/remote); and each JSON object's span with the active context in force inside it. Prefactor only: no LSP behaviour changes.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**Parallel slices:** lock on src/jsonld/{expand,context,mod}.rs (loader.rs if the context file's URL needs carrying). Runs beside 02 and 03.

- [ ] Unit tests at the JSON-LD module seam: a document with a scoped context, an alias, a compact IRI, a `@vocab`-relative key and a workspace context file gives the expected uses and definitions
- [ ] Given an offset, the innermost scope's active context is returned (also on a document with a JSON syntax error before the cursor)
- [ ] W3C suites, oracle and lints tests unchanged; jsonld_budget still passes in release
