# 03: The source step: manifest, frozen extracts, the script and CI

**What to build:** the vocabulary list comes from sources turtle-lsp names
and pins, and the abandoned package is gone.

For a user: schema.org 30.1, QUDT 3.5.3, DCAT 3, GeoSPARQL 1.1, RiC-O 1.1,
the DBpedia ontology snapshot and the other fetched vocabularies of the
spec's "Sources" table are current. GS1 is checked in
`https://ref.gs1.org/voc/` and in its two older namespaces, DPV in
`https://w3id.org/dpv#`. The 578 Urdu-script names are gone from `dbo`.
Terms that schema.org, QUDT, GS1, RDA and Activity Streams have deprecated
get the `deprecated-term` hint. A hover comment ends at a sentence or with
"…". The list is no larger, gzipped, than it is today.

For the maintainer: `data/vocab-sources.tsv` lists every source with its
version, licence and checksum. `tools/check-vocab.sh` downloads the fetched
rows, verifies them, regenerates and compares. `npm run vocab` does the
same and writes the list. CI's audit job runs the script. The old generator
code, its corrections step and `@zazuko/rdf-vocabularies` are deleted in
this ticket, not kept beside the new command.

The server's loader reads a vocabulary's aliases from its `@ns` line, and
the `ALIASES` constant is deleted (spec, "The inputs").

Work in this order, so that each part is checked before the next builds on
it: the command and the script; the frozen extracts with frozen rows only,
proved equal to today's list; the fetched rows, one publisher at a time,
each compared with the spec's "What the source step is expected to change";
the comment cap; the aliases in the server; CI.

**Blocked by:** 01, 02.

**Status:** done

- [ ] The manifest has 81 vocabularies: the 22 fetched and 59 frozen of the spec. `geof` and `geor` have no row. Every fetched URL is `https` and names a release, tag, commit or versioned file where the publisher has one. Every row has a licence. A fetched row that cannot be fetched or parsed becomes a frozen row plus corrections, with the reason under Departures in the README.
- [ ] The frozen extracts are cut from the committed list by a script, not by hand. For every frozen vocabulary, the regenerated terms equal today's in every column but the comment, and each changed comment is the old one cut by the new rule. The comparison's output is in the report.
- [ ] `tools/check-vocab.sh` passes. A second run downloads nothing and passes. `tools/check-vocab.sh --write` leaves `git diff --exit-code data/vocab.tsv` clean. `-f` downloads again and passes.
- [ ] A changed source fails by name: flip one byte of a downloaded file, and of a frozen extract, and the script names the row with the expected hash and the one found. An interrupted download leaves only a `.part` file, and the next run recovers. Show each once by hand and say so in the report.
- [ ] The regeneration's summary agrees with the spec's table of expected changes, or the difference is explained under Departures. No `dbo` local name is outside ASCII. `sh:js` and the other 13 SHACL-JS terms are still known. `quantitykind` has the 31 propulsion kinds.
- [ ] The comment cap is the largest of 400, 300, 240, 200 and 160 that keeps the list at or under 733,090 bytes gzipped at level 9. The cap, the list's size and its term count are recorded under Departures.
- [ ] `data/vocab-patches.tsv` gains `mads:Medium`, `dtype:dateUnion`, `dtype:defaultObjectValue` and `dtype:objectValue`. A line of 01's that a fetched source now provides is deleted, since the generator refuses it.
- [ ] `@zazuko/rdf-vocabularies` is out of `package.json` and `package-lock.json`, and the lock file has fewer than 43 packages. `grep -ri zazuko` finds it only in `README.md`, `THIRD_PARTY_NOTICES.md` and `CHANGELOG.md`, which 04 owns. `npm audit --audit-level=moderate` passes.
- [ ] The loader takes aliases from the `@ns` line and `ALIASES` is gone. Unit tests in the vocabulary module: `https://schema.org/Person` is `schema`; `Product` is `gs1` in each of its three namespaces; the `https` spelling of GeoNames is `gn`; `http://www.w3.org/ns/dpv#` is no vocabulary. The module doc names the manifest, not the package.
- [ ] `test/unknown-term.test.mjs` gains: a GS1 term added since 1.3 is known in the canonical and in a legacy namespace, and a GS1 typo is reported; a DPV 2 term is known in `https://w3id.org/dpv#`; `qudt:applicableSystem` and a unit added since 2.1.18 are known; a superseded schema.org term and a deprecated QUDT unit get the `deprecated-term` hint.
- [ ] `tests/jsonld_lints.rs` loses its two entries marked "until the snapshot is refreshed" (GS1 and `dcat:next`), and its other entries still hold. `tests/vocabulary.rs` gains `brick.ttl`, `dcat-ap-shacl.ttl` and `w3c-shacl.ttl` of the public corpus, each analysed alone, every remaining `unknown-term` listed with its reason, with the usual rule for an absent corpus.
- [ ] CI's audit job runs `npm ci`, restores the downloads from a cache keyed on the manifest's hash, and runs the script after `tools/check-vendor.sh`, with one retry as the corpus step has. The nightly run passes `-f`. Actions stay pinned to commit SHAs.
- [ ] `cargo fmt --check` and `cargo clippy --release --locked --all-targets -- -D warnings` pass.
- [ ] After `cargo build --release --locked`, `cargo test --release --locked --lib --test vocabulary --test jsonld_lints --no-run`, `tools/corpus/fetch-jsonld.sh` and the download of the three corpus files (the `awk` filter and `tools/corpus/fetch.sh` call of CI's corpus step, with those three names), these pass:
  - `timeout 120 cargo test --release --locked --lib -- vocab:: features::hover:: features::diagnostics:: features::completion:: features::semantic:: sparql::features:: --skip timing_`
  - `timeout 120 cargo test --release --locked --test vocabulary`
  - `timeout 120 cargo test --release --locked --test jsonld_lints`
  - `timeout 120 node --test --test-timeout=30000 test/unknown-term.test.mjs test/vocab-build.test.mjs`
- [ ] The report lists the removed terms per vocabulary as the script printed them, what was not run (everything else is 04's) and anything skipped.
