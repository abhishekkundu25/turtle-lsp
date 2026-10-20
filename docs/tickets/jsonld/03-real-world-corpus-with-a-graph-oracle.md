# 03: JSON-LD: real-world corpus with a graph oracle

**What to build:** public, licensed JSON-LD of the kinds people edit, fetched by the existing corpus script into the gitignored corpus folder: schema.org examples, ActivityStreams, Verifiable Credentials and DIDs, IIIF manifests, DCAT, Solid profiles and GS1. A test compares each file's graph with oxjsonld's (a dev-dependency used only as an oracle) by isomorphism, reporting per file. A committed source list records URLs and licences. A generator makes large synthetic files for the performance budgets.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** owns the corpus script additions, the source list and a new oracle test.

- [ ] At least 30 files across at least 6 sources, all licences recorded
- [ ] The oracle test skips cleanly when the corpus is not downloaded, and reports per-file results when it is
- [ ] The synthetic generator produces a file of about 30,000 lines

Spec: docs/specs/jsonld.md (Testing Decisions: "Corpus").
