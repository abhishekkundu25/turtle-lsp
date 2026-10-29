# 04: `label-whitespace` and its trim fix

**What to build:** in `features::curation`, `label-whitespace` (warning) on an `rdfs:label`, `skos:prefLabel` or `skos:altLabel` literal of any subject whose value starts or ends with whitespace (spaces, tabs, newlines, as `char::is_whitespace`). A quick fix rewrites only the literal's lexical text between its quotes: the leading and trailing whitespace (written raw or as escapes such as `\t`, `\n`, ` `) is removed; quote style, other escapes, language tag and datatype stay. JSON-LD edits the string value. The fix is unsafe: add the code beside `RENAME_CODES` in `src/batch.rs` so `check --fix` applies it only with `--unsafe-fixes`.

**Blocked by:** 01.

**Status:** done

- [x] Unit tests: leading, trailing, both, tab and newline (raw and escaped), long-quoted `"""…"""`, single quotes, language tag and datatype kept, JSON-LD value, a label that is all whitespace (fix leaves `""`), no diagnostic on clean labels or on other predicates.
- [x] `batch` test: the fix is not safe; `fix_all` applies it only with `include_unsafe`.
- [x] New `test/label-lints.test.mjs`: the diagnostic and the fix applied over LSP; `check --fix` leaves it and `--fix --unsafe-fixes` trims it.
- [x] `tests/chaos.rs` passes unchanged (it checks every quick fix's tokens and graph).
