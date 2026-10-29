# 06: Own `deprecated-term`

**What to build:** a use (not in subject position) of an IRI that `Knowledge::deprecated_here` reports is a `deprecated-term` warning with the deprecated tag, in Turtle-family, JSON-LD and SPARQL documents. Bundled-vocabulary deprecations stay hints. The semantic-token deprecated modifier and hover ("deprecated") also consult `deprecated_here`. Edit only `vocabulary_terms` in `src/features/diagnostics.rs` (or add a sibling function called next to it).

**Blocked by:** 02.

**Status:** done

- [x] Unit tests: warning on use, none on the subject's own description, `false` clears it, vCard stays a hint, SPARQL `.rq` use flagged, JSON-LD use flagged; semantic modifier and hover.
- [x] `test/lint.test.mjs`: deprecated in one file, used in another; marking it false clears the warning.
