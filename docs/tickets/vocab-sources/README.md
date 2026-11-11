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
