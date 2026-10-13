# SHACL engine: evaluation and decision

Research date: 2026-10-13. Time-boxed evaluation for ticket 14 (SHACL validation on save).

## Decision

**Implement SHACL Core ourselves** (`src/shacl/graph.rs`, `src/shacl/validate.rs`, `src/shacl/literal.rs`), on the triples the parser already produced. All three candidates pull in a full SPARQL engine, an async runtime or both, and none takes our triples without copying them into its own store.

The only new crate is `regex` 1.13.1 (MIT OR Apache-2.0), used for `sh:pattern`. It matches in linear time, so a pathological pattern in a shapes file cannot hang the validation thread.

| | Before | After |
|---|---|---|
| W3C SHACL Core suite (`tests/w3c/shacl/core`, 98 tests) | – | **98 / 98 pass**, no allow-list |
| Clean release build (12 cores, other builds running) | 8 s | 9 s |
| Release binary | 6,575,264 bytes | 7,842,544 bytes (+1.27 MB) |
| Crates in the build (normal dependencies) | – | +3 (`regex`, `regex-automata`, `regex-syntax`) |

Of the +1.27 MB, `regex` with only the `std` feature is about 0.79 MB. The Unicode features (`unicode-perl`, `unicode-case`, `unicode-gencat`) add 0.48 MB. They are kept because XPath regular expressions are Unicode-aware: `\w`, `\d`, `\p{L}`, and the `i` flag.

## Candidates

Each candidate was added to an empty crate on crates.io's latest version, and its dependency tree was counted with `cargo tree -e normal`. The tree was then built in release mode with a cold target directory.

| Crate | Version | Licence | Unique crates | Heavy dependencies | Release build of the dependencies | Input graph |
|---|---|---|---|---|---|---|
| rudof [`shacl`](https://crates.io/crates/shacl) (+ `shacl_validation`, `shacl_ast`, `srdf`) | 0.3.25 | MIT OR Apache-2.0 | 236 (228 with `default-features = false`) | `oxigraph` 0.5, `spargebra`, `sparesults`, `tokio`, `reqwest`, `hyper` (still present without the `sparql` feature) | 37 s, 536 MB target dir | Generic over its `NeighsRDF` trait; in practice an oxrdf-based graph |
| [`oxirs-shacl`](https://crates.io/crates/oxirs-shacl) | 0.4.1 | Apache-2.0 | 397 (396 with only `core`) | `oxirs-core`, `scirs2-core`, `ndarray`, `tokio`, `reqwest` | not built (dependency count already disqualifying) | `oxirs-core` store; 135k lines of source |
| [`purrdf-shapes`](https://crates.io/crates/purrdf-shapes) | 3.0.1 | MIT OR Apache-2.0 OR MulanPSL-2.0 | 56 | 17 `purrdf-*` crates, including `purrdf-sparql-eval`, `purrdf-sparql-algebra`, `purrdf-datalog`, plus `aes-gcm`, `ed25519`, `sha2` and a zstd decoder | 60 s, 279 MB target dir | Its own `RdfDataset` with interned term ids; 124k lines in the shapes crate alone |

Against the spec's criteria:

- **Licence.** All three pass `deny.toml`: purrdf's `OR` includes MIT, and oxirs is Apache-2.0.
- **Dependency weight and build time.** All fail. Our whole server builds in about 8 s. The lightest candidate by crate count (purrdf-shapes) takes 60 s just for its dependencies. rudof's `shacl` brings an async HTTP stack and Oxigraph even without SPARQL.
- **Our triples without an extra parse.** No candidate reads `crate::syntax::Triple`. rudof could in principle accept them through its graph trait, but at the cost of the dependencies above. purrdf and oxirs need every term copied into their own store.
- **Result fields.** All three report focus node, path, value, source shape, component and severity. None reports where a result came from in the source text, so we would still map results back to spans ourselves.
- **W3C SHACL Core pass rate.** Not measured for the candidates: they were rejected on weight first. purrdf's crate description says it is "gated against the SHACL conformance corpus". rudof runs the suite as a git submodule in its own CI.

## What our engine does

- **Graph.** The union of every workspace and external source. Blank nodes are numbered per document, so `_:b` in two files never merges. Every distinct term gets a dense id. Literals are borrowed from the parse, not copied. Triples are indexed by subject and object (compressed rows) and by predicate.
- **Targets.** `sh:targetNode`, `sh:targetClass` (with the `rdfs:subClassOf` closure), `sh:targetSubjectsOf`, `sh:targetObjectsOf`, and implicit class targets. A shape typed `rdfs:Class`, or `owl:Class` as an extension, is its own target.
- **Paths.** Predicate, inverse, sequence, alternative, zero-or-more, one-or-more and zero-or-one, evaluated in both directions with set semantics.
- **Components.** All SHACL Core components: class, datatype (with lexical checks for the XSD numeric, boolean and date/time types), nodeKind, min/maxCount, min/max Exclusive/Inclusive, min/maxLength, pattern with flags, languageIn, uniqueLang, equals, disjoint, lessThan, lessThanOrEquals, not, and, or, xone, node, property, qualifiedValueShape with min/max counts and `sh:qualifiedValueShapesDisjoint`, closed with ignoredProperties, hasValue and in. Also `sh:deactivated`, `sh:severity` and `sh:message`.
- **Literal order** (`src/shacl/literal.rs`):
  - Numbers compare by value, as `i128` when both are integers and as `f64` otherwise. Decimals beyond about 15 significant digits can therefore compare as equal.
  - `xsd:dateTime` and `xsd:date` compare only when both values have a timezone or neither has one. XSD leaves a mixed comparison indeterminate.
  - Strings compare by code point, and booleans with false before true.
  - Everything else, including ill-formed literals, is not comparable, so the constraint fails.
- **Choices where the spec leaves room.**
  - A recursive shape reference, or nesting deeper than 64 levels, conforms.
  - A shape with an ill-formed `sh:path` is skipped.
  - Only the literal `true` turns on boolean parameters, as the suite requires (`uniqueLang-002`).
  - A `sh:pattern` that `regex` cannot compile, such as XPath character-class subtraction, is ignored.

## Performance

- The bench workspace has about 180k triples and one shape with a class constraint and a pattern over 45,000 `foaf:Person` nodes. From `didSave` to SHACL diagnostics takes p50 15 ms.
- Keystroke-to-diagnostics latency measured while validations run is p95 4.9 ms (`npm run bench`).
- The worker polls a generation counter between focus nodes, so a newer save stops a running validation early.
