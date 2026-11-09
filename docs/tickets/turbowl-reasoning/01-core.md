# 01: The reasoning core and the setting

**What to build:** with `turtle.reasoning` set to `default` or `complete`, turtle-lsp can build the workspace's base (documents and loaded imports in `(Source, uri)` order, as oxrdf triples through `rdf::Graph`), reason over it with TurbOWL, and map any base triple back to its document, range and line. The setting and its size limit exist in Rust, VS Code and the browser settings. Nothing is scheduled yet (02) and nothing is shown yet (03); `off` changes nothing.

Build the `reasoning` module's types for every later ticket: the base with its origin map; the shared state (reasoner, origin map, generation, explain cache) behind a read-write lock that only the worker writes; the worker (one thread, newest job wins, generation, panics caught, as `features::shacl_validate::Worker`) and `run_inline`; and a read API for hover: "the proofs of these inferred types of this term, within a deadline", with the cache passed in and out. Thread the shared state into `ide::Session` as one field.

**Blocked by:** 00.

**Status:** done (ef20c9b)

- [ ] `turbowl` path dependency (`default-features = false`, `features = ["rdf-12"]`); `cargo build --release --locked` passes.
- [ ] Settings `turtle.reasoning` (`off`/`default`/`complete`) and `turtle.reasoning.maxTriples`, parsed and clamped in `settings.rs` with unit tests; in `package.json` (with `complete` described as slower) and `restrictedConfigurations` as SHACL's cap is; `manifest.test.mjs` passes; `web/app/settings.js` lists them.
- [ ] Unit tests: the base is identical for the same documents in any order; a base triple maps back to its line; reasoning a subclass example gives the inferred type; a run over the size limit reports skipped.
- [ ] `off`: no thread, no state built.
