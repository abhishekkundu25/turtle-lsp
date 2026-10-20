# W3C test suites (vendored)

Copied without the generated reports so the conformance tests run offline.

| Suite | Source | Commit |
|---|---|---|
| `rdf-tests/` (Turtle, TriG, N-Triples, N-Quads; RDF 1.1 and 1.2) | https://github.com/w3c/rdf-tests | `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0` |
| `sparql-tests/` (SPARQL 1.1 `syntax-query`, `syntax-update-1`, `syntax-update-2`, `syntax-fed`; SPARQL 1.2 without reports) | https://github.com/w3c/rdf-tests (`sparql/`) | `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0` |
| `shacl/core/` (SHACL Core) | https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/core`) | `8c9d0f6a20667fbfc24bbcd18e919da6b5ffac53` |
| `shacl/sparql/` (SHACL-SPARQL) | https://github.com/w3c/data-shapes (`data-shapes-test-suite/tests/sparql`) | `8c9d0f6a20667fbfc24bbcd18e919da6b5ffac53` |

Each directory keeps its upstream licence (`LICENSE.md`).

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
