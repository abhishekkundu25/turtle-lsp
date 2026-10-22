# 25: Test depth

**What to fix:** gaps the coverage map found, and tests that reach past the seam they
claim to test.

- `Workspace::change_doc` (incremental sync, advertised as `INCREMENTAL`) has no test at
  any seam CI runs; the Node client sends full text (`src/workspace.rs:463-493`,
  `test/lsp-client.mjs:215-222`).
- The W3C SPARQL harness is syntax-only; evaluation tests are parsed, never evaluated;
  `query::run` and the patched spareval have no conformance oracle
  (`tests/w3c_sparql.rs:139-145, 229-291`).
- `completionItem/resolve` (Turtle and SPARQL) and Turtle `prepareRename` have no
  assertion in any test CI runs.
- Wall-clock assertions flake under load (history: 303b9c9, 8fe49bd, daf8df2, 2c8185c):
  `query-lsp.test.mjs:166-197`, `shacl-validate.test.mjs:200`,
  `workspace-diagnostics.test.mjs:264-265` (asserts a race), `lsp.test.mjs:39`,
  `lens.test.mjs:186`, `unknown-term.test.mjs:48`, `sparql-format.test.mjs:107`,
  `sparql-completion.test.mjs:155`, `src/query/tests.rs:247-389`.
- `tests/w3c_shacl.rs` normalises expected paths with the code under test
  (`parse_path`) and matches the allow-list by substring; `w3c_conformance.rs`
  re-implements the undeclared-prefix rule; the oxttl oracle and `examples/corpus.rs`
  compare ground triples only; the JSON-LD `KNOWN` list accepts any difference with the
  same counts; two tests replace the global panic hook in a parallel test binary.
- The VS Code suite writes `__vscode-shacl.ttl` into tracked fixtures that the Node
  tests and chaos read.

**Files owned:** `src/workspace.rs` (`#[cfg(test)]` module only), new
`tests/w3c_sparql_eval.rs` + vendored `tests/w3c/sparql-tests/sparql11/{aggregates,bind,
construct,exists,grouping,negation,optional,property-path,subquery,functions}` with
`.srx/.srj/.ttl` results + a status list + `tests/w3c/README.md`, `tests/w3c_shacl.rs`,
`tests/w3c_conformance.rs`, `tests/oxttl_oracle.rs`, `tests/jsonld_oracle.rs`, new
`tests/support/oracle.rs`, `examples/corpus.rs` (`compare`), `src/query/tests.rs` (tests
only), `src/server.rs` (the two panic-hook tests only), `test/{workspace-diagnostics,
query-lsp,shacl-validate,lsp,lens,unknown-term,sparql-format,sparql-completion}.test.mjs`
(timing → counted/ordered, or moved to a serial `test/latency.test.mjs`),
`test/lsp-client.mjs` (`changeRange` helper), new `test/resolve-rename.test.mjs`,
`editors/vscode/test/run.mjs` (temp fixture copy).
**Hands off:** `src/**` beyond the test modules named.

**Blocked by:** tickets 04 (`src/query/tests.rs`), 20 (`workspace.rs` lock #4), 11
(`lsp-client.mjs`).

**Status:** ready-for-agent

- [ ] `change_doc` table test: insert, delete, replace, multi-change batch in order,
      UTF-16 after an astral char, past EOF, reversed range; e2e: diagnostics after
      ranged edits equal diagnostics after the equivalent full-text change.
- [ ] `tests/w3c_sparql_eval.rs`: default-graph-only `QueryEvaluationTest`s through
      `query::run`, results compared order-insensitively (unless `ORDER BY`) with an
      oxrdf isomorphism check; a status list that fails on regression and on a known-fail
      that passes; README conformance row for ticket 22.
- [ ] e2e: resolve a Turtle and a SPARQL completion item (`documentation`/`detail`);
      Turtle `prepareRename` on a prefixed name, a full IRI, a keyword (refused), a
      literal (refused).
- [ ] Timing asserts replaced by counted or ordered checks where the property allows;
      the rest in one serial latency file run with `--test-concurrency=1`; the
      `before < 299` race made deterministic. Each touched file passes 5× in a row under
      `npm test` load.
- [ ] SHACL harness reads expected paths with an independent reader and matches the
      allow-list by equality; conformance negatives go through
      `features::diagnostics::diagnostics` (or a public `syntax` rule); oracles compare
      canonicalised datasets via `tests/support/oracle.rs` (also used by
      `examples/corpus.rs`); `KNOWN` entries carry a hash of the symmetric difference;
      panic-hook tests restore via a guard under a `Mutex`; SHACL counts to stderr.
- [ ] VS Code suite runs on a temp copy of `test/fixtures`; no write under
      `test/fixtures` during `npm run test:vscode`.
- [ ] Counts unchanged (RDF 1137/1137, SPARQL 425/426, SHACL 0 failures); `cargo test
      --release`, `npm run test:node` green.
