# 34a: Token memory: a 12-byte token, one token vector, borrowed messages, an error cap

**What to build:** memory per input byte is high, and the token stream is a large part
of it (audit X5-04, A1 R6). `syntax::lexer::Token` is 32 bytes, because it carries an
`error: Option<&'static str>` beside `kind`, `start` and `end`
(`src/syntax/lexer.rs:47-54`). The parser holds the tokens twice: `all`, then a
comment-free copy `toks` (`src/syntax/parser.rs:94-126`). Every `SyntaxError.message`
is an owned `String` (`src/syntax/mod.rs:291-298`), even when it is one of the
lexer's static texts (`parser.rs:112-120`). And a broken file can carry an error per
token. Expand, then contract, so every step stays green:
1. **Expand.** Add `Token::error() -> Option<&'static str>` and
   `Token::new(kind, start, end)`, `SyntaxError::new(span, message, severity, code)`
   and `SyntaxError::message() -> &str`. Move every reader and constructor to them:
   the parser (`:97-101`, `:112`, `:651` and the rest), the lexer tests, `tests/chaos.rs:1813`,
   the `SyntaxError` constructions in `src/jsonld/{json,context,expand}.rs` and
   `src/workspace.rs` (`failed_analysis`, `:421-435`), and the message readers the
   compiler lists, such as `features::diagnostics` at `:166`.
2. **Contract.** `#[repr(u8)] Kind`; the error becomes a private `u8` code into a static
   message table in the lexer (0 = none); `SyntaxError.message` becomes
   `Cow<'static, str>`. A compile-time assertion pins `size_of::<Token>() == 12`.
3. **One vector.** The parser walks `all`, skipping comments in `peek` and `bump`, or
   through a `u32` index of the non-comment tokens: no second `Vec<Token>`.
4. **Cap.** Turtle-family syntax errors are capped at 100 per document, as JSON's are
   (`jsonld::json::MAX_ERRORS`, `src/jsonld/json.rs:188`, `:530-545`). One final error
   says "More than 100 syntax errors; the rest are not shown". Parsing and triples go
   on past the cap (owner decision 9).

Steps 1 to 3 change no output. Step 4 is the one behaviour change, and it has tests.

**Files owned:** `src/syntax/lexer.rs`, `src/syntax/parser.rs`, `src/syntax/mod.rs`
(`SyntaxError` and the `Token` re-export only), the `SyntaxError` constructions in
`src/jsonld/json.rs`, `src/jsonld/context.rs` and `src/jsonld/expand.rs`,
`src/workspace.rs` (`failed_analysis` only), the message readers under
`src/features/**`, `src/batch.rs` and `src/cli/**` (only the lines the type change
breaks; not `src/cli/files.rs`, which 31 holds in this wave), `tests/chaos.rs` (the
token and error call sites), `examples/corpus.rs` (the same).
**Hands off:** `src/analysis.rs` and `Analysis`'s token handling (34b),
`src/features/format.rs` beyond a type fix (34b), the rest of `src/workspace.rs` (31
holds it in this wave), `src/syntax/mod.rs` above `SyntaxError` (the module doc is
30's), `src/knowledge.rs` (33c), `src/server.rs`.

**Blocked by:** 26b (`src/features/format.rs`) and 30 (`src/syntax/parser.rs`,
`tests/chaos.rs`). Runs in the same wave as 28b, 31 and 33c. It shares
`src/workspace.rs` with 31 in separate hunks: 34a edits only `failed_analysis`
(`:421-435`), and 31's nearest edit is `is_turtle` (`:456`).

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks: the memory budget is measured in 34b. No
new dependencies. Leave the changes uncommitted.

- [ ] `const _: () = assert!(std::mem::size_of::<Token>() == 12);` in the lexer;
      `rg -n "Vec<Token>" src/syntax/parser.rs` finds no second vector.
- [ ] Every lexer error text is in the static table, and a unit test round-trips each
      code to its text. `SyntaxError.message` is a `Cow` and borrowed for every static
      text (a test checks `Cow::Borrowed` for an unterminated string).
- [ ] Cap tests: a document of 1,000 lines with one syntax error each gives 101 errors
      (100, then the cap error), and still gives the triples of its valid statements;
      `tests/chaos.rs` `pathological_huge_tokens` still holds.
- [ ] Green, unchanged output apart from the cap: `--test oxttl_oracle`,
      `--test w3c_conformance`, `--test chaos`, `--test format`, `--test w3c_jsonld`,
      `--test jsonld_oracle`, `--test jsonld_lints`, `cargo test --release --locked --lib`;
      `test/lsp.test.mjs`, `test/display.test.mjs` (semantic tokens),
      `test/jsonld-syntax.test.mjs`, `test/cli.test.mjs`;
      `cargo build --release --locked --examples`; `cargo fmt --check`, clippy.
