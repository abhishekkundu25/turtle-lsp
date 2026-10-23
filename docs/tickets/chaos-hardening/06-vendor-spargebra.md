# 06: Vendor `spargebra` unchanged, exact pin, golden of parsed queries (class E, step 1)

**What to build:** Vendor `spargebra` 0.4.7 as `vendor/spargebra` with its licence file, byte-identical to the registry crate: a `[patch.crates-io]` entry in Cargo.toml AND in web/engine/Cargo.toml (the browser build has its own patch table and lock file; a patch in a dependency's manifest does not apply), both Cargo.lock files updated, and the root requirement changed from `"0.4"` to `"=0.4.7"` (Cargo.toml:43; the patch applies only while the registry version satisfies the requirement, and `"0.4"` is not a pin; vendor/spareval/Cargo.toml already requires `=0.4.7`). A `vendor/spargebra/PATCH.md` (empty patch list for now, with the review rule from ticket 07 to be filled in). tools/check-vendor.sh becomes a loop over a table of crates (name, version, sha, patch files, strip depth) instead of a copy of the spareval block. AGENTS.md Release step 3 names both crates. A golden test `tests/sparql_golden.rs` that parses every W3C SPARQL query that currently parses and every `test/corpus` query, writes `Query::to_string()` for each, and compares against a committed data file; the file is generated in this ticket on the unchanged grammar.

**Blocked by:** nothing.

**Issue:** #18. **Wave:** 1. **Files (lock in bold):** new vendor/spargebra/; **Cargo.toml**, **Cargo.lock**, **web/engine/Cargo.toml**, **web/engine/Cargo.lock**; tools/check-vendor.sh; AGENTS.md (Release step 3 only); new tests/sparql_golden.rs and its data file

**Status:** proposed

- [ ] `cargo build --release --locked` and `node web/build.mjs` both build from the vendored crate (`cargo tree -i spargebra` in both manifests shows the path source).
- [ ] `tools/check-vendor.sh` passes for spareval and spargebra; a deliberate one-byte change in vendor/spargebra fails it.
- [ ] `tests/sparql_golden.rs` passes on the unchanged grammar and its data file is committed; the test runs under `timeout 120`.
- [ ] `tests/w3c_sparql.rs` at 425/426; `cargo test --release --locked --doc` green; `RELEASE_TAG` version test unchanged.
- [ ] `bench/perf.mjs` SPARQL rows (if any) within the spec's noise definition, measured by ticket 13.
