# 16: JSON-LD edits keep the token stream on every input (found by ticket 12)

**What to build:** Ticket 12's token checks fail 14 to 27 times per JSON-LD seed at 30,000 iterations. (F4) A rename writes an absolute `http://...` IRI into an unquoted word; its `//` then lexes as a comment that swallows the rest of the line (about 195 cases over 10 seeds). A JSON-LD term lives in a string: rename writes only into string tokens and refuses (the usual "cannot rename here") where the term is not one. (F5) Completion's auto-import puts the `@context` member inside an unclosed string, or after a `//` comment on the same line, which absorbs it. The insertion point must be found from the JSON tokens at a member boundary of the top-level object, as ticket 02 did for Turtle, with the inserted text kept from fusing with its neighbours.

**Blocked by:** #21.

**Issue:** #31. **Wave:** 4. **Files (lock in bold):** src/features/navigation/jsonld.rs; src/features/completion/jsonld.rs; src/jsonld/json.rs only if a token helper is needed

**Status:** proposed

- [ ] The replays from ticket 12's report pass: rename s2 i15100 and default seed i8502; completion s4 i9519 and s7 i9484.
- [ ] Unit tests beside each fix: rename on an unquoted word is refused; auto-import with an unclosed string and with a `//` comment on the line lands at a member boundary and lexes to the expected tokens.
- [ ] 10 seeds at `CHAOS_ITERS=30000`, `-- mutated_jsonld`, under `timeout 120`: no failure.
