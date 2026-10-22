# 15: Turtle naming module

**What to fix:** the rules for writing an IRI in Turtle (which prefixes are in scope at
an offset, whether a local part is a valid PN_LOCAL, whether a name is a PN_PREFIX,
where a declaration goes and in which style, the line ending, token spacing) are spread
over six files with different answers: two `shorten` (`completion.rs:697-719` accepts
`-a`, digits-like starts and any prefix name, ties by hash order; `refactor.rs:198-215`
is correct), three local-name checks, two `namespace_of` that differ (`knowledge.rs:156`
vs `refactor.rs:379`), three `const OWL`, two truncate helpers, PN_CHARS tables copied
between the two lexers. Consequences, all reproduced:

- Renaming `ex:a` to `<http://example.org/-a>` wrote `ex:-a` (invalid) into four files;
  hover, outline, type hierarchy, query results and SPARQL rename use the same helper.
- Rename never validates the new name (`foo bar` → `<…/foo bar>` everywhere; prefix
  rename accepts `1x`, `a.`).
- Prefixes are treated as bound for the whole document: the undeclared-prefix fix
  inserts after the *last* declaration even below the use (and then reports a
  duplicate); completion adds no `@prefix` when the prefix is declared later; definition
  and hover disagree on a redefined prefix (`diagnostics.rs:672-705`, `completion.rs:115`,
  `navigation.rs:86`, `hover.rs:41`). `refactor::scope_at` already has the answer.
- Inserted prefix lines and `owl:imports` always end in `\n`; `organise_block` produces
  `\r\r\n` on CRLF files (`diagnostics.rs:657-669`, `refactor.rs:598-631`).
- "Add all missing prefixes" ignores `has_prefixes()` and writes `@prefix` into
  N-Triples (`diagnostics.rs:542-562`); `uri.ends_with("prefixes.ttl")` matches
  `myprefixes.ttl` (`:290`, `refactor.rs:519`); duplicate-prefix fix titled "Remove
  unused prefix".
- `document_symbols` is O(statements × graphs) for a JSON-LD-only check (20k-graph TriG:
  183 ms, `symbols.rs:80-84`); trailing comments fold as a comment run (`:360-375`).

**Files owned:** new `src/syntax/names.rs` (PN_CHARS tables, `is_pn_prefix`,
`is_pn_local`, both namespace rules under names that say which is which, one `shorten`
with deterministic ties, `Scope::at(a, offset)`, `Scope::write(iri)`,
`declaration_edit(a, enc, name, ns, before)`, `newline(text)`), `src/syntax/lexer.rs` and
`src/sparql/lexer.rs` (use the shared tables), `src/features/{completion,refactor,hover,
navigation,hierarchy,symbols,diagnostics,inlay,literals}.rs`, `src/features/completion/
jsonld.rs` (call sites), `src/sparql/features/{rename,actions}.rs`, `src/query/render.rs`,
`test/refactor.test.mjs`, `test/prefix-import.test.mjs`, `test/prefix-rename.test.mjs`,
`test/nquads.test.mjs`, `test/trig.test.mjs`, `test/display.test.mjs`.
**Hands off:** `src/knowledge.rs` (`namespace_of` moved by ticket 07), `src/syntax/mod.rs`
(ticket 16 adds `pub mod names;`), `src/features/lint.rs` (ticket 07),
`src/features/format.rs`.

**Blocked by:** ticket 16 (the `names` stub), ticket 07 (`knowledge::namespace_of`).

**Status:** ready-for-agent

- [ ] First commit behaviour-preserving: move the helpers, keep two named `shorten`
      variants, every caller compiles. Second commit: one checked `shorten`; rename
      validates with `oxiri::Iri::parse` and `is_pn_prefix` and returns `InvalidParams`.
- [ ] Scope-aware prefix handling: the fix inserts after the last declaration *before*
      the first use (else the top), completion adds `@prefix` when the prefix is not in
      scope at the cursor, definition and hover agree on the declaration in scope.
- [ ] `newline(text)` used by every inserted line; `organise_block` normalises `\r\n`
      before re-joining.
- [ ] "Add all" returns early when `!has_prefixes()`; `file_uri_name(uri) ==
      Some(PREFIX_FILE)` in both places; duplicate fix titled "Remove duplicate prefix".
- [ ] `document_symbols` builds a graph-span set once and only for JSON-LD; comment
      folds count own-line comments only.
- [ ] Tests: rename to `<…/-a>` and to `foo bar` refused; fix for a use before a
      mid-file declaration; CRLF fix text; `.nt` with two undeclared prefixes gets no
      `@prefix`; `myprefixes.ttl` gets `unused-prefix`; 20,000-graph TriG outline under
      50 ms; trailing comments do not fold. `refactor.rs` unit tests
      (`graph_is_kept_by_every_action`), `hierarchy.test.mjs`, `lsp.test.mjs`,
      `sparql-rename.test.mjs`, `src/query/tests.rs` green.
