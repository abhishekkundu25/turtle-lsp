# W3C test suites (vendored)

Copied without the generated reports so the conformance tests run offline.

| Suite | Source | Commit |
|---|---|---|
| `rdf-tests/` (Turtle, TriG, N-Triples, N-Quads; RDF 1.1 and 1.2) | https://github.com/w3c/rdf-tests | `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0` |
| `rdf-tests/rdf11/rdf-xml/` (RDF/XML 1.1; without the report generator `convert-manifest.rb` and `template.haml` and the archives `TESTS.zip` and `TESTS.tar.gz`) | https://github.com/w3c/rdf-tests (`rdf/rdf11/rdf-xml`) | `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0` |
| `sparql-tests/` (SPARQL 1.1 `syntax-query`, `syntax-update-1`, `syntax-update-2`, `syntax-fed`; SPARQL 1.2 without reports) | https://github.com/w3c/rdf-tests (`sparql/`) | `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0` |
| `sparql-tests/sparql11/{aggregates,bind,construct,exists,functions,grouping,negation,property-path,subquery}` (SPARQL 1.1 query evaluation: `.rq`, `.ttl`, `.srx`, `.srj`, manifests and one empty `.nt`; 510 files, 275 KB) | https://github.com/w3c/rdf-tests (`sparql/sparql11/`) | `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0` |
| `shacl/core/` (SHACL Core) | https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/core`) | `8c9d0f6a20667fbfc24bbcd18e919da6b5ffac53` |
| `shacl/sparql/` (SHACL-SPARQL) | https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/sparql`) | `8c9d0f6a20667fbfc24bbcd18e919da6b5ffac53` |
| `owl2/` (OWL 2 Functional-Style Syntax: the 78 Functional texts of the approved OWL 2 test cases, one file each, the 57 RDF/XML texts of the same ontologies, and their `MANIFEST.tsv`) | https://www.w3.org/2009/11/owl-test/all.rdf (the W3C archive of the OWL 2 Test Case Repository) | `Last-Modified` 2009-11-18, sha256 `5383f1ddf4cf2f03703a2f886f41d4e5bc375633a1cfa94a03254fd89330f8bb` |
| `owl2-primer/` (OWL 2 XML Serialization: the OWL/XML and Functional texts of the 59 examples of the OWL 2 Primer, the RDF/XML and Turtle texts of its appendix ontology, 120 files, and their `MANIFEST.tsv`) | https://www.w3.org/TR/2012/REC-owl2-primer-20121211/ (W3C Recommendation 11 December 2012) | fetched 2026-11-23, sha256 `e64c94d2900e24142940cd45126651082f8e9a23e23396899f39ce4be9543630` |

Each directory keeps its upstream licence (`LICENSE.md`); `owl2/` is
CC BY 3.0, with its authors named per file; `owl2-primer/` is under the
W3C Document License, and its `LICENSE.md` says how the texts were taken
from the HTML.

## OWL 2 Functional-Style Syntax

The parser is private to the crate, so its runner is a unit test
(`w3c_owl2_functional_inputs_parse` in `src/ofn/tests.rs`). The archive
has no syntax tests: every one of its tests is about entailment,
consistency or profiles, and each text in it is a valid document. Of its
489 tests, 83 have a Functional text; the 60 of them that are approved give
the 78 files here (60 premises, 16 conclusions, 2 non-conclusions). The
test passes when each parses with no error. Result: 78 of 78.

The mapping to RDF is checked against the 57 of them whose test also has
an RDF/XML text of the same ontology (`owl2/rdfxml/`, the rows flagged
`also-rdfxml-text`), read by the RDF/XML frontend
(`w3c_functional_and_rdfxml_texts_give_one_graph` in
`tests/ofn_oracle.rs`): the two graphs must be isomorphic. Result: 51 of
57; the other 6 are texts that are not the same ontology (an extra or a
missing triple, a `differentFrom` pair the other way, a list ended with
the `rdf:` namespace IRI instead of `rdf:nil`), each listed with the edit
that makes them agree, which the test checks.

## OWL 2 XML Serialization (OWL/XML)

The test-case archive of `owl2/` has no OWL/XML text. It holds 489 test
cases and 811 ontology texts: 103 Functional (`test:fsPremiseOntology`,
`test:fsConclusionOntology`, `test:fsNonConclusionOntology`) and 708
RDF/XML (`test:rdfXmlPremiseOntology`, `test:rdfXmlConclusionOntology`,
`test:rdfXmlNonConclusionOntology`, `test:rdfXmlInputOntology`); no
property holds any other syntax, no `test:normativeSyntax` is other than
`FUNCTIONAL` or `RDFXML`, and the string "OWL/XML" does not occur in it.
The W3C OWL/XML texts are those of the OWL 2 Primer, in `owl2-primer/`.

`tests/owx_oracle.rs` reads them through `Analysis`. Two things are done to
the texts in memory, the files staying verbatim: each U+00A0 in a
Functional or Turtle text (357 and 103 of them, each before a `:` or `;`,
where the HTML writes a space as `&nbsp;`) is read as a space; and each
known difference is corrected by its edit (`KNOWN`, 35 edits, each with the
sentence of the Primer or of the specifications that decides it). 32 of
them are faults of the texts; 3 are not (see 59 below). The test
checks that each edit applies once and is needed: with any one left out,
the texts do not agree.

- `the_primer_s_examples_agree_between_owl_xml_and_functional`: each
  example's OWL/XML text and Functional text, put inside the header of the
  appendix ontology in their syntax (its prefixes, `xml:base` and
  entities; a prefix the example declares itself left out of the header,
  as in example 52), or read as they are for the three whole documents
  (50, 51, 59), parse with no error to isomorphic graphs. Result: 51 of
  59 as they are (50, and 52 with that prefix left out); the other 8 with
  their edits. 22: `Happy` for
  `HappyPerson` in the OWL/XML. 49: quotes inside the OWL/XML `Literal`.
  50 and 51: `...` for the elided parts, in both texts. 56, 57, 58:
  `DisjointClasses` with Father before Mother in the OWL/XML, a different
  `owl:members` list. 59: the same two OWL/XML faults, a declaration of
  `Meg` only in the OWL/XML, and axioms written twice (a property chain in
  the OWL/XML, an `EquivalentClasses` in both). An axiom written twice is
  not a fault of the text: it is one axiom of the ontology, but
  turtle-lsp maps each axiom as written, in Functional syntax as in
  OWL/XML, so its graph has two blank-node copies. Whether the mapping
  should merge them is open.
- `the_primer_s_ontology_is_one_graph_in_four_syntaxes`: the appendix
  ontology in OWL/XML, Functional syntax, RDF/XML and Turtle parses with
  no error once edited (the Turtle text as given does not parse: a `.`
  inside `[ ]`). The OWL/XML, Functional and RDF/XML texts give one graph
  of 315 triples, the RDF/XML with 14 edits (19 declarations it lacks, 2
  it has, `owl:DataProperty`, two typed inverse properties, six literals
  with the element's white space, two classes as `owl:AllDisjointClasses`
  instead of `owl:disjointWith`). The Turtle is not the same ontology: it
  leaves out 61 of the graph's statements (40 declarations, 21 axioms).
  With 6 edits, everything it says is in the graph, and what it leaves out
  is pinned (count and fingerprint). The RDF/XML and Turtle graphs are also
  those of oxrdfxml and oxttl.
- `the_primer_s_manifest_names_every_file`: `MANIFEST.tsv` and the files
  agree.

## RDF/XML 1.1

The parser is private to the crate, so its runner is a unit test beside it
(`rdfxml_w3c_suite` in `src/rdfxml/grammar.rs`), and `tests/w3c_conformance.rs`
skips the directory. An evaluation test passes when the graph is isomorphic
to the expected N-Triples; a negative syntax test when at least one error is
reported. The manifest's commented-out entries are not run. Result at the
commit above: 166 of 166 pass.

## SPARQL 1.1 query evaluation

`tests/w3c_sparql_eval.rs` runs the `mf:QueryEvaluationTest`s of the nine
evaluation directories above through `turtle_lsp::query::run` (the engine of
`turtle-lsp query` and "Run query") and compares the answers with the
expected results: SELECT results as RDF graphs checked for isomorphism (row
order and blank node labels do not count; with `ORDER BY`, the order of the
leading plain-variable keys does), ASK as the boolean, CONSTRUCT as an
isomorphic graph. Numbers compare by value within their datatype (`3.21E4`
and `32100` are one `xsd:double`).

Only the files those tests read were copied (and `functions/data-empty.nt`,
an empty N-Triples data file four tests read): the RDF/XML data files
(`.rdf`) were left out, so the four tests that read them are out of scope.
Also out of scope: tests with named graphs (`qt:graphData`) and queries with
`FROM`, since `query::run` has no named graphs and ignores `FROM`. There is
no `sparql11/optional` directory upstream (OPTIONAL is tested in the SPARQL
1.0 suites, not vendored).

`sparql-tests/sparql11/eval-status.tsv` gives every test a status: `pass`,
`known-fail` or `out-of-scope`, with a reason. The run fails when a pass
regresses, a known failure starts passing, or the list and the suites
disagree. To rewrite the list from the actual results (the reasons of known
failures that still fail are kept):

```sh
SPARQL_W3C_WRITE=1 cargo test --release --test w3c_sparql_eval
```

Result at the commit above: 176 of the 183 tests in scope pass; 7 known
failures (all in spareval 0.2.7: `GROUP_CONCAT` keeps a shared language tag
(2), `BNODE(str)` reuses a blank node across solutions (1), a zero-length
path from or to a constant not in the data matches nothing (4)); 21 out of
scope. `tests/w3c_sparql.rs` skips these directories, so its count stays
that of the syntax suites.

## JSON-LD 1.1

| Suite | Source | Commit |
|---|---|---|
| `jsonld/` (JSON-LD 1.1 API: `toRdf`, `expand`, `remote-doc` manifests and their files) | https://github.com/w3c/json-ld-api (`tests/`) | `55514738d854e56bb8bad4c53cda2f0e23be4a50` |

`tests/w3c_jsonld.rs` runs them offline: a loader serves each file under its
`https://w3c.github.io/json-ld-api/tests/` URL and follows a test's
`redirectTo`. Expand and remote-doc tests compare RDF, because the engine has
no expanded-JSON output: the input's quads must match the expected expanded
document read by an expanded-form oracle in the test, which is checked against
the toRdf suite's results.

`jsonld/expected-status.tsv` gives every test a status: `pass`, `known-fail`
or `out-of-scope`, with a reason. The run fails when a pass regresses, a
known-fail starts passing, or the list disagrees with the suites. To rewrite
the list from the actual results:

```sh
JSONLD_W3C_WRITE=1 cargo test --release --test w3c_jsonld
```

Out of scope, and not vendored: the compact, flatten, frame (in
w3c/json-ld-framing), fromRdf and html manifests. Out of scope inside the
vendored manifests, with the reason in the status list: JSON-LD 1.0 tests
(`specVersion` or `processingMode` `json-ld-1.0`), `expandContext`,
`produceGeneralizedRdf`, `rdfDirection`, and the remote-doc tests about
document loading (HTTP `Link` headers, content types, missing documents, HTML).
