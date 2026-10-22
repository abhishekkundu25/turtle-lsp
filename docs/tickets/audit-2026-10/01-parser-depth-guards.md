# 01: Turtle parser depth guards

**What to fix:** annotation blocks `{| ... |}` nest without limit. `annotation` →
`predicate_object_list` → `object_list` → `annotation` never passes through `nested()`,
so `MAX_NESTING` (256) covers only `[ ]`, `( )`, `<< >>` and `<<( )>>`
(`src/syntax/parser.rs:1270-1311`, guard at `:1097-1109`). A stack overflow cannot be
caught: 30,000 levels (a 270 KB file) abort `query`, `check` of *any* file in the same
root, and the LSP server during start-up indexing (SIGABRT, reproduced). Two small lexer
defects in the same files: a backslash before a newline in a short string skips the
newline and swallows the next statement (`lexer.rs:174-189`); a lone `\r` does not end a
comment, so old-Mac line endings lose the whole file after the first comment
(`lexer.rs:130-133`, `parser.rs:322`).

**Files owned:** `src/syntax/parser.rs`, `src/syntax/lexer.rs`.
**Hands off:** `src/syntax/mod.rs` (ticket 16/21), `tests/chaos.rs` (ticket 11),
`src/analysis.rs`.

**Blocked by:** nothing. Land before ticket 11 (its chaos generators assume this fix).

**Status:** ready-for-agent

- [ ] One depth guard at the recursive entry points: make `nested` generic over its
      return type and wrap the `LAnnotation` arm (or `predicate_object_list` as a whole).
      Update the `MAX_NESTING` doc comment to list `{|`.
- [ ] Unit test: 100,000 `{|` levels, run on a thread with a 2 MiB stack, gives exactly
      one "Nested more than 256 levels deep" error and no abort. Same for 100,000 `[`,
      `(`, `<<` and `<<(` as controls.
- [ ] `string()`: when not a long string and the byte after `\` is `\n` or `\r`, end the
      token there with "Unterminated string". Unit test: the statement after it survives.
- [ ] Comments end at `\r` as well as `\n`; `at_line_start` accepts `\r`. Unit test: a
      CR-only file with a comment keeps its triples.
- [ ] `cargo test --release` (parser, lexer, `tests/w3c_conformance.rs` 1137/1137,
      `tests/oxttl_oracle.rs`, `tests/format.rs`) green; `test/lsp.test.mjs` green.
