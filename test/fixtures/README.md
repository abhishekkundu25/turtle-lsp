# Test fixtures

- `workspace/`: small hand-written files for navigation tests.
- `project/`: headerless Turtle fragments sharing one `prefixes.ttl`.
- `datasets/`: a headerless TriG file (`graphs.trig`) and a Turtle vocabulary
  sharing `prefixes.ttl`, and an N-Triples file (`people.nt`) that spells
  every IRI in full. Used by `test/trig.test.mjs`, the editor suites and the
  oxttl oracle.
- `oracle/`: every TriG graph-block form and N-Triples term form, compared
  with oxttl's parsers in `tests/oxttl_oracle.rs`.
- `real/` (local-only, gitignored): Turtle fragments copied from the private `ontology-design` repo
  (`design/guide/examples`, `design/simulation/library`, `design/doc/code`,
  `prototype/live-designer/server`), file paths flattened with `_`.
  They have no `@prefix` lines; `real/prefixes.ttl` is the shared header that
  `ontology-design/design/guide/check_examples.py` prepends before parsing.
