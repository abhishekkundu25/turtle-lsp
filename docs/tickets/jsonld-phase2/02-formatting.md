# 02: Formatting JSON-LD

**What to build:** `textDocument/formatting`, `turtle-lsp format` (`--check`, `--diff`) and the browser engine format JSON-LD: JSON.stringify-style layout with the editor's indent, scalars kept as written, key order kept, nothing done on a document with syntax errors, result checked to parse to the same JSON value, guarantee "same graph".

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**Parallel slices:** new src/jsonld/format.rs; one `pub mod` line in src/jsonld/mod.rs (trivial conflict with 01); the JSON-LD branch in src/features/format.rs and src/batch.rs.

- [ ] Promote: "formatting JSON-LD keeps its JSON value…", "format: JSON-LD files are formatted in place…", and the browser engine test
- [ ] Added: formatting refused on a syntax error; tabs honoured; idempotent on the jsonld corpus (Rust test over test/corpus/jsonld, skipped when absent)
