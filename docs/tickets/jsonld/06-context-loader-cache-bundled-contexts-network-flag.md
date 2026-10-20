# 06: JSON-LD: context loader (cache, bundled contexts, network flag)

**What to build:** the desktop and browser implementations of the context loader interface. Order: file-relative contexts, the existing import cache, bundled common contexts (schema.org, ActivityStreams, Verifiable Credentials), then the network only with `imports.network` / `--network`. Fetched contexts are cached in the imports cache directory with its offline rules. Nothing goes online on open by default.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** owns the loader module and the bundled context files; touches the imports cache only through its existing interface.

- [ ] Offline, a document using the schema.org context loads it from the bundle
- [ ] With the network off, an unknown remote context is reported (not fetched)
- [ ] With the network on, it is fetched once and served from the cache afterwards
- [ ] The browser engine uses a loader backed by the bundled contexts

Spec: docs/specs/jsonld.md ("Context loading").
