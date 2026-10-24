# 03: Renames, "Change to", removal and fix composition (class D rest)

**What to build:** Route the remaining token writers through `token_edits` (ticket 01): Turtle prefix rename and term rename (src/features/navigation.rs, whole-token spans for prefix renames, one call per document); SPARQL prefix, term and variable rename (src/sparql/features/rename.rs); the "Change to" quick fix (src/features/quickfix.rs and src/sparql/features/actions.rs resolve `d.range` to a span and call it with one replacement); `quickfix::removal` ("Remove unused prefix", "Remove duplicate prefix", both syntaxes) as a deletion, so `ex:o.@prefix u: <..> .ex:t` keeps `ex:o.` and `ex:t` apart. Term rename writes the name with the scope at the use, not `Scope::whole`, so a prefix declared after the use or redeclared with another IRI cannot yield a different IRI. In src/batch.rs `fix_all_in`, a round applies only edits that do not touch; an edit that touches one already taken is deferred to the next round and recomputed against the new text. The web engine's `fixAll` (web/engine/src/api.rs) already calls `batch::fix_all`; confirm and add a test.

**Blocked by:** #16.

**Issue:** #24. **Wave:** 2. **Files (lock in bold):** src/features/navigation.rs; src/sparql/features/rename.rs; src/features/quickfix.rs (`removal`, "Change to"); src/sparql/features/actions.rs (quick-fix and removal hunks); src/batch.rs

**Status:** proposed

- [ ] navigation.rs tests: prefix rename `:` to `ex` in `@prefix:<http://e/> . :s :p :o .`; `ex` to `e5` in `ex:s ex:p (1ex:b) .`; term rename `http://e/` to `http://e/a` in `ex:s ex:p ( ex:-1 ) .`; a term rename where the prefix is declared after the use; each keeps every triple but the renamed term (expected graph = old graph with the IRI replaced).
- [ ] sparql rename.rs tests: `PREFIX:<http://e/>` renamed to `ex`; `FILTER(?a<ex:b)` term rename to an IRI with no prefix keeps `?a <` and `<..>` apart.
- [ ] quickfix tests: "Change to" on `FILTER(?a<ex:typo)` with a full-IRI suggestion; removal on `ex:s ex:p ex:o.@prefix u: <http://u/> .ex:t ex:p ex:o .` leaves two statements and no error, in Turtle and in SPARQL (`PREFIX`).
- [ ] batch.rs test: two safe fixes whose edits touch (a removal next to a "Change to") applied by `fix_all` leave a file with no new error; the second lands in round two.
- [ ] web/test/engine.test.mjs: `fixAll` on the same input gives the same text as `check --fix`.
- [ ] A grep for `TextEdit::new` and `new_text` in src/ shows every writer going through `token_edits` or listed with a reason (whole-line organise edits are out of scope).
