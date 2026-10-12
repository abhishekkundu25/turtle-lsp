# Test fixtures

- `workspace/`: small hand-written files for navigation tests.
- `real/` (local-only, gitignored): Turtle fragments copied from the private `ontology-design` repo
  (`design/guide/examples`, `design/simulation/library`, `design/doc/code`,
  `prototype/live-designer/server`), file paths flattened with `_`.
  They have no `@prefix` lines; `real/prefixes.ttl` is the shared header that
  `ontology-design/design/guide/check_examples.py` prepends before parsing.
