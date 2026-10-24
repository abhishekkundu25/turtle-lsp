# 10: JSON-LD multi-byte spans: fix the producers, full corpus in the shard

**What to build:** The JSON-LD chaos shard runs ASCII inputs only (tests/chaos.rs `mutated_jsonld`, the `is_ascii` filter) because in malformed JSON-LD several spans end inside a multi-byte character: a dropped key written without quotes, a value without its closing quote; the diagnostics turn them into positions, `line-index` panics ("invalid offset") and `check` aborts on such a file. `semantic::encode_with` also slices the text at a token boundary inside a character. The ignored test `jsonld_spans_stay_on_character_boundaries` reproduces them. Fix each span producer in src/jsonld/ so every span it emits starts and ends on a char boundary (snap the way src/sparql/validate.rs `check_tokens` now does, or compute from the tokenizer's char offsets), fix `encode_with`, remove the `#[ignore]`, and remove the `is_ascii` filter so the shard mutates the whole corpus. This is one of the two grandfathered exemptions; the spec's "no exemptions" line is only true once it is gone.

**Blocked by:** nothing.

**Issue:** #27. **Wave:** 1. **Files (lock in bold):** src/jsonld/** (span producers), src/semantic.rs (`encode_with`); **tests/chaos.rs** (the `is_ascii` filter and the ignored test only)

**Status:** proposed

- [ ] `jsonld_spans_stay_on_character_boundaries` passes and is no longer ignored.
- [ ] A unit test per producer fixed, with the minimal input from the ignored test's doc comment (unquoted key after a multi-byte character, unterminated value with a multi-byte character).
- [ ] `check` on each such file exits 1 with diagnostics and no abort (test/cli.test.mjs or test/jsonld.test.mjs).
- [ ] The JSON-LD shard runs the full corpus; 10 push-sized runs (`CHAOS_ITERS=30000`, `-- mutated_jsonld`) pass under `timeout 120` each.
- [ ] No span producer in src/jsonld/ returns a byte offset without a boundary check (grep for `Span::new` and `..` slices in the report).
