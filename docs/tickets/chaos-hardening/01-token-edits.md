# 01: `token_edits` with re-lex; shorten, extract, expand (class D core)

**What to build:** `pub(crate) fn token_edits(text: &str, reps: Vec<(Span, String)>, lex: impl Fn(&str) -> Vec<Token>) -> Vec<(Span, String)>` in src/syntax/names.rs, replacing `spaced` and the adjacent-IRI loop from PR #14 in `shorten_all_edits`. Callers pass whole tokens; an insertion is an empty span and a deletion an empty text. Sort by start and build the final text. For each junction (both ends of every replacement), re-lex a window from the previous kept token to the next with the caller's lexer and compare the (kind, text) sequence against the expected one (the old tokens with the swapped tokens substituted). Where they differ, insert a space at that junction (or the document's line break when the replacement is a directive, see ticket 02) and re-check. Then trim the common prefix and suffix of old and new by `char`. The lexer is a parameter because `syntax` must not import `sparql`: `features` passes `syntax::lexer::tokenize`, `sparql::features` passes `sparql::lexer::tokenize`. Route Turtle Shorten, Shorten all, Extract and Expand (src/features/refactor.rs) and SPARQL Shorten, Shorten all and Expand (src/sparql/features/actions.rs) through it. The doc comment states the invariant: the token stream after equals the stream before with exactly the given tokens changed.

**Blocked by:** nothing.

**Issue:** #16. **Wave:** 1. **Files (lock in bold):** **src/syntax/names.rs** (`spaced` replaced by `token_edits`); src/features/refactor.rs; src/sparql/features/actions.rs (shorten and expand hunks)

**Status:** proposed

- [ ] names.rs table test over `token_edits` with both lexers, comparing token sequences: `ex:a.b.<http://s/V>` to `ex:a.b. sh:V`; `_:b.<http://e/x>`; `1.<http://g/x>` to `1. e1:x`; `a<http://e/C>`; `<http://e/a><http://e/b>` to `ex:a ex:b`; `<http://e/x>.b`; `<http://e/x>:y`; `<http://e/x>-1`; `@prefix:<http://e/>` with `:` to `ex`; an empty replacement between two names; a replacement of `café` by `cafè` (trim stays on a char boundary); SPARQL `?a<=<http://e/b>&&?c>?a` with the IRI shortened.
- [ ] refactor.rs: `graph_is_kept_by_every_action` (or a new test) over `<s> a ex:C ; ex:p ex:a.b.<http://e/V> a <http://e/C> .` for Shorten all, Shorten IRI and Extract, and Extract over `<http://n.org/a><http://n.org/b>`; the PR #14 test still passes.
- [ ] SPARQL actions tests: Shorten all over `{ <http://e/s><http://e/p><http://e/o> }`, Shorten over `?s ex:p?<http://e/o>`, Expand over `FILTER(?a<ex:b)`; each reparses with no error and the same token sequence (with ticket 04's lexer where `<` follows an operand).
- [ ] `spaced` and the PR #14 loop are gone; a grep for `TextEdit::new` and `new_text` in src/features and src/sparql/features lists every remaining writer and which ticket routes it.
- [ ] Replay of 149959 passes with seed 1792815672.
