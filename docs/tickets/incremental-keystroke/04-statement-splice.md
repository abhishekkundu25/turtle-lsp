# 04: Statement splice: the parser's start state and the splice module

**What to build:** the core of Part 3, on `Parsed` alone. No wiring into
the analysis or workspace (that is 05).

- The parser (`src/syntax/parser.rs`) gains one way to start in a given
  state: prefixes and base in effect, graph block state, blank nodes
  allocated so far, statement index, at a token position. Full parses go
  through the same code; there is still one parser.
- A new module `src/syntax/splice.rs` (one `pub(crate) mod splice;` line in
  `src/syntax/mod.rs`, nothing else there). Its entry point takes the old
  `Parsed`, the old and new text, the external prefixes and base, and one
  edit (byte range in the old text, new length), and returns either the new
  `Parsed` or a fallback reason. The reason is an enum (directive, blank
  label, TriG brace or `GRAPH`, deep nesting, unsupported syntax, no
  statements, window too large, ...), so 05 can count paths.
- Rules from the spec (Implementation Decisions, Part 3): re-lex from the
  token before the one holding the edit to the first new token at or past
  the edit's end that lines up with an old token start; re-parse from one
  statement before the token window until a statement ends past the
  meeting point and the next token starts an old statement with the same
  parser state; splice tokens, statements, triples, refs, prefix uses,
  graphs, and the three error groups separately; shift later spans,
  statement indexes and fresh blank node ids. Every fallback in the spec's
  list, checked on the old and new window. N-Triples and N-Quads follow the
  same rules (a statement is a line). SPARQL and JSON-LD always fall back.
- Keep the fallbacks strict. A fallback costs only speed.

**Blocked by:** none (compare with `format!("{:?}")` until 01's
`PartialEq` lands; switch to `==` after).

**Status:** open

- [ ] Unit tests in `splice.rs`, each a small text and one edit, the result
  equal to a full parse of the new text (or the expected fallback): a
  literal edit; a comment edit; a directive edit (falls back); an opened
  and a closed `"""`; `1.` then `5`; a prefixed name losing its trailing
  dot; `^^`; `{|`; a broken statement swallowing the next; `[ ]` added
  before later blank nodes; a `_:b` (falls back); TriG braces (fall back)
  and an edit inside a graph block; an N-Quads line edit; an edit at the
  very start and the very end; deleting everything.
- [ ] A property test in the same module: random edits on the chaos
  fixtures, splice result equals the full parse whenever it does not fall
  back, and it does not fall back on most single-character edits inside
  literals.
- [ ] The existing parser tests pass.
