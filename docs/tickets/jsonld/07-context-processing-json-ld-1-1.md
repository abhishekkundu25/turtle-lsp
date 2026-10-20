# 07: JSON-LD: context processing (JSON-LD 1.1)

**What to build:** the 1.1 context processing algorithm over parsed JSON with spans: active context, term definitions, `@vocab`, `@base`, `@language`, `@direction`, `@protected`, `@propagate`, `@import`, scoped contexts and keyword aliases. Processed contexts are cached by URL and content. Errors (invalid definitions, cycles, protected redefinitions) are `jsonld-context` diagnostics on the offending definition.

**Blocked by:** 05

**Status:** ready-for-agent

**Parallel slices:** owns the context-processing module. It uses the loader interface from 01, so it does not wait for 06.

- [ ] Context errors show at the exact definition in the editor and in `check`
- [ ] Re-checking a document with an unchanged remote context does not re-process it
- [ ] The relevant W3C expand tests move from known-fail to pass (expected-status manifest updated)

Spec: docs/specs/jsonld.md ("Engine parts").
