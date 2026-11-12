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
  comments (comment text grows from 1.66 MB to 1.88 MB at 240). After the
  review's sentence-cut and paragraph fixes, the list is 3,051,993 bytes,
  723,358 gzipped, 81 vocabularies and 24,140 terms.
- Review: a `.` after a single letter, cf, vs, No or Inc, or inside an open
  bracket, no longer ends a sentence; "etc." still may. schema.org's
  paragraph breaks, written as the characters `\n\n`, now end its first
  paragraph (65 comments carried a second paragraph; `main` has 108).
  `check-vocab.sh` exits 3 when only downloads failed, and CI retries only
  then, without `-f`; an HTTP error skips that file only, a connection
  failure skips the host's other rows. Two shared namespaces or aliases are
  refused.
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
- 04: `test/project.test.mjs` failed on two files of `test/fixtures/project`
  with `'unit' is not defined in the qudt vocabulary`. The fixture used
  `qudt:unit`, which QUDT 3 removed in favour of `qudt:hasUnit`; reporting it
  is what the spec asks for. The fixture now uses `qudt:hasUnit`.
- 04: of the 11 licences "not stated in the file", 7 are stated elsewhere by
  the publisher, and the manifest's licence column now names them (no hash
  changes): `bibo` CC-BY-SA-3.0 (DCMI's page for BIBO), `cc` CC-BY-4.0
  (creativecommons.org's content), `dash` Apache-2.0 (the TopQuadrant/shacl
  repository, which holds `dash.ttl`), `hydra` W3C Software and Document
  License (the HydraCG/Specifications repository), `lvont` CC-BY-SA-3.0
  (lexvo.org's legal page, for its RDF dump), `mads` public domain
  (id.loc.gov's about page) and `sdmx` CC-BY-SA-4.0 (the `LICENSE.md` beside
  the file, written for the SDMX vocabulary files). Four stay unknown and
  are listed so in `THIRD_PARTY_NOTICES.md`: `rss` and `sioc` (their
  specifications' notices exclude the format and the ontology terms), `sem`
  (a copyright line, no licence) and `void` (nothing on the archived page,
  in the file or in its repository).
- 04: the comment cap is 240, by the maintainer's choice, not the 160 that
  03 picked to meet budget 1. It is `COMMENT_CAP` in `tools/vocab-lib.mjs`.
  At 240 the list is 81 vocabularies, 24,140 terms, 3,280,105 bytes and
  788,028 gzipped at level 9 (at 160: 3,051,993 and 723,358). The cap cuts
  2,380 of the 18,473 comments, 397 of them inside a sentence (ending in
  "…"). The spec's budget 1 (no larger than 733,090 gzipped) is therefore
  not met: the list grows by 54,938 bytes gzipped against `main`'s, which
  is the budget.
  `SIZE_BUDGET` is now 800,000: the list at 240, rounded up to leave room
  for a publisher's refresh. The limit that matters is the engine's
  `WASM_GZIP_BUDGET`, unchanged at 3,700,000.
- 04: the engine (`web/app/engine.wasm`, gzipped at level 9): 3,395,322
  bytes on `main` (b36468e, 11,610,087 raw), 3,449,894 on this branch at
  the 240 cap (12,023,568 raw), so 54,572 bytes more; 3,385,401 at the 160
  cap. The budget leaves 250,106 bytes.
- 04: the spec's six budgets, measured on this branch at the 240 cap:
  1. The list at most 733,090 bytes gzipped: **fail**, 788,028, by the
     maintainer's choice of cap (above).
  2. `WASM_GZIP_BUDGET` 3,700,000 unchanged and `node web/build.mjs`
     passes: **pass**, 3,449,894.
  3. `node bench/perf.mjs --only big-file-peak` passes: **pass**, 54.6 bytes
     committed per input byte for `format --check` and 41.7 for
     `check --no-shacl`, budget 64.
  4. The keystroke rows do not move against `main`: **pass**, but measured
     on a loaded machine (load average 2.7 to 6 for ten minutes of waiting,
     3.1 to 5.1 during the runs; nothing of this work was running). Three
     runs of each tree, alternated; median p50 in ms, branch against `main`
     (range of the three in brackets): keystroke 1.5 [1.5-1.6] against
     1.6 [1.6]; Turtle 1.4 [1.4] against 1.4 [1.4]; JSON-LD 5.1 [5.0-5.5]
     against 5.0 [5.0-5.4]; near-miss 1.6 [1.6] against 1.6 [1.6]; big file
     5.0 [4.9-5.0] against 5.0 [5.0-5.3]; big vocabulary 7.1 [6.9-7.7]
     against 7.1 [6.9-7.6]; undeclared prefixes 254.6 [242.9-276.5] against
     256.6 [232.0-288.5]. Every difference is inside the spread of the runs.
  5. The root `package-lock.json` holds fewer than 43 packages: **pass**,
     25.
  6. `tools/check-vocab.sh`, run twice, downloads nothing the second time
     and gives the same result: **pass**; `-f` downloaded 25 files, the
     next run 0, and both found the committed list.
- 04: the editor extension's Node tests need `npm run build` first
  (`manifest.test.mjs` checks the built browser entry), so they ran after
  it. The public corpus file `dbpedia-ontology` could not be downloaded
  (downloads.dbpedia.org timed out, three tries), so `corpus_edits` ran on
  the other files without it. No test printed `skipped`.
