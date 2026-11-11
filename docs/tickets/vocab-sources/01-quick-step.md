# 01: The quick step: core W3C terms, `rdf:_n`, EARL and the hover line

**What to build:** the false warnings of issue #106 go away, with the
generator as it is. A file that uses the XSD facets, `owl:real`,
`owl:rational`, `rdf:reifies`, `rdf:dirLangString`, `rdf:langRange` or a
container membership property (`rdf:_1`, `rdf:_2` and so on) gets no
`unknown-term`, in Turtle, SPARQL and JSON-LD. EARL terms are checked in
`http://www.w3.org/ns/earl#`. Hover writes "From the X vocabulary" only for
a term the list has. A typo keeps its warning and its fix.

The 27 added terms come from the new corrections file, which the generator
applies after it has read the package (spec, "The inputs" and
"Corrections"). Each line gives its reason, and its label and comment come
from the specification it names. EARL's namespace is set in the generator.
`rdf:_n` is answered by the vocabulary module's term lookup, so no caller
changes, and the list of terms does not grow (spec, "The server"). GS1 and
DPV stay as they are until 03. This ticket can be released alone.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] The file in issue #106 gets no `unknown-term`. `rdf:_1` and `rdf:_12` get none in Turtle and in SPARQL; `rdf:_0`, `rdf:_01` and `rdf:_x` keep theirs. `earl:Assertion` in the `http` namespace is known and `earl:Asertion` is reported with its fix. Cases in `test/unknown-term.test.mjs`.
- [ ] Hover on `rdfs:label` keeps "From the rdfs vocabulary"; hover on `rdfs:lable` has no vocabulary line; hover on `rdf:_3` says it is a property and cites RDF Schema. Unit tests in the hover module, and one case in `test/unknown-term.test.mjs`.
- [ ] Unit tests in the vocabulary module for the lookup: `_1` and `_12` found in `rdf`, `_0`, `_01`, `_x` and `_` not found, and `_1` not found in any other vocabulary.
- [ ] `data/vocab-patches.tsv` holds the 27 lines of the spec's table, in the spec's column order. The generator stops with the line named when a correction names an unknown prefix or a term the package already has. Show each once by hand and say so in the report.
- [ ] `npm run vocab` reproduces the committed list: after it, `git diff --exit-code data/vocab.tsv` is clean. Against the list before this ticket, the diff is the 27 added `term` lines and EARL's `@ns` line, nothing else (`git diff --numstat`: 28 added, 1 removed).
- [ ] New `tests/vocabulary.rs`, first test: every core W3C term is known. The terms are constants in the test, transcribed from the specifications named in the spec's "Testing Decisions", not copied from `data/vocab.tsv`. One Turtle document uses each, and `batch::analyse` reports no `unknown-term`.
- [ ] `tests/vocabulary.rs`, second test: the files under `tests/w3c`, analysed as one workspace as `check --root tests/w3c` sees them, get only the listed `unknown-term` warnings. Every entry has its reason and must match a warning, as in `tests/jsonld_lints.rs`. Today `check` gives 211 warnings there. After this ticket it gives 51, under nine names: `rdfs:XMLLiteral`, `rdf:Graph`, `rdfs:definedBy`, `sh:nodeShape`, `dcterms:licence`, `dcterms:published`, `schema:term`, `schema::fish` and `og:audio%3Atitle`.
- [ ] `cargo fmt --check` and `cargo clippy --release --locked --all-targets -- -D warnings` pass.
- [ ] After `cargo build --release --locked` and `cargo test --release --locked --lib --test vocabulary --no-run`, these pass:
  - `timeout 120 cargo test --release --locked --lib -- vocab:: features::hover:: features::diagnostics:: features::completion:: features::semantic:: sparql::features:: --skip timing_`
  - `timeout 120 cargo test --release --locked --test vocabulary`
  - `timeout 120 node --test --test-timeout=30000 test/unknown-term.test.mjs`
- [ ] The report lists what was not run (everything else is 04's) and anything skipped.
