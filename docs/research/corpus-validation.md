# Corpus validation

`npm run corpus` downloads public RDF files (`tools/corpus/sources.tsv`) into
`test/corpus/` (not tracked), then:

1. `cargo run --release --example corpus` compares our parser with oxttl on
   every file, checks that formatting keeps the graph and the comments and is
   idempotent, and reports panics, hangs (over 300 s) and timings. RDF/XML
   files are converted to N-Triples first; each Turtle file under 5 MB also
   gets a TriG copy serialized by oxttl (`NAME.gen.trig`). Output:
   `target/corpus/report.md` and `target/corpus/details/`.
2. `node tools/corpus/lsp-cost.mjs` runs the real server once per file and
   records the time to first diagnostics, formatting, semantic tokens and
   symbols, the memory afterwards, idle CPU, and whether the server exits
   cleanly. Output: `target/corpus/lsp-cost.md`.

## First run (2026-10-18, v0.2.0, macOS arm64)

41 downloaded files, 68 cases, 252 MB, 2.17 million triples: schema.org,
DBpedia (ontology and resources), Wikidata entities, QUDT, Brick, SAREF, gist,
DCAT-AP shapes, W3C vocabularies (SHACL, PROV, ORG, DCAT, SOSA/SSN, OWL, RDFS,
Time, SKOS, ODRL, vCard, LOCN, ActivityStreams), Dublin Core, FOAF, and OBO
ontologies (BFO, RO, PATO, Gene Ontology).

Parser and formatter: no findings. Every case parses to the same triples as
oxttl; every formatted file keeps its graph and comments and is a fixed point;
no panics or hangs. The Gene Ontology (1.28 million triples, 118 MB of
N-Triples) parses in 438 ms and formats in 777 ms.

Server: every file opened, answered, went idle and exited cleanly. Findings:

1. **Slow diagnostics with many unknown terms.** `qudt-units.ttl` (2.3 MB,
   9,609 unknown-term warnings) takes 2.3 s to the first diagnostics and
   4.6 s after each edit; `qudt-quantitykinds.ttl` (3,952 warnings) takes
   0.9 s and 1.7 s. Requests such as formatting wait behind it. The time grows
   with the number of warnings (about 0.24 ms each), most likely the "did you
   mean" search run for every unknown term. Files without many unknown terms
   of the same size take 7 to 20 ms.
2. **Unknown-term false positives.** The bundled QUDT snapshot lacks current
   terms (for example `qudt:applicableSystem`), and a term is reported as not
   defined in its vocabulary even when the open file itself defines it: 1,099
   of the 9,609 warnings in `qudt-units.ttl` are on terms the file defines
   (`unit:A-HR-PER-DEG_C` …).
3. **SHACL errors caused by unresolved imports.** `brick.ttl` gets 24 SHACL
   errors ("is not an instance of qudt:QuantityKind") because the QUDT
   vocabularies it imports are not available without network imports. The 13
   unresolved imports are reported as information, but the violations are
   errors.
4. **N-Triples memory.** `dbpedia-ontology.nt` (3.9 MB) leaves the server at
   363 MB; Wikidata Q42 takes 211 MB as N-Triples and 103 MB as Turtle.
5. **Formatter base IRI (latent).** The formatter's safety check re-parses its
   output without a base IRI, so a document parsed with one is refused when it
   has relative IRIs. The server never parses with a base, so users do not hit
   this today.
