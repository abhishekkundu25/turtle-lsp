# Vocabulary sources: pinned publishers, patches and a check

Status: proposed. Fixes #106. Tickets:
[../tickets/vocab-sources/](../tickets/vocab-sources/README.md).

## Problem Statement

`unknown-term` warns about terms that are valid. The check treats the bundled
vocabulary list (`data/vocab.tsv`, 83 vocabularies, 20,993 terms) as
complete. The list is generated from one npm package,
`@zazuko/rdf-vocabularies` 2023.1.19. Its data dates from 2022, and its
maintainers have since replaced it with `@vocabulary/<prefix>` packages, so
`npm outdated` never reports it. An audit of all 83 vocabularies against
each publisher's current file (2026-11-12) found these defects:

- **Terms that only a specification defines.** `xsd` lacks all 14
  constraining facets, `owl` lacks `real` and `rational`, and `rdf` lacks
  `reifies`, `dirLangString` and `langRange`. The file in issue #106 gets
  four false warnings. The successor packages lack the same terms.
- **A family no list can hold.** `rdf:_1`, `rdf:_2` and every other
  container membership property are reported as unknown.
- **A wrong namespace.** EARL is bundled under `https://www.w3.org/ns/earl#`.
  The namespace is `http://www.w3.org/ns/earl#`, so real EARL reports get no
  vocabulary support.
- **Stale data.** schema.org is version 14 (225 terms of 30.1 are missing).
  QUDT is 2.1.18 (1,394 units of 3.5.3 are missing, and so are
  `qudt:deprecated` and `qudt:applicableSystem`, which every current QUDT
  file uses). DCAT is version 2, GeoSPARQL 1.0, RiC-O 0.2. GS1 and DPV are
  checked in namespaces their publishers have left.
- **Junk.** `dbo` holds 578 names in Urdu script that DBpedia never defined.
  They appear in completion and in "did you mean".
- **Missed deprecations.** The generator reads only `owl:deprecated`. None
  of the 83 schema.org terms that carry `schema:supersededBy` is marked, and
  a QUDT 3 file, which uses `qudt:deprecated`, would mark nothing.
- **A hover that says more than it knows.** Hover on `xsd:maxExclusive`
  says "From the xsd vocabulary" although the list has no such term. Hover
  checks only the namespace.

On the W3C test suites committed under `tests/w3c`, `turtle-lsp check`
reports 211 `unknown-term` warnings today. 160 of them are false:
`rdf:reifies` 147 times, `rdf:dirLangString` once, `rdf:_1` and `rdf:_2`
six times each. On the public corpus, the QUDT schema and OWL-Time get false
warnings for `xsd:pattern`, `xsd:minInclusive` and `xsd:maxInclusive`, and
gist for `owl:real` and `owl:rational`.

Moving to the successor packages would not fix this, and would break four
vocabularies. `@vocabulary/dbo` is a 2016 dump that drops 193 valid terms.
`@vocabulary/gs1` declares one namespace and holds its data in another, so
it would yield no term. `@vocabulary/sh` drops the 14 SHACL-JS terms.
`@vocabulary/schema` is version 29.4, which lacks four terms the list has
today.

## Solution

turtle-lsp takes each vocabulary from a source it names and pins, as it
already does for its vendored crates and its test corpus.

For a user:

1. Every term of RDF 1.2, RDF Schema, OWL 2, XSD 1.1, SHACL 1.0 and SKOS is
   known, the terms that only the specifications define included. `rdf:_1`
   and its siblings are known.
2. schema.org, QUDT, DCAT, GS1, DPV, GeoSPARQL, RiC-O, the DBpedia ontology
   and eight smaller vocabularies are their publishers' current releases.
   GS1 and DPV are checked in their current namespaces, and EARL in its real
   one.
3. A term its publisher has deprecated gets the `deprecated-term` hint,
   whichever of five markers the publisher uses.
4. Hover says "From the X vocabulary" only for a term the list has.
5. The list is no larger, gzipped, than it is today, so the browser
   editor's engine does not grow with it.

For the maintainer:

1. One file lists every source with its version and checksum. One file
   lists turtle-lsp's own corrections, each with its reason.
2. One command downloads the pinned sources, verifies them and regenerates
   the list. CI runs the same command and fails when the committed list
   differs.
3. The abandoned dependency is gone. It brings 39 of the 43 packages in the
   root lock file. The two parsers that replace it bring fewer.

The work lands in two steps. The first removes the false warnings of issue
#106 with the generator as it is. The second replaces the source of the
data.

## User Stories

1. As an ontology author, I want `xsd:minInclusive`, `xsd:pattern` and the other XSD facets accepted inside `owl:withRestrictions`, so that a datatype restriction gets no false warning.
2. As an ontology author, I want `owl:real` and `owl:rational` accepted as datatypes, so that an OWL 2 range gets no false warning.
3. As an RDF 1.2 author, I want `rdf:reifies` and `rdf:dirLangString` known, so that the terms my reified triples and directional strings expand to are not flagged.
4. As an ontology author, I want `rdf:langRange` known, so that an OWL 2 restriction on `rdf:PlainLiteral` gets no false warning.
5. As a data author, I want `rdf:_1`, `rdf:_2` and every later membership property known, so that an RDF container gets no warning.
6. As a data author, I want `rdf:_0`, `rdf:_01` and `rdf:_x` still reported, so that the membership rule hides no typo.
7. As a data author, I want a typo such as `rdfs:lable` to keep its warning and its fix, so that the corrections cost me no true finding.
8. As a test-report author, I want EARL terms checked in `http://www.w3.org/ns/earl#`, so that my reports get hover, completion and typo checks.
9. As a catalogue author, I want the DCAT 3 terms (`dcat:DatasetSeries`, `dcat:inSeries`, `dcat:version`) known, so that DCAT-AP 3 data gets no false warning.
10. As a web author, I want schema.org 30.1, so that `schema:Certification` and the other terms added since version 14 are known.
11. As a web author, I want a hint on a schema.org term that carries `schema:supersededBy`, so that I move to its successor.
12. As a building-data author, I want the QUDT units and quantity kinds that Brick uses known, so that a Brick model gets no QUDT warning when QUDT itself is not loaded.
13. As a QUDT author, I want `qudt:deprecated`, `qudt:applicableSystem` and the other properties of QUDT 3 known, so that editing a current QUDT file gives no false warning.
14. As a QUDT user, I want a hint on a unit QUDT has deprecated, so that I pick its replacement.
15. As a QUDT user, I want a unit QUDT has removed to be reported, so that I find stale data.
16. As a supply-chain author, I want GS1 terms checked in `https://ref.gs1.org/voc/` and in the two older `gs1.org` namespaces, so that both spellings get the current vocabulary.
17. As a privacy engineer, I want DPV 2 checked in `https://w3id.org/dpv#`, so that current DPV data gets vocabulary support.
18. As an archivist, I want RiC-O 1.1, so that the properties renamed since 0.2 are known and the removed ones are reported.
19. As a geospatial author, I want the GeoSPARQL 1.1 terms (`geo:asGeoJSON`, `geo:hasCentroid`) known.
20. As an author of vCard, ShEx, DOAP, GeoNames, Open Graph or SPARQL Service Description data, I want the terms their publishers added since 2022 known.
21. As a DBpedia user, I want completion and "did you mean" for `dbo:` free of the 578 Urdu-script names, so that suggestions are real terms.
22. As a user of a vocabulary whose publisher's site is gone (VoID, GTFS, SDMX), I want it to stay bundled as it is, so that a dead server takes nothing away from me.
23. As a SHACL author, I want `sh:js` and the other SHACL-JS terms to stay known, so that a refresh takes no SHACL 1.0 term away.
24. As a user, I want hover on a term the list lacks to say nothing about a vocabulary, so that hover and the warning agree.
25. As a user, I want hover on a known term to keep its "From the X vocabulary" line.
26. As a user, I want a hover comment to end at a sentence, or with "…" when it was cut inside one, so that I can tell a cut text from a whole one.
27. As a user, I want deprecated terms ranked below current ones in completion for the vocabularies that mark them, so that I pick current terms first.
28. As a SPARQL author, I want the same answers in queries as in Turtle, so that a term valid in my data is valid in my query.
29. As a JSON-LD author, I want the same answers through a context, so that a GS1 or DCAT 3 document gets no false warning.
30. As a user, I want a term my workspace or an import describes to stay known whatever the list says, as today.
31. As a browser-editor user, I want the refreshed list to add nothing to the engine's download, so that the page loads as fast.
32. As a user, I want the server to make no network request for vocabularies, as today.
33. As the maintainer, I want one row per source with its publisher, version and checksum, so that I can answer "which version of X do we ship" by reading one file.
34. As the maintainer, I want to move a vocabulary to a new release by editing its row and running one command, so that an update is one small diff.
35. As the maintainer, I want every correction of ours in one file with its reason, so that no term is known for a reason nobody can find.
36. As the maintainer, I want a correction the publisher has made unnecessary to fail the build, so that the corrections file does not collect dead lines.
37. As the maintainer, I want a source that yields no term to fail the build, so that a moved namespace is caught when it happens.
38. As the maintainer, I want a publisher's silent change of an unversioned file found by the nightly run, so that it does not fail an unrelated pull request.
39. As a reviewer, I want a regeneration to print the terms each vocabulary gained and lost, so that I can check removals against the publisher's notes.
40. As a contributor, I want the generator's tests to run without the network, on every platform CI has.
41. As the maintainer, I want no dependency that `npm outdated` cannot see, and fewer packages for `npm audit` to read.
42. As the maintainer, I want each source's licence recorded beside it, so that the notices file lists what the binary ships.
43. As the maintainer, I want the first step shippable alone, so that the false warnings of issue #106 go away before the source work is done.

## Implementation Decisions

### The inputs

- **The source manifest** (`data/vocab-sources.tsv`). Tab-separated, `#`
  for comments, one row per source file, with the columns `prefix`,
  `namespace`, `aliases`, `format`, `url`, `version`, `licence` and
  `sha256`. `aliases` holds other spellings of the namespace, separated by
  spaces, or `-`. `format` is `ttl`, `nt`, `rdfxml` or `tsv`. `version` is
  what the publisher calls the release. `sha256` is the hash of the bytes
  the generator reads. A vocabulary with more than one file repeats its
  prefix, and its rows must agree on namespace and aliases. The manifest
  replaces the package's prefix list as the list of bundled vocabularies.
- **Two kinds of source.** A fetched source (`ttl`, `nt`, `rdfxml`) is
  downloaded from `url`, which must be `https`. A frozen source (`tsv`) is
  an extract committed in the repository, one file per vocabulary under
  `data/vocab-frozen/`, holding the vocabulary's `term` lines as
  `data/vocab.tsv` writes them. For a frozen row, `url` names the
  publisher's file the extract was compared with, and `version` says when.
- **The corrections file** (`data/vocab-patches.tsv`). Tab-separated, one
  correction per line, with the columns `action`, `prefix`, `local`,
  `reason`, and then the columns of a `term` line: kind, flags, label,
  comment, supers, domains, ranges. The only action is `add`. `reason`
  names the specification section that defines the term and says why the
  source lacks it. Labels and comments are taken from the specifications.
- **The output** keeps its format. One addition: an `@ns` line carries the
  vocabulary's aliases as further tab-separated fields. The server's loader
  reads them in place of the `ALIASES` constant, which is deleted. A loader
  that reads only two fields still loads the file.

### Which vocabularies are fetched and which are frozen

A vocabulary is fetched when the audit found its terms differ from the
publisher's current file and the publisher serves that file over `https` in
a format the generator reads. Every other vocabulary is frozen. A
difference that fetching cannot deliver becomes a line of the corrections
file. The table under Further Notes gives each vocabulary's source.

- **22 fetched.** schema.org 30.1, seven QUDT 3.5.3 vocabularies, the
  DBpedia ontology, GS1, DPV, RDA, RiC-O, GeoNames, DOAP, GeoSPARQL, Open
  Graph, ShEx, DCAT, vCard, SPARQL Service Description and Activity
  Streams.
- **59 frozen**, 4,730 terms and 780 KB of extracts. 48 match their
  publishers term for term. `rdf`, `rdfs`, `owl`, `xsd`, `time`, `xhv`,
  `dtype` and `mads` lack only terms the corrections file adds. `sh` stays
  SHACL 1.0: its namespace document now serves the SHACL 1.2 draft, which
  drops the SHACL-JS terms. `earl` keeps its 34 terms and gets its
  namespace from the manifest. `frbr`, `xhv` and `dtype` keep four extra
  terms between them that their publishers do not list. They are harmless.
- **2 dropped.** `geof` and `geor` have no terms and are not registered
  today. They get no manifest row, which changes nothing a user sees.

Frozen extracts exist for three reasons. First, eight of these
vocabularies cannot be fetched or parsed: VoID's host is dead, GTFS's
namespace is gone, SDMX answers 503, SEM's certificate has expired, QUDT
withdrew `discipline`, the published files of `ical` and `mads` are RDF/XML
that a strict parser rejects, and `xhv` is published only as RDFa. Second,
fetching the other 51 would add 51 downloads, most of them of unversioned
files, and change no term. Third, the extract is what the audit compared,
so freezing it adds no new claim. The `sha256` of a frozen row covers its
extract, so an edit to an extract shows as a manifest change. A correction
goes in the corrections file, with its reason, never into an extract.

The corrections file needs no `drop` action. The `dbo` junk goes away
because the DBpedia ontology snapshot does not contain it. The SHACL-JS
terms stay because `sh` is frozen.

### The generator

- **A library and a command.** The library turns sources, the manifest and
  the corrections into the text of `data/vocab.tsv`. It reads no file and
  opens no connection. The command hands it the bytes: it reads the
  manifest, the corrections, the frozen extracts and a folder of downloaded
  sources, and writes or compares the output.
- **Parsers.** `n3` for Turtle and N-Triples, `rdfxml-streaming-parser` for
  RDF/XML, both pinned to exact versions as the other dev dependencies are.
  RDF/XML is needed for RiC-O, GeoNames and DOAP, whose publishers serve
  nothing else. The regular expression that reads N-Quads today is deleted.
- **Checksums at the boundary.** The library receives each source as
  bytes, hashes them and refuses a source whose hash is not the manifest's,
  before any parsing. No path builds from unverified bytes, and the tests
  can show the refusal without a download.
- **What a term is** stays as today: a subject IRI in the vocabulary's
  namespace whose local name has no whitespace, `#` or `/`. A subject in an
  alias namespace counts too, which is how GeoNames 3.3 is read (its file
  declares the `https` spelling). Kind and label are chosen as today, and
  so is the property a comment is taken from.
- **Deprecation** is a table of markers, not a chain of conditions:

  | Marker | Publisher |
  |---|---|
  | `owl:deprecated true` | most, GS1 among them |
  | `rdf:type owl:DeprecatedClass` or `owl:DeprecatedProperty` | Activity Streams |
  | `schema:supersededBy`, with either spelling of the namespace | schema.org |
  | `qudt:deprecated true` | QUDT 3 |
  | `regap:status` with the value `RegStatus/1008` | RDA |

  A frozen extract keeps the flags it has.
- **Comments.** A comment is the first paragraph of the source's text, with
  whitespace collapsed. A comment longer than the cap ends at the last
  sentence end inside the cap. When there is none, it ends at the last word
  inside the cap, followed by "…". The cap is one constant. A frozen
  extract's comments, already one paragraph, get the cap too. Today's rule
  cuts at 600 characters without a mark, which 368 terms reach.
- **Order.** Vocabularies by prefix and terms by local name, both by code
  point. Today's order follows the locale of the machine that runs the
  generator.
- **Failures.** The generator stops with a message that names the row or
  line when: a source's hash differs, a source yields no term, a
  vocabulary's rows disagree, a correction names an unknown prefix, a
  correction adds a term the source already has, a frozen extract uses a
  prefix the manifest lacks, or the output is over its size budget.
- **A summary.** A regeneration prints, per vocabulary, how many terms were
  added and removed against the committed file, and lists the removed ones.

### Fetching and checking

- **One script** (`tools/check-vocab.sh`), beside `tools/check-vendor.sh`.
  It downloads each fetched row into a folder under `target/`, then runs
  the command. By default the command compares its output with the
  committed file and fails on a difference. With `--write` it writes the
  file; `npm run vocab` does that. With `-f` it downloads everything again.
- **Downloads** use the corpus scripts' `download` and `sha256_of`
  (`https` only, redirects included, a time limit per file). A file goes to
  a `.part` name and is renamed once its hash matches. A file already
  present with the right hash is not downloaded again, so a second run
  needs no network. A file present with the wrong hash is downloaded again.
- **A changed hash** fails the run and prints the row, the expected hash
  and the one found. The maintainer reads the publisher's change, copies
  the new hash into the row and regenerates.
- **Pins.** A row names an immutable URL where the publisher has one: a
  release, a tag, a commit or a versioned file. Otherwise it names the
  current file, and its hash is the signal that the publisher changed it.
  `tools/corpus/sources.tsv` follows the same rule.
- **CI.** The audit job runs the script after `tools/check-vendor.sh`. The
  downloads are cached under a key made from the manifest's hash, as the
  Tests job caches the corpus, so a pull request that does not change the
  manifest downloads nothing while the cache lasts. The nightly run passes
  `-f`, so a publisher's change of an unversioned file fails the nightly
  run first.

### The server

- **`rdf:_n`.** The vocabulary module's term lookup answers for a local
  name `_` followed by a positive integer without a leading zero, in the
  `rdf` namespace, with one shared term: a property, a subproperty of
  `rdfs:member`, with a one-line comment that cites RDF Schema 5.1.6. Every
  caller gets it through the lookup, so diagnostics, hover and semantic
  tokens agree and none of them changes. The list of terms, which
  completion and "did you mean" read, does not grow.
- **Hover** writes "From the X vocabulary" only when the lookup finds the
  term. Turtle, SPARQL and completion documentation share the one function
  that writes the line.
- **Aliases** are read from the `@ns` lines, as said above. The message of
  `unknown-term` keeps naming the vocabulary's own namespace.
- Nothing else in the server changes. The data stays embedded and each
  vocabulary is still parsed on first use.

### Delivery

1. **The quick step** uses the generator as it is. The generator gains the
   corrections step and EARL's namespace, and the list is regenerated from
   the same package. The server gains `rdf:_n` and the hover rule. This
   step can ship alone.
2. **The source step** adds the manifest, the frozen extracts and the
   script, rewrites the command over the library, regenerates the list from
   the pinned sources and removes the package. The corrections step the
   quick step added to the old generator is deleted with it.

GS1 and DPV change namespace in the source step, not in the quick step.
Moving a namespace before its data would turn "not checked" into about a
thousand false warnings for each of them.

### Budgets, pass or fail

1. `data/vocab.tsv`, gzipped at level 9, is at most 733,090 bytes, its size
   today. The generator fails over it.
2. `WASM_GZIP_BUDGET` stays 3,700,000 and `node web/build.mjs` passes.
3. `node bench/perf.mjs --only big-file-peak` passes.
4. The keystroke-latency rows of the benchmark do not move against `main`,
   measured on a quiet machine.
5. The root `package-lock.json` holds fewer than its 43 packages of today.
6. `tools/check-vocab.sh`, run twice, downloads nothing the second time and
   gives the same result.

What limits budget 1 is comment text: 1.66 MB of the file's 2.86 MB. The
refresh adds about 3,100 terms (15%), which at today's bytes per term is
about 0.5 MB, or 130 KB gzipped. The cap pays for it. Measured on today's
file:

| Cap, whole sentences | Gzipped | Comments cut, of 13,638 |
|---|---:|---:|
| 600 (today, cut anywhere) | 733 KB | 368 |
| 400 | 681 KB | 751 |
| 300 | 647 KB | 1,167 |
| 240 | 619 KB | 1,530 |
| 200 | 597 KB | 1,979 |
| 160 | 574 KB | 2,720 |

The source step picks the largest of these caps that meets budget 1 and
records it. The estimate says 240: 619 KB today, so about 730 KB with 15 to
18% more terms, where 300 would give about 760 KB. It is an estimate from
average bytes per term, and the source step measures the real file. At 240,
one comment in nine is cut. Without a cap the refreshed file would take
about 130 KB of the 300 KB the engine has left under its budget (it weighs
about 3.40 MB).

## Testing Decisions

A good test here gives the server a document and reads what a user would
see: the warnings, the hints, the hover. It does not read the list's lines
or the generator's variables.

- **Core terms** (new `tests/vocabulary.rs`). The test holds, as constants
  transcribed from the specifications and not from the bundled data, the
  terms of RDF 1.2 and RDF Schema (the namespace tables of RDF 1.2 Schema),
  OWL 2 (the mapping to RDF and the datatype map), XSD 1.1 (the built-in
  datatypes and the 14 facets), SHACL 1.0 (Core, SHACL-SPARQL and the
  SHACL-JS names) and SKOS. It writes one Turtle document that uses each
  and expects no `unknown-term`, through `batch::analyse`, as
  `tests/diagnostics_agree.rs` reads diagnostics.
- **The W3C suites** (same file). Every file under `tests/w3c` is analysed
  as `check` analyses it. Each `unknown-term` must match a listed entry
  with its reason, and each entry must match a warning, as
  `tests/jsonld_lints.rs` lists the JSON-LD corpus's lints. After the quick
  step the list has nine names, all mistakes or inventions of the suites:
  `rdfs:XMLLiteral`, `rdf:Graph`, `rdfs:definedBy`, `sh:nodeShape`,
  `dcterms:licence`, `dcterms:published`, `schema:term`, `schema::fish` and
  `og:audio%3Atitle`. The source step adds the three public-corpus files CI
  already downloads (`brick.ttl`, `dcat-ap-shacl.ttl`, `w3c-shacl.ttl`),
  each analysed alone, with the usual rule for an absent corpus.
- **The lookup** (unit tests in the vocabulary module, beside
  `https_schema_org_is_schema`): `rdf:_1` and `rdf:_12` are found, `rdf:_0`,
  `rdf:_01` and `rdf:_x` are not; an alias namespace resolves to its
  vocabulary; no `dbo` name is outside ASCII; `sh:js` is found.
- **Hover** (unit tests beside the existing ones in the hover module, and
  cases in `test/unknown-term.test.mjs`): the vocabulary line is there for
  `rdfs:label` and absent for `rdfs:lable`.
- **End to end** (`test/unknown-term.test.mjs`, which already covers
  `https://schema.org/` and GeoSPARQL functions): the file of issue #106
  gets no warning; `rdf:_1` gets none in Turtle and in SPARQL; EARL, GS1 in
  its three namespaces and DPV are checked; a superseded schema.org term
  and a deprecated QUDT unit get the hint.
- **The generator** (new `test/vocab-build.test.mjs`, on small sources with
  `ex:` names): one case per format, per deprecation marker and per failure
  listed above, the comment rule, the order, the aliases on the `@ns` line,
  and a second run that gives the same bytes. It runs offline on every
  platform, since `npm run test:node` picks it up.
- **The JSON-LD corpus** (`tests/jsonld_lints.rs`): its two entries marked
  "until the snapshot is refreshed" (GS1 and `dcat:next`) are removed by
  the source step.
- **The data** is tested by the script: CI fails when the committed list is
  not what the pinned sources and the corrections give.

Each ticket runs format, clippy and the test files it touches. The
close-out runs everything once.

## Out of Scope

- SHACL 1.2 terms. The drafts still change; `sh` moves when SHACL 1.2 is a
  Candidate Recommendation.
- New vocabularies: DPV's personal-data extension, QUDT's `datatype:` and
  `coords:` namespaces, terms for `geof` and `geor`, CIDOC CRM's property
  classes. Each is a manifest row later.
- The ActivityPub terms the Activity Streams context maps into `as:`, the
  `rdf:PropositionForm` names RDF 1.2 Schema reserves, and RDA's lexical
  aliases.
- A message that names a deprecated term's replacement. It needs a new
  column in the list.
- Stripping HTML and LaTeX from QUDT's comments.
- A setting to pick a vocabulary's version, and vocabularies loaded at run
  time.
- A compact binary format for the list.
- Reporting the upstream defects (`@vocabulary/gs1`'s namespace,
  `@vocabulary/dbo`'s source). Nothing is posted without the maintainer.

## Further Notes

### Sources

Fetched. The source step resolves each to an immutable URL where one
exists, and records the licence.

| Prefix | Publisher's file | Version | Format |
|---|---|---|---|
| `schema` | `https://schema.org/version/30.1/schemaorg-current-http.nt` | 30.1 | nt |
| `unit`, `constant`, `qkdv`, `prefix`, `sou` | one file each under `src/main/rdf/vocab/` at tag `v3.5.3` of `qudt/qudt-public-repo` (the release itself is one zip) | 3.5.3 | ttl |
| `quantitykind` | the quantity kinds file and the propulsion extension's, same tag | 3.5.3 | ttl |
| `qudt` | `SCHEMA_QUDT`, `SCHEMA_QUDT-DATATYPE` and `SCHEMA_QUDT-COORDINATES`, same tag | 3.5.3 | ttl |
| `dbo` | `dbo-snapshots.nt` of `dbpedia/ontology-tracker`, at its commit of 2026-07-22 | 2026.07.22 | nt |
| `gs1` | `https://ref.gs1.org/voc/data/gs1Voc.ttl`; namespace `https://ref.gs1.org/voc/`, aliases `https://gs1.org/voc/` and `https://www.gs1.org/voc/` | 1.18 | ttl |
| `dpv` | `2.3/dpv/dpv.ttl` of `w3c/dpv`; namespace `https://w3id.org/dpv#` | 2.3 | ttl |
| `rdau` | `nt/Elements/u.nt` of `RDARegistry/RDA-Vocabularies` at tag `v5.5.2` | 5.5.2 | nt |
| `rico` | `ontology/current-version/RiC-O_1-1.rdf` of `ICA-EGAD/RiC-O` at tag `v1.1` | 1.1 | rdfxml |
| `gn` | `https://www.geonames.org/ontology/ontology_v3.3.rdf`; alias `https://www.geonames.org/ontology#` | 3.3 | rdfxml |
| `doap` | `schema/doap.rdf` of `ewilderj/doap` at a commit (the publisher's own host has a wrong certificate; the files are identical) | 2022 | rdfxml |
| `geo` | `geosparql11/geo.ttl` at tag `1.1.0-ghpages` of the OGC GeoSPARQL repository | 1.1 | ttl |
| `og` | `https://ogp.me/ns/ogp.me.ttl` | current | ttl |
| `shex` | `https://www.w3.org/ns/shex.ttl` | current | ttl |
| `dcat` | `https://www.w3.org/ns/dcat.ttl` | DCAT 3 | ttl |
| `vcard` | `https://www.w3.org/2006/vcard/ns.ttl` | current | ttl |
| `sd` | the namespace document of `http://www.w3.org/ns/sparql-service-description#`, as Turtle | current | ttl |
| `as` | `https://www.w3.org/ns/activitystreams-owl` | current | ttl |

Frozen: `acl`, `bibo`, `cc`, `cnt`, `crm`, `csvw`, `dash`, `dc11`, `dcam`,
`dcmitype`, `dcterms`, `discipline`, `dqv`, `dtype`, `duv`, `earl`,
`ebucore`, `exif`, `foaf`, `frbr`, `gr`, `grddl`, `gtfs`, `http`, `hydra`,
`ical`, `ldp`, `locn`, `lvont`, `ma`, `mads`, `oa`, `org`, `owl`, `prov`,
`qb`, `rdf`, `rdfa`, `rdfs`, `rr`, `rss`, `sdmx`, `sem`, `sh`, `sioc`,
`skos`, `skosxl`, `sosa`, `ssn`, `test`, `time`, `vaem`, `vann`, `void`,
`vs`, `wgs`, `xhv`, `xkos`, `xsd`.

A fetched row that turns out not to be fetchable over `https`, or not to
parse, becomes a frozen row plus corrections, with the reason in the
tickets' README.

### Corrections

The quick step adds 27 lines, all for W3C vocabularies:

| Vocabulary | Terms | Defined in |
|---|---|---|
| `xsd` | `length`, `minLength`, `maxLength`, `pattern`, `enumeration`, `whiteSpace`, `maxInclusive`, `maxExclusive`, `minExclusive`, `minInclusive`, `totalDigits`, `fractionDigits`, `assertion`, `explicitTimezone` | XSD 1.1 Part 2, section 4.3; the facet IRIs by the rule of section 3. XSD has no RDF namespace document. |
| `owl` | `real`, `rational` | OWL 2 Structural Specification 4.1; the namespace file does not declare them. |
| `rdf` | `reifies`, `dirLangString`, `langRange` | RDF 1.2 Concepts and RDF 1.2 Schema; `langRange` in `rdf:PlainLiteral` and the OWL 2 datatype map. The namespace document is still RDF 1.1. |
| `rdfs` | `Proposition` | RDF 1.2 Schema 5.1, the range of `rdf:reifies`. |
| `time` | `unitCentury`, `unitDecade`, `unitMillenium` | OWL-Time, section 4; the namespace document has seven of the ten units. |
| `as` | `closed` | Activity Streams 2.0 Vocabulary; absent from the W3C OWL file. |
| `xhv` | `form`, `rowgroup`, `scrollbar` | the XHTML vocabulary page, as RDFa. |

The source step adds `mads:Medium` (MADS/RDF 1.5.0) and `dtype:dateUnion`,
`dtype:defaultObjectValue` and `dtype:objectValue` (the 2023 file), whose
vocabularies stay frozen.

### What the source step is expected to change

From the audit, by the same rule for what a term is. A count that differs
is explained in the tickets' README.

| Vocabulary | Today | After | Added | Removed | Deprecated after |
|---|---:|---:|---:|---:|---:|
| `schema` | 2,801 | 3,023 | 225 | 3 | 95 |
| `unit` | 1,747 | 2,940 | 1,394 | 201 | 85 |
| `quantitykind` | 881 | 1,262 | 410 | 29 | 98 |
| `qudt` | 933 | 564 | 69 | 438 | |
| `constant` | 656 | 662 | 12 | 6 | |
| `qkdv` | 213 | 273 | 78 | 18 | |
| `prefix` | 29 | 33 | 4 | 0 | |
| `sou` | 18 | 10 | 1 | 9 | |
| `dbo` | 4,258 | 3,681 | 35 | 612 | |
| `gs1` | 1,371 | 2,539 | 1,185 | 17 | 87 |
| `dpv` | 351 | 1,174 | 1,028 | 205 | |
| `rdau` | 1,141 | 1,214 | 73 | 0 | 129 |
| `rico` | 593 | 664 | 268 | 197 | |
| `gn` | 733 | 735 | 4 | 2 | |
| `geo` | 39 | 65 | 26 | 0 | |
| `dcat` | 36 | 52 | 16 | 0 | |
| `shex` | 68 | 79 | 11 | 0 | |
| `vcard` | 146 | 152 | 6 | 0 | |
| `doap` | 53 | 56 | 3 | 0 | |
| `sd` | 38 | 41 | 3 | 0 | |
| `as` | 123 | 125 | 2 | 0 | 11 |
| `og` | 35 | 36 | 1 | 0 | |

The removed terms are ones their publishers removed: QUDT's purge of
deprecated terms in 3.0.0 and its move of datatype instances to other
namespaces, DBpedia's 34 withdrawn terms and the 578 junk names, RiC-O's
renames in 1.0, the personal-data categories DPV moved to an extension.
Reporting their use is correct. The 31 propulsion quantity kinds QUDT moved
to a community extension in the same namespace are kept, which is why
`quantitykind` reads two files.

Only 74 terms carry the deprecated flag today, in eight vocabularies, and
none in schema.org, GS1, RDA or Activity Streams.

### Where the numbers come from

- The audit: three reports dated 2026-11-12, comparing the list with each
  publisher's current file and with the current `@vocabulary` packages.
  Their findings are in issue #106 and in the tables above.
- The suite counts: `turtle-lsp check --select unknown-term` on `tests/w3c`
  with the list as it is, 211 warnings in 124 files.
- The sizes: `data/vocab.tsv` is 2,858,818 bytes, 733,090 gzipped at level
  9. The cap table applies each cap to the comments of today's file.
- The lock file: 43 packages, 39 of them reachable from
  `@zazuko/rdf-vocabularies`, `n3` among them.

## Principles

- redesign-from-first-principles: the list had one upstream by assumption. With "current and corrected" as a requirement, every vocabulary gets a declared source, and a frozen extract is one kind of source, not an exception in code.
- subtract-before-you-add: the package, the regular-expression parser, the `ALIASES` constant and the two empty vocabularies go. The corrections file has one action because the sources make `drop` unnecessary.
- migrate-callers-then-delete-legacy-apis: the source step moves `npm run vocab` to the new command and deletes the old generator, its corrections step and the dependency in the same ticket.
- outcome-oriented-execution: two steps with a stated end. The quick step may duplicate the corrections reader for one wave; the close-out verifies the whole.
- foundational-thinking, model-the-domain: the manifest row, the correction line and the marker table are settled in this spec before any code. A source's `format` decides how it is read; nothing else branches on the vocabulary.
- boundary-discipline: the script and the command are the boundary (network, files, checksums). The library is a pure function from bytes to text, and the tests call it.
- experience-first: the false warnings go first. The comment rule ends at a sentence or says it was cut. Hover stops claiming a vocabulary for a term it does not have.
- make-operations-idempotent: a second run of the script downloads nothing and writes the same bytes; a download interrupted halfway leaves only a `.part` file; the order does not depend on the machine.
- separate-before-serializing-shared-state: the first wave's two tickets share no file. The generated list is written by one ticket at a time and is regenerated, never merged.
- fix-root-causes: the `dbo` junk and GS1's empty result are fixed by the source, EARL by its namespace, `rdf:_n` in the lookup every caller uses, and the hover line where it is written.
- explain-the-number: the size budget names what limits it (comment text) and the measurement behind each cap.
- build-the-lever, encode-lessons-in-structure: the script is the check a reviewer reruns. A source with no term, a stale correction and a list over budget fail the build, and the suites test lists every remaining warning with its reason.
