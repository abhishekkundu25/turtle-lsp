# 11: Test harness

**What to fix:** the test infrastructure hides failures and misses input families.

- `npm run test:node` runs two reporters with `--test-force-exit`, so the pending
  reporter's summary ("pending tests that pass, to promote") never prints; a todo that
  starts passing goes unnoticed (`package.json:7`, `test/pending-reporter.mjs`).
  Verified: the summary prints without `--test-force-exit`.
- `tests/chaos.rs` reads its Turtle corpus with `read_dir` unsorted (the SPARQL half
  sorts), so a CI seed may not reproduce locally; failures are dumped to
  `std::env::temp_dir()` (CI discards it) for Turtle and `CARGO_TARGET_TMPDIR` for SPARQL.
  It never feeds JSON-LD, embedded SPARQL in SHACL (`sh:select`), or any SPARQL feature
  beyond parse/format; its mutations never reach deep nesting, long lines, lone CR or
  huge tokens (`chaos.rs:114-149, 181-208, 220-371, 497-503, 613-714`). The span check
  omits several span fields.
- Nothing tests the protocol error paths: unknown method, wrong params, request before
  `initialize`, bad `Content-Length`, invalid JSON/UTF-8, huge length
  (`src/server.rs:1369, 1794-1797`).
- `test/lsp-client.mjs` cleans up only on `exit` (not SIGINT/SIGTERM), leaving
  `turtle-lsp-caches-*` dirs. `test/nvim.test.mjs` ignores `run.status`/`signal` and never
  checks the plan count.

**Files owned:** `tests/chaos.rs`, new `test/fixtures/chaos/` (JSON-LD and `sh:select`
corpus files), `package.json` (`test:node` script only), `test/pending-reporter.mjs`,
`test/lsp-client.mjs`, `test/nvim.test.mjs`, new `test/protocol-errors.test.mjs`.
**Hands off:** `src/**`, other `test/*.test.mjs`, `test/nvim/spec.lua` (ticket 10).

**Blocked by:** ticket 01 (the new deep-nesting generators fail before it lands: write
them, confirm they fail on the base commit, then land after 01).

**Status:** ready-for-agent

- [ ] Reporter order fixed (pending reporter first) or `--test-force-exit` dropped
      (the `running` registry already kills leaked servers); the reporter sets
      `process.exitCode = 1` when a todo passes. Verified with a one-file run.
- [ ] Chaos: sorted corpus; the mutated input (truncated) in the panic message; both
      halves dump under `CARGO_TARGET_TMPDIR`; `mutated_jsonld_never_panics` seeded from
      W3C toRdf inputs and `test/fixtures` JSON-LD; `tests/w3c/shacl/sparql` and a few
      `sh:select` shapes in the Turtle corpus; `sparql_exercise` calls the
      `sparql::features` entry points at random positions; a deterministic pathological
      test (10,000-level nesting of each bracket kind including `{|` on a 2 MiB thread,
      a 100k-token TriG line with an error under a time bound, CR-only text, a 1 MB IRI);
      every span field checked. Default runtime stays under 5 s.
- [ ] `test/protocol-errors.test.mjs` on `rawServer` (from `resources.test.mjs`): for
      each bad input assert the JSON-RPC error code, that a following `hover` is still
      answered, and a clean exit.
- [ ] `lsp-client.mjs` handles `SIGINT`/`SIGTERM` (remove the cache root, kill running
      servers, re-raise). `nvim.test.mjs` asserts `status === 0 && !signal` and a `1..N`
      plan line printed by `spec.lua` (coordinate: `spec.lua` is ticket 10's; add the plan
      line there or read N from a comment).
- [ ] `cargo test --release --test chaos`, `npm run test:node` green.
