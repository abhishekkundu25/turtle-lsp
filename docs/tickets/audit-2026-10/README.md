# Audit 2026-10: remediation tickets

Source: the repo-wide audit of 2026-10-22 (commit d7c33eb, 0.7.1). The audit report
and the full refactor plan are local-only (`docs/audit/`, excluded from git); each
ticket here is self-contained: it names the defect, the files it owns, the files it
must not touch, and the tests that prove it.

Each ticket is one slice for `/parallel-slices`: slices in the same wave edit disjoint
files and run together; a **lock** file is edited by one slice at a time, in the order
given. Tests every slice keeps green, each run under two minutes: `cargo fmt --check`,
`cargo clippy --release --locked --all-targets -- -D warnings`, `cargo test --release
--locked`, and the Node files the ticket names (`timeout 120 node --test
--test-timeout=30000 test/<file>.test.mjs`). Every ticket adds a regression test per
finding it closes.

## Wave 1: High (parallel, disjoint files)

| # | Ticket | Lock |
|---|---|---|
| 01 | [Turtle parser depth guards](01-parser-depth-guards.md) | |
| 02 | [SPARQL complexity guard](02-sparql-complexity-guard.md) | |
| 03 | [Settings trust and duration parsing](03-settings-trust-and-duration.md) | `src/server.rs` (two call sites), `src/settings.rs` |
| 04 | [Query stop semantics](04-query-stop-semantics.md) | |
| 05 | [JSON-LD context identity and budget](05-jsonld-context-identity-budget.md) | `src/analysis.rs` (one line) |
| 06 | [SHACL bounds and fail-closed validation](06-shacl-bounds-fail-closed.md) | `src/features/shacl_validate.rs` |
| 07 | [Suggestion search and vocabulary data](07-suggestion-search-vocabulary.md) | |
| 08 | [CI, release and web gating](08-ci-release-web-gating.md) | `.github/**` |
| 09 | [Web engine seam and worker](09-web-engine-seam-worker.md) | |
| 10 | [Editor integrations](10-editor-integrations.md) | |
| 11 | [Test harness](11-test-harness.md) | `package.json`; land 01 first |

## Wave 2: Medium (after wave 1; locks in the order listed)

| # | Ticket | Lock / order |
|---|---|---|
| 16 | [Module stubs](16-module-stubs.md) | `src/lib.rs`, `src/syntax/mod.rs`; **first** |
| 13 | [Server isolation and index consistency](13-server-isolation-index.md) | `src/server.rs` #1, `src/workspace.rs` #1 |
| 14 | [Fetch module and file-URL policy](14-fetch-module-file-policy.md) | `src/workspace.rs` #2 |
| 12 | [CLI robustness](12-cli-robustness.md) | `src/workspace.rs` #3 |
| 15 | [Turtle naming module](15-turtle-naming-module.md) | after 16 |
| 19 | [Diagnostics composer and quick fixes](19-diagnostics-composer-quickfix.md) | `src/server.rs` #2 |
| 20 | [Performance set A](20-performance-set-a.md) | `src/server.rs` #3, `src/workspace.rs` #4, after 15 |
| 21 | [Turtle semantics](21-turtle-semantics.md) | `src/syntax/mod.rs`; after 05 and 16 |
| 17 | [Web app wave 2](17-web-app-wave2.md) | `package.json` after 11 |
| 18 | [JSON-LD follow-ups](18-jsonld-followups.md) | after 05 |
| 23 | [Manifest and release profile](23-manifest-and-profile.md) | `Cargo.toml`; alone |
| 24 | [Perf CI and bench](24-perf-ci-bench.md) | `.github/**` after 08 |
| 25 | [Test depth](25-test-depth.md) | after 04, 20 |
| 22 | [Docs truth, notices, onboarding](22-docs-truth-notices-onboarding.md) | docs; **last** |

Wave 3 (design: layout engine, `Workspace::Effects`, `ide::Session`, server state
groups, file-kind table, public-surface narrowing, SHACL model, token memory, response
serialisation) is planned in the local audit folder and ticketed once wave 2 lands.

## Owner decisions assumed

1. `turtle-lsp.json` cannot enable the network or raise resource limits; `--network`
   and editor settings still can.
2. `check` exits non-zero when a shape, path or file was skipped (fail closed).
3. Recursion past the depth limit is reported, not silently passed.
4. Turtle, TriG and N-Quads are parsed against the document URI as base (as JSON-LD is).
5. SHACL-SPARQL *target* queries are not held to the pre-binding restrictions.
6. Publishing waits for CI on the tagged commit.
