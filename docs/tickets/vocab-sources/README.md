# Tickets: vocabulary sources

Spec: [docs/specs/vocab-sources.md](../../specs/vocab-sources.md). Fixes
#106. Lock files are in **bold**. Each ticket file carries its status.

| # | Ticket | Blocked by | Files |
|---|---|---|---|
| 01 | [The quick step: core W3C terms, `rdf:_n`, EARL and the hover line](01-quick-step.md) | none | `src/vocab.rs` (the lookup and its tests), `src/features/hover.rs`, **`tools/build-vocab.mjs`** (the corrections step, EARL's namespace), new `data/vocab-patches.tsv`, **`data/vocab.tsv`** (regenerated), **`test/unknown-term.test.mjs`**, new `tests/vocabulary.rs` |
| 02 | [The generator library, on small sources](02-library.md) | none | new `tools/vocab-lib.mjs`, new `test/vocab-build.test.mjs`, new `test/vocab/`, **`package.json`**, **`package-lock.json`** (two parsers added) |
| 03 | [The source step: manifest, frozen extracts, the script and CI](03-sources.md) | 01, 02 | **`tools/build-vocab.mjs`** (rewritten as the command), new `tools/check-vocab.sh`, new `data/vocab-sources.tsv`, new `data/vocab-frozen/`, `data/vocab-patches.tsv`, **`data/vocab.tsv`** (regenerated), **`package.json`**, **`package-lock.json`** (the package removed, the `vocab` script), `src/vocab.rs` (aliases, module doc, tests), `.github/workflows/ci.yml` (the audit job), `tests/jsonld_lints.rs`, `tests/vocabulary.rs`, **`test/unknown-term.test.mjs`**, fixes to `tools/vocab-lib.mjs` and its test if the real sources need them |
| 04 | [Close-out: docs and the full validation](04-close-out.md) | 03 | `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `AGENTS.md`, `THIRD_PARTY_NOTICES.md`, `web/build.mjs` (the size in its comment), the spec's status line, this README |

## How the lock files stay apart

- **`tools/build-vocab.mjs`**: 01 adds two things to the generator as it
  is. 02 does not open it and writes the library as a new file. 03 replaces
  its body with the command over the library. 01 and 03 never run together.
- **`data/vocab.tsv`**: written by 01, then by 03. It is generated. Nobody
  merges it by hand, and a conflict is settled by running the generator
  again.
- **`package.json`, `package-lock.json`**: 02 adds two `devDependencies`.
  03 removes one and may change the `vocab` script. 01 touches neither, and
  `npm run vocab` keeps working through wave 1 because the old package
  stays until 03.
- **`test/unknown-term.test.mjs`**: 01 adds its cases, then 03 adds its
  own. Both append new `test(...)` blocks and change none that exist.
- `src/vocab.rs`, `data/vocab-patches.tsv` and `tests/vocabulary.rs` are
  written by 01 and extended by 03, in that order.
- 01 and 02 share no file. The one thing between them is the format of the
  corrections file, which the spec fixes ("The inputs"). 01 reads it in the
  old generator, 02 in the library, and 03 deletes 01's reader.
- The new test data lives in `tests/vocabulary.rs` itself and in
  `test/vocab/`, not under `test/fixtures/`. The formatter, chaos and oxttl
  tests read every file under `test/fixtures/`.

## Waves

1. **01, 02** together.
2. **03**.
3. **04**.

01 can be released on its own before 03 starts.

## Validation

Tickets 01 to 03 run format, clippy and the test files they touch, each
under `timeout 120`, as their acceptance lists name them. Compiling is not a
test run: build first, then start the capped command. Only 04 runs the full
validation.

## Departures

- 01: the list grows by 1,298 bytes gzipped (733,090 to 734,388), over the
  budget until 03's comment cap applies. Shipping 01 alone would be slightly
  over it.
- 01: `owl:real` and `owl:rational` are now tagged `class` in semantic
  tokens, because `is_datatype` in `src/features/semantic.rs` knows only XSD
  and fixed `rdf:` names. Moved into 03.
- 02: triples are sorted by their N-Triples form before reading, and
  `owl:DeprecatedClass` / `owl:DeprecatedProperty` give kinds C and P. The
  package's `schema:supersededBy` marks 82 terms, not 83.
- 02: the RDA `regap` status namespace and IRI shape come from memory; 03
  checks them against the RDA files.
- 03: the comment cap is 160, not the 240 the spec estimated. Measured on
  the pinned sources (24,140 terms), the list weighs, gzipped at level 9:
  877,143 bytes at 400, 827,175 at 300, 790,529 at 240, 759,524 at 200 and
  724,421 at 160. Only 160 meets the budget of 733,090. The estimate assumed
  today's bytes per term; the added GS1, QUDT and DPV terms carry longer
  comments (comment text grows from 1.66 MB to 1.88 MB at 240). The list is
  3,055,418 bytes, 724,421 gzipped, 81 vocabularies and 24,140 terms; 4,418
  of its 18,473 comments are cut.
- 03: every fetched row of the spec fetched and parsed; none became frozen.
  The term counts added and removed equal the spec's table for all 22. Two
  deprecation counts differ from its "Deprecated after" column: `gs1` has
  204, not 87 (87 is how many of the 1.3 terms 1.18 deprecates; 1.18
  deprecates 204 in all), and `as` has 10, not 11 (the W3C file marks ten
  terms `owl:DeprecatedProperty`, `as:id` among them; `as:OrderedItems` is
  not marked).
- 03: the QUDT rows read the files under `src/main/rdf/` at tag `v3.5.3`, as
  the spec says, not the release zip the audit read; the term counts agree.
- 03: the RDA marker 02 wrote from memory is right: `u.nt` at `v5.5.2` uses
  `http://metadataregistry.org/uri/profile/regap/status` with
  `http://metadataregistry.org/uri/RegStatus/1008` on 129 terms and
  `RegStatus/1001` on the other 1,085. Nothing changed.
- 03: `owl:real` and `owl:rational` are datatypes in semantic tokens again
  (`is_datatype` knows the OWL 2 datatype map's two), with a unit test.
- 03: the JSON-LD corpus gets new true positives, listed in
  `tests/jsonld_lints.rs`: `deprecated-term` on the terms GS1 1.18 deprecates
  in GS1's own 1.16 file (in its three spellings) and on schema.org's
  superseded terms in schema.org's own file, and `unknown-term` on
  `gs1:AllergenTypeCode-SQ` and `-TN`, replacements GS1 names but never
  defines.
- 03: `brick.ttl` keeps three `unknown-term`s, listed in
  `tests/vocabulary.rs` with their reasons: `rdf:Class`,
  `unit:CentiM_H20_4DEG_C` (a zero for the letter O) and `unit:Kilo-FT3`.
  `dcat-ap-shacl.ttl` and `w3c-shacl.ttl` get none.
- 03: the loader no longer skips a vocabulary without terms: the generator
  refuses a source that yields none, and `geof` and `geor` have no row.
- 03: 11 rows have no licence in the publisher's file and none known
  otherwise; their licence column says "not stated in the file": `bibo`,
  `cc`, `dash`, `hydra`, `lvont`, `mads`, `rss`, `sdmx`, `sem`, `sioc` and
  `void`. The W3C-hosted files that state none say "W3C Document License".
  04 checks these before the notices file lists them.
