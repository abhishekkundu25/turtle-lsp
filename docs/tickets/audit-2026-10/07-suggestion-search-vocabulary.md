# 07: Suggestion search and vocabulary data

**What to fix:** the `undescribed-term` "Did you mean" search makes keystrokes take
seconds. For each flagged IRI, `suggest` (`src/features/lint.rs:34-54`) copies
`k.terms_in(ns)` into a `Vec`, runs `is_described` on each, allocates two `String`s in
`plausible` and one `to_lowercase` per candidate, and computes a full `edit_distance`
(five `Vec`s) with no length prefilter and no cache across keystrokes, on the single main
loop. Measured twice independently: 807 ms p50 / 1.35 s p95 per keystroke with 1,000
near-miss IRIs against a 7k-term namespace (3.6 ms without them); 4.9–9.9 s against a
30k-term namespace; `check` goes from 0.18 G to 19.7 G instructions. Any partly-written
project reaches this, and hover and completion wait behind it. `vocab::suggest`
(`src/vocab.rs:66-86`) already prunes by length; the two searches should be one.

Vocabulary data defects in the same files: `geof` and `geor` (GeoSPARQL) are registered
with **zero terms** (`src/vocab.rs:117-146`, `data/vocab.tsv`), so every GeoSPARQL
function (`geof:sfWithin`, `geof:distance`) is reported as `unknown-term` and
`--max-warnings 0` fails; `lookup` splits at the last `#`/`/`/`:` so Open Graph terms
with `:` in the local name (`og:audio:album`) can never be found (`vocab.rs:110-114`,
duplicated in `knowledge.rs:156`); `https://schema.org/` is not an alias of the `schema`
section.

**Files owned:** `src/features/lint.rs`, `src/vocab.rs`, `src/knowledge.rs`
(`namespace_of` → `vocab::split_namespace`; optional `described_locals(ns)` cache keyed
by `described_generation()`), `tools/build-vocab.mjs`, `data/vocab.tsv` (regenerated with
`npm run vocab`; must stay byte-reproducible), `test/lint.test.mjs`,
`test/unknown-term.test.mjs`, `test/knowledge.test.mjs`.
**Hands off:** `src/features/completion.rs` (ticket 15/20), `src/features/diagnostics.rs`
(ticket 19), `README.md` (ticket 22 updates the vocabulary count).

**Blocked by:** nothing.

**Status:** ready-for-agent

- [ ] One allocation-free `edit_distance` with reusable buffers and an early exit at
      `max`, shared by `vocab::suggest` and `lint::suggest`.
- [ ] Per diagnostics run, one lower-cased candidate table per namespace (bucketed by
      length) built once from the described terms; `plausible` allocation-free; a cap on
      suggestions per document (the rest get no "Did you mean").
- [ ] Release-only budget test (like `unknown-term.test.mjs:29`): 1,000 distinct
      undescribed IRIs against a 30,000-term own namespace diagnose in under 50 ms.
      `vocab::tests::suggest_matches_the_unpruned_search` plus the same property for
      lint suggestions.
- [ ] `load()` skips (or `build-vocab.mjs` omits) vocabularies with no terms; unit test:
      every `VOCABS.all` entry has at least one term; `geof:sfWithin` gets no
      `unknown-term` in Turtle or SPARQL.
- [ ] `lookup` tries each delimiter from the right and keeps the longest registered
      namespace (or the tool drops `:` locals and documents it); `knowledge::namespace_of`
      calls the shared helper. Test: `og:audio:albm` is flagged, `og:audio:album` is not.
- [ ] `https://schema.org/` resolves to the `schema` vocabulary. Test.
- [ ] `cargo test --release` (vocab, knowledge, lint), the three Node files,
      `test/project.test.mjs` green; `npm run vocab` reproduces `data/vocab.tsv` byte for
      byte.
