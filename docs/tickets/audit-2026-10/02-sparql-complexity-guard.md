# 02: SPARQL complexity guard

**What to fix:** `too_complex` (`src/sparql/diagnostics.rs:171-218`) counts bracket
depth and `OPTIONAL`/`MINUS` only. `spargebra` parsing and `spareval` evaluation recurse
once per element of a chain, so a long `||`/`&&`/comparison chain, a long property path
(`/`, `|`), or a run of unary `!` overflows the stack and aborts the process. Reproduced:
a `.rq` with a 20,000-step path aborts `check`; a 10,000-term `||` aborts `query`;
opening such a `.rq` aborts the server's main thread; a `.ttl` whose `sh:select` holds
the path aborts the server's `shacl` thread during indexing with no file open. The guard
is shared by the editor, the SHACL lint (`src/features/shacl_lint/sparql.rs:117`) and
SHACL validation (`src/shacl/sparql.rs:78`), so one fix covers all three. Also:
uppercase `TRUE`/`FALSE` are lexer `Error` tokens with a misleading message
(`src/sparql/lexer.rs:577-579`); SPARQL keywords are case-insensitive.

**Files owned:** `src/sparql/diagnostics.rs`, `src/sparql/lexer.rs`,
`src/sparql/tests.rs`.
**Hands off:** `src/query/mod.rs` (ticket 04), `src/sparql/structure.rs`,
`src/sparql/format.rs`, `src/shacl/**` (ticket 06).

**Blocked by:** nothing.

**Status:** ready-for-agent

- [ ] `too_complex` also counts: operands between binary operators in one expression,
      path elements between `/` and `|`, and runs of unary operators. Pick limits by
      measuring the abort point on an 8 MiB stack and dividing by four; record the
      measurement in a comment.
- [ ] Unit tests: 20,000 `||` terms, 20,000 path steps and 20,000 `!` each produce one
      diagnostic; the limits minus one parse cleanly.
- [ ] A `.ttl` whose `sh:select` holds the 20,000-step path passes `check` with a
      diagnostic and exit 1, no abort (e2e in `test/shacl-sparql-lint.test.mjs` or a new
      file).
- [ ] `true`/`false` lexed case-insensitively as `Boolean` and normalised for
      `spargebra` in `token_for_spargebra`, as dots are; or a direct message. Unit test.
- [ ] `tests/w3c_sparql.rs` 425/426 unchanged; `test/sparql.test.mjs`,
      `test/shacl-sparql-lint.test.mjs` green.
