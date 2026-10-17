# Test fixtures

- `workspace/`: small hand-written files for navigation tests.
- `project/`: headerless Turtle fragments sharing one `prefixes.ttl`.
- `datasets/`: a headerless TriG file (`graphs.trig`) and a Turtle vocabulary
  sharing `prefixes.ttl`, and an N-Triples file (`people.nt`) that spells
  every IRI in full. Used by `test/trig.test.mjs`, the editor suites and the
  oxttl oracle.
- `oracle/`: every TriG graph-block form and N-Triples term form, compared
  with oxttl's parsers in `tests/oxttl_oracle.rs`.
- `real/` (local-only, gitignored): Turtle fragments copied from a private
  design repository, file paths flattened with `_`. They have no `@prefix`
  lines; `real/prefixes.ttl` is the shared header that repository prepends
  before parsing.
- `imports/`: `owl:imports` resolution (test/imports.test.mjs). `ws/` is the
  workspace: `main.ttl` imports through `ws/catalog-v001.xml`, which maps to
  Turtle files in `ws/vendor/` (importing each other in a cycle) and to
  RDF/XML in `ws/vendor/gear.rdf`. Local imports outside the workspace are
  never read.
