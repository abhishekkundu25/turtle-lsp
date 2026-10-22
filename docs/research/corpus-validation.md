# Corpus validation

> **Historical results.** Each run below is dated and describes the code of
> that release; later fixes may have changed the figures. The steps of
> `npm run corpus` just below are current. The README's "Tested against the
> real world" table has the current results.

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

## After the fixes (`docs/specs/corpus-findings.md`)

- Findings 1, 2 and 5 fixed: `qudt-units.ttl` opens in 67 ms (was 2.3 s),
  formats in 20 ms, and has 8,510 unknown-term warnings (was 9,609; the 1,099
  on terms the file defines are gone). The rest come from the outdated bundled
  QUDT snapshot; providing QUDT through `owl:imports` or a workspace file
  silences them.
- Finding 3 fixed: Brick's 24 SHACL violations are warnings naming the
  unresolved imports, once import resolution has run (about a second after
  start-up; the first validation before that still shows errors).
- Finding 4 (N-Triples memory) is open.

## N-Quads and SPARQL (2026-10-19, branch `nquads-sparql`, macOS arm64)

New sources: a 16 MB slice of a Web Data Commons RDFa N-Quads dump
(`wdc-rdfa.nq`, 57,000 quads) and the SIB `sparql-examples` repository at
commit `45e5b53` (1,327 `.ttl` files). The harness now also checks:

- N-Quads against oxttl's N-Quads parser, line by line (both must reject the
  same lines and agree on every other quad); a file with invalid lines is
  also checked without them (`NAME.valid.nq`) so the formatter sees the real
  data; every Turtle file under 5 MB gets an oxttl-serialized
  `NAME.gen.nq` (one named graph).
- Every `sh:select`, `sh:ask`, `sh:construct`, `sh:describe`,
  `spex:describe` and `sh:update` literal of the SIB examples as a SPARQL
  case, with the repository's `sh:prefixes` declarations as external
  prefixes: our verdict against spargebra's, no lexer error token in a valid
  query, and the formatter (same algebra with generated names renumbered,
  comments kept, idempotent; invalid queries refused).

Results (99 RDF cases, 2.56 million triples; 1,317 SPARQL cases):

| Check | Result |
|---|---|
| Old 68 RDF cases | Unchanged |
| `.gen.nq` (29 cases, 280,703 quads) | Same quads, formatter safe |
| `wdc-rdfa.nq` | Parsed in 31 ms (oxttl: 39 ms). Both reject the same 2 lines; oxttl also rejects 26 lines with `[` in an IRI that we accept (finding 7). `wdc-rdfa.valid.nq`: same, formatter safe |
| SIB queries | Same verdict as spargebra on 1,317 / 1,317; 0 error tokens; formatter safe on all 1,317; 0 panics, 0 timeouts |
| SPARQL time | Parse 134 ms, format 533 ms for all 1,317 (max 0.4 / 1.2 ms per query) |
| Server, `wdc-rdfa.nq` | Diagnostics 77 ms, semantic tokens 37 ms, outline 98 ms, 410 MB, clean exit; no CPU once settled |
| Server, longest SIB query (3.4 KB) | Diagnostics 1 ms, formatted, 11 MB, clean exit |

Findings:

6. *Fixed on the branch.* Local names with two or more dots (`ec:1.1.1.353`,
   `ec:3.-.-.-`, used by five UniProt and Rhea queries) are valid by the
   SPARQL grammar but rejected by spargebra 0.4. The server escapes their
   dots in the text it gives spargebra (`ec:1\.1\.1\.353`, the same IRI), for
   `.rq` documents, the formatter's check and SHACL-SPARQL queries.
7. *Open.* The RDF parser accepts `[` and `]` inside an IRI
   (`<http://a/s?f[1]=x>`), which the Turtle / N-Triples grammar allows as a
   character but RFC 3987 does not outside an IP literal; oxttl rejects
   them. Pre-existing, in all four RDF syntaxes.
