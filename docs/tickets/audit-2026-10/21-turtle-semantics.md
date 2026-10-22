# 21: Turtle semantics

**What to fix:** wrong IRIs and terms reach query, SHACL and navigation. Reproduced.

- Turtle, TriG, N-Triples and N-Quads are parsed with no base IRI (`src/analysis.rs:137`
  calls `syntax::parse`); the JSON-LD frontend resolves against the document URI. So
  `<#me>` in two files is one IRI across the workspace (definitions, references, rename,
  query and SHACL merge them); a valid relative `@base <rel/> .` is reported as an error;
  query results show relative IRIs. `format.rs` already passes `document_base`.
- The base direction is stored inside the language handle (`"en--rtl"`,
  `src/syntax/mod.rs:175-179`, `parser.rs:1050-1056`) and `rdf/dataset.rs:162` passes it
  to spareval as the tag: `LANG()` returns `"en--rtl"`, `LANGDIR()` is empty, and a
  constant `"hi"@en--rtl` never matches. Four places split `--` by hand.
- A blank node inside a triple term is named `_:t{n}` while the same node outside is
  `_:g{base+n}` (`dataset.rs:155` vs `:53`): joins between them fail; the same label in
  two documents collides.
- Error recovery inside a TriG block rescans the line per skipped token
  (`parser.rs:321-342, 460`): 20k tokens 0.15 s, 40k 0.60 s, 80k 2.51 s.
- A prefixed name with an undeclared prefix becomes the IRI `nope:p` and reaches the
  query/SHACL graph (`parser.rs:1356`); `<<( << … >> … )>>` is accepted (grammar: no
  `reifiedTriple` inside a triple term; confirm against the spec first);
  `from_extension` is case-sensitive; `position_in` can panic on a mid-character offset;
  `prefer` picks the last English label and matches `enm`.

**Files owned:** `src/syntax/mod.rs` (`dir: Option<Direction>` on `Node::Literal`;
`from_extension` case-insensitive), `src/syntax/parser.rs` (base; O(1) line start in
recovery; `tt_subject`/`tt_object`; unresolved-prefix marking), `src/rdf/dataset.rs`,
`src/rdf/graph.rs` (blank base for triple terms), `src/analysis.rs`
(`parse_with_base(text, external, syntax, Some(origin.unwrap_or(uri)))`; `position_in`
char snap; `prefer`), `src/jsonld/*` call sites that read `lang` (with care: ticket 18
owns the bodies; coordinate), `src/features/*` and `src/shacl/*` call sites that read
`lang`, `tests/oxttl_oracle.rs`, `tests/w3c_conformance.rs` (direction handling
simplified), `test/project.test.mjs`, `test/lsp.test.mjs` (new base cases).
**Hands off:** `src/syntax/lexer.rs` beyond `lang` (ticket 01 landed), `src/knowledge.rs`.

**Blocked by:** ticket 05 (`analysis.rs` one-line change), ticket 16 (`syntax/mod.rs`
lock). Owner decision assumed: Turtle parses against the document URI.

**Status:** ready-for-agent

- [ ] `Node::Literal` carries `dir`; `lang` is the bare tag; `node_term`/`internal`/
      `Index::new` use `new_directional_language_tagged_literal_unchecked`; the oracle
      and W3C harness stop splitting `--`. Dataset test: `LANG`, `LANGDIR`, a directional
      constant matches.
- [ ] Blank nodes inside triple terms use the graph's numbering; `internal` maps them
      back. Test: the `<<( _:x … )>>` join returns `?v = 2`; two documents do not collide.
- [ ] `Analysis::with_loader` parses against the document URI (or `origin` for cached
      imports); the import cache's `@base` line reviewed. Tests: `<#me>` in two files are
      two IRIs; `@base <rel/>` gives no error; navigation and rename see absolute IRIs
      (`project.test.mjs`, `refactor.test.mjs`).
- [ ] Recovery tests `is_starter` first and finds line starts in O(1). Test: a
      200k-token TriG line with an error parses under 0.5 s.
- [ ] Triples with unresolved prefixes flagged (or kept out of `Node::Iri`) so
      `Graph::new` callers can filter; `tt_subject`/`tt_object` per grammar (after
      confirming); `eq_ignore_ascii_case` extensions; `position_in` snaps to a char
      boundary; `prefer` prefers the first `en`/`en-*` label.
- [ ] W3C RDF 1137/1137, SPARQL, SHACL, JSON-LD suites unchanged; `tests/chaos.rs`,
      `tests/format.rs`, `npm run test:node` green.
