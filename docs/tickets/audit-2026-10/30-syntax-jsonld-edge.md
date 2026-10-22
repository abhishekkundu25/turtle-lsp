# 30: The syntax core no longer calls JSON-LD

**What to build:** the Turtle-family parser depends upward on the JSON-LD frontend.
`syntax::parse_with_base` sends `Syntax::JsonLd` to `crate::jsonld::parse` with
`NoNetwork` (`src/syntax/parser.rs:80-92`), and the module docs say so
(`src/syntax/mod.rs:4`, `:23`). That edge makes the planned syntax package of
`docs/specs/workspace-split.md` depend on JSON-LD (audit X4-02 (1)). No production
caller needs it. `Analysis` routes JSON-LD to `jsonld::parse_document_with` itself
(`src/analysis.rs:162-174`), and `features::format::format` returns early for JSON-LD
(`src/features/format.rs:79-85`), so its verification re-parse (`:107`) is never
JSON-LD. The callers that can pass JSON-LD are a unit test that pins the edge
(`the_turtle_entry_point_hands_jsonld_to_this_frontend`, `src/jsonld/mod.rs:595-610`)
and the chaos test: `exercise` parses with the document's syntax (`tests/chaos.rs:430`),
which `mutated_jsonld_never_panics` (`:867`) makes JSON-LD, and
`code_actions_keep_the_graph` re-parses with the analysis's syntax (`:685`, `:699`).
`examples/corpus.rs` calls `parse_with_base` too (`:413`, `:466`, `:481`) but reads no
JSON-LD (`:1266`), so it needs no change. Drop the branch. Given `Syntax::JsonLd`,
`parse_with_base` returns a `Parsed` with no triples and one error (code `syntax`,
message "JSON-LD documents are parsed by jsonld::parse"). It keeps its "never fails"
contract, and the doc comment says the function is for the Turtle family. The chaos
test routes JSON-LD to `crate::jsonld::parse` through a local helper, so its JSON-LD
coverage stays; no public dispatcher is added (owner decision 5). **No behaviour
change** for the server, the CLI and the browser.

**Files owned:** `src/syntax/parser.rs` (`parse_with_base` and its doc comment),
`src/syntax/mod.rs` (the module doc lines 1-25 only), `src/jsonld/mod.rs` (that one
test, rewritten to pin the new contract), `tests/chaos.rs` (the three call sites and a
helper).
**Hands off:** the rest of `src/syntax/mod.rs` (31 and 34a edit it later), the lexer and
the rest of the parser, `src/analysis.rs`, `src/features/**`.

**Blocked by:** nothing in wave 3.

**Status:** done

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] `rg -n "jsonld" src/syntax/parser.rs src/syntax/lexer.rs src/syntax/iri.rs`
      finds no code reference, and `src/syntax/` has no `use crate::jsonld`.
- [ ] Unit tests: `parse_with_base` given `Syntax::JsonLd` gives the one error and no
      triples; `jsonld::parse` on the old test's input still gives only the unclosed `{`.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib`,
      `--test w3c_conformance`, `--test oxttl_oracle`, `--test format`,
      `--test w3c_jsonld`, `--test jsonld_oracle`, `--test jsonld_lints`, `--test chaos`;
      `cargo build --release --locked --examples`; `test/jsonld.test.mjs`,
      `test/jsonld-syntax.test.mjs`, `test/format.test.mjs`; `cargo fmt --check`,
      clippy.
