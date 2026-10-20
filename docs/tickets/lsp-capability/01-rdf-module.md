# 01: Shared `rdf` module (graph and SPARQL dataset)

**What to build:** move the validation graph and the `spareval` dataset adapter out of SHACL into a new top-level `rdf` module (`rdf::graph`, `rdf::dataset`), so other features can query workspace data. SHACL keeps pre-binding, query checks, `$PATH` and its batching engine; the `$shapesGraph` named graph becomes an optional extra named graph the adapter is given. Pure move: no logic or speed change.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] `src/rdf/graph.rs` and `src/rdf/dataset.rs` exist; `src/shacl/graph.rs` is gone (moved with git so history follows); SHACL and the features import from `rdf`.
- [ ] The adapter no longer names SHACL; SHACL passes its shapes-graph IRI in.
- [ ] Rust, Node and web suites pass with no test changed except `use` lines; W3C SHACL and SHACL-SPARQL suites unchanged.
- [ ] `cargo bench`-free check: `bench/perf.mjs` SHACL rows unchanged within noise.
