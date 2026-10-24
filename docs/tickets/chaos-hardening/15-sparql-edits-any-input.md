# 15: SPARQL edits keep the token stream on every input (found by ticket 12)

**What to build:** Ticket 12's token checks fail 58 to 81 times per SPARQL seed at 30,000 iterations, in three shapes. (F1) An IRI written right after an operand inside an expression's parentheses lexes as the `<` operator (ticket 04's rule), so Expand, the add-PREFIX quick fix, completion auto-import and rename change the token stream; every sampled input already had a syntax error. No space can separate the tokens there, so `token_edits` gives up and the edit goes out anyway. The rule to enforce: an edit whose result cannot lex to the expected tokens is not made. `token_edits` reports that case, and each caller drops the code action, the completion's additional edit or the rename (with the rename's usual "cannot rename here" error), instead of writing it. If the lexer's operator-or-IRI choice is itself wrong for a valid query, fix the lexer instead and say why. (F2) Shorten to the empty-local name `:` or Shorten all writes `:` against a following keyword (`:SELECT`), which fuses; find why the padding misses it (the window's end, or the keyword lexing as a local name) and fix it in the shared code. (F3) Remove unused prefix on `PREFIX PREFIX:<http://example/>` deletes from the second `PREFIX` and leaves the keyword; the removal must delete the whole directive's tokens.

**Blocked by:** #21 (the checks that find these).

**Issue:** #30. **Wave:** 4. **Files (lock in bold):** **src/syntax/names.rs** (`token_edits` result); src/sparql/features/actions.rs; src/sparql/features/edits.rs; src/sparql/features/completion.rs; src/sparql/features/rename.rs; src/sparql/lexer.rs or src/sparql/mod.rs only if F2 or F3 needs it; the Turtle callers of `token_edits` only to follow its new result type

**Status:** proposed

- [ ] The replays from ticket 12's report pass: Expand s1 i15198, add PREFIX s1 i10886, completion s1 i116, rename s2 i11225, Shorten s1 i25341, removal s4 i21775 (`CHAOS_SEED=<s> CHAOS_ITERS=30000 CHAOS_ONLY=<i>`).
- [ ] Unit tests beside each fix: an edit that cannot keep its tokens is not offered (one per caller); `:` before `SELECT`; the `PREFIX PREFIX:` removal.
- [ ] 10 seeds at `CHAOS_ITERS=30000`, `-- mutated_sparql`, under `timeout 120`: no failure.
- [ ] Turtle callers unchanged in behaviour (their checks already pass on 10 seeds).
