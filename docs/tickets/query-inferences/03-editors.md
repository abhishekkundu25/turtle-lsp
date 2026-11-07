# 03: Editors: `turtle/query` and "▶ Run query"

**What to build:** `turtle/query` takes a `reasoning` parameter
(`off`/`default`/`complete`, default the `query.reasoning` setting; any other
value is `InvalidParams`) and answers an `inferred` member when the note is
added (`{"rules", "triples", "findings"}`) or skipped (`{"skipped":
{"triples", "limit"}}`), absent when off. "▶ Run query" reads
`query.reasoning`, and its results document gains the note's line under its
heading. Both pass the workspace's imports to the core, as background
reasoning reads them, and `turtle.reasoning.maxTriples` as the limit. The
module docs of both files say so. VS Code: the `turtle.query.reasoning`
setting in the manifest (enum with descriptions, default `off`), the client
forwarding it as the other `query.*` settings are, and the
`turtle-lsp.json` schema kept in step with the server's keys (its test).

**Blocked by:** 01.

**Status:** open

- [ ] `test/query-request.test.mjs`: the `reasoning` parameter returns an inferred row and the `inferred` member; off answers exactly as before with no member; the skipped form over a low `turtle.reasoning.maxTriples`; the default follows `query.reasoning`; a bad value is `InvalidParams`; an imported axiom infers.
- [ ] `test/query-lsp.test.mjs`: with `query.reasoning` on, "▶ Run query"'s document includes the inferred row and the note; off, the document is unchanged.
- [ ] The VS Code manifest and schema tests pass with the new key.
- [ ] `cargo fmt --check`, clippy and the lib unit tests pass.
