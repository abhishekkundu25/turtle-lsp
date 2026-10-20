# 02: JSON-LD: W3C 1.1 test suite runner (toRdf, expand, remote-doc)

**What to build:** the W3C JSON-LD 1.1 toRdf, expand and remote-doc manifests vendored next to the existing W3C suites, run by `cargo test` through the engine interface. An expected-status manifest records each test as pass, known-fail (with a reason) or out-of-scope. The run fails when a pass regresses or a known-fail starts passing. Remote-doc runs offline through an injected loader. At first every in-scope test is known-fail.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** owns a new integration test and the vendored suite folder only.

- [ ] The suite runs offline in under 2 minutes and prints pass/known-fail/out-of-scope counts
- [ ] Changing one expected status in either direction fails the run with a clear message
- [ ] Compaction, flattening, framing and fromRdf manifests are listed as out of scope

Spec: docs/specs/jsonld.md (Testing Decisions: "W3C suites").
