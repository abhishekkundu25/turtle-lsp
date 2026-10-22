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

## Wave 3: Design

The ten design changes D1 to D10 of the local refactor plan, numbered 26 to 35 in the
plan's order (D1 = 26, D10 = 35). A change too big for one agent session is split into
lettered slices. Every slice from 26a to 33c is a refactor with **no behaviour
change**, except the failure-path change in 29b (owner decision 4). 34a changes
behaviour with an error cap, and 34b and 35 carry pass/fail memory budgets. 36 closes
the wave. Re-grounded against the code at 8e3f8d7: every line number in the tickets is
current, and the findings that waves 1 and 2 closed are dropped (see "Already done").

**Before wave 3:** on a quiet machine, take the benchmark baseline at 8e3f8d7, outside
the repository: `node bench/perf.mjs --json <outside>/bench-8e3f8d7.json`. Ticket 36
compares against it.

| # | Ticket | Blocked by | Files (lock files in bold) |
|---|---|---|---|
| 26a | [Layout engine 1: options and token document](26a-layout-engine-doc.md) | | `src/layout/{mod,doc}.rs`, `src/features/format.rs`, `src/sparql/format.rs`, import lines of `src/sparql/mod.rs`, `src/sparql/features/mod.rs`, `src/jsonld/format.rs`, `src/query/render.rs` |
| 26b | [Layout engine 2: printer and terms](26b-layout-engine-printer.md) | 26a | `src/layout/{printer,terms}.rs`, `src/features/format.rs`, `src/sparql/format.rs` |
| 27a | [Workspace effects 1: private fields](27a-workspace-effects-fields.md) | | **`src/workspace.rs`**, **`src/server.rs`** (field reads), `src/server/{run_query,workspace_diagnostics}.rs`, `src/batch.rs:750` |
| 27b | [Workspace effects 2: `Effects`](27b-workspace-effects.md) | 27a | **`src/workspace.rs`**, **`src/server.rs`** (`notification` to `after_edit`), `src/server/workspace_diagnostics.rs` |
| 28a | [`ide::Session` 1: navigation](28a-ide-session-navigation.md) | 27a | `src/ide/{mod,navigation,display,edit}.rs`, **`src/server.rs`** (`request` and the helpers after it) |
| 28b | [`ide::Session` 2: display](28b-ide-session-display.md) | 28a | `src/ide/display.rs`, **`src/server.rs`** (display arms) |
| 28c | [`ide::Session` 3: edits and diagnostics](28c-ide-session-edits.md) | 28b | `src/ide/edit.rs`, **`src/server.rs`** (edit arms, `diagnostics_for`, the macros) |
| 29a | [Server state groups](29a-server-state-groups.md) | 27b, 28c, 31 | **`src/server.rs`**, `src/server/*` (new `progress`, `shacl`, `refresh`, `layers`) |
| 29b | [One execution mode: `Spawn`](29b-server-spawn-mode.md) | 29a | **`src/server.rs`**, `src/server/*`, `src/imports/mod.rs`, `src/features/shacl_validate.rs` |
| 30 | [Syntax core no longer calls JSON-LD](30-syntax-jsonld-edge.md) | | `src/syntax/parser.rs`, **`src/syntax/mod.rs`** (module doc), `src/jsonld/mod.rs` (one test), `tests/chaos.rs` |
| 31 | [One file-kind table](31-file-kind-table.md) | 27b | new `src/workspace/kinds.rs`, **`src/workspace.rs`**, **`src/server.rs`** (items above `Server`, `register_watchers`), `src/server/{workspace_diagnostics,run_query}.rs`, `src/cli/files.rs` |
| 32a | [Public surface 1 (expand)](32a-public-surface-expand.md) | 26b, 28c, 29b, 30, 31, 33c, 34b | **`src/lib.rs`**, `src/sparql/features/mod.rs`, `src/diagnose.rs`, `tests/chaos.rs`, dead code in the seven modules made private |
| 32b | [Public surface 2 (contract)](32b-public-surface-contract.md) | 32a, 35 | every file under `src/` |
| 33a | [SHACL model 1: model, vocabulary, lint](33a-shacl-model.md) | | `src/shacl/{model,vocab,mod}.rs`, `src/shacl/shapes.rs` (deleted), `src/shacl/validate.rs` (one import), `src/features/shacl_lint.rs`, `src/features/shacl_lint/sparql.rs` |
| 33b | [SHACL model 2: compiler](33b-shacl-compiler-on-model.md) | 33a | `src/shacl/{validate,model,vocab}.rs` |
| 33c | [SHACL model 3: completion shapes](33c-knowledge-shapes.md) | 33b | `src/knowledge.rs`, `src/shacl/{model,vocab}.rs` |
| 34a | [Token memory](34a-token-memory.md) | 26b, 30 | `src/syntax/{lexer,parser}.rs`, **`src/syntax/mod.rs`** (`SyntaxError`), `src/jsonld/{json,context,expand}.rs`, **`src/workspace.rs`** (`failed_analysis`), message readers, `tests/chaos.rs` |
| 34b | [Analysis memory, 40 B per byte](34b-analysis-memory.md) | 34a, 27b, 31 | `src/analysis.rs`, **`src/workspace.rs`** (`change_doc`, `analyze` calls), `src/batch.rs`, `src/syntax/{parser,names}.rs`, `src/features/{format,completion,semantic,refactor,symbols}.rs`, `src/features/completion/ranking.rs`, `bench/perf.mjs` |
| 35 | [Response serialisation, RSS 1.3x](35-response-serialisation.md) | 29b, 28c | **`src/server.rs`**, new `src/server/wire.rs`, `src/server/*` (send sites), `src/main.rs` (hook) |
| 36 | [Wave 3 close-out](36-wave3-close-out.md) | all of the above | `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `AGENTS.md`, `docs/specs/workspace-split.md`, this file's wave-3 outcome |

The plan assumed all ten changes take the `src/server.rs` lock. Only D2 (27a, 27b), D3
(28a-c), D4 (29a, 29b) and D10 (35) do. D6 (31) edits only the top of the file and
`register_watchers`. D7 touches it only in its sweep (32b). D1, D5, D8 and D9 (26a,
26b, 30, 33a-c, 34a, 34b) edit no server file.

### Waves

| Wave | Tickets | Shared lock files, and how the edits stay apart |
|---|---|---|
| W1 | 26a, 27a, 30, 33a | None: the file sets are disjoint. |
| W2 | 26b, 27b, 28a, 33b | `src/server.rs`: 27b edits `notification` to `after_edit`, and 28a edits `request` and the helpers after it. The one shared statement is `use crate::workspace::{..}`: 27b removes `PREFIX_FILE`, 28a removes `path_to_uri`, and the second to land keeps both removals. |
| W3 | 28b, 31, 33c, 34a | `src/server.rs`: 28b edits the display arms and the `use crate::features` statements; 31 edits the items above `pub struct Server`, the `use crate::workspace` statement and `register_watchers`. The `use` lines between stay untouched. `src/workspace.rs`: 31 edits the kind predicates and their callers, 34a only `failed_analysis`. `src/syntax/mod.rs`: only 34a. |
| W4 | 28c, 34b | None: 28c edits only `src/server.rs` and `src/ide/edit.rs`, and 34b edits neither. |
| W5 | 29a | Alone. |
| W6 | 29b | Alone. |
| W7 | 35, 32a | None: 35 owns `src/server*` and `src/main.rs`; 32a owns `src/lib.rs`, `src/sparql/features/mod.rs` and dead code in the private modules, and no server file. A `dead_code` warning that appears only once both land is fixed by the second to land. |
| W8 | 32b | Alone (it edits every file). |
| W9 | 36 | Alone. |

Seams come before what builds on them. 27b (`Effects`) and 28a-c (`ide::Session`) land
before 29a (state groups), and 28c before 35 (the response path goes through
`Session`). D7 is numbered 32 but runs last of the code tickets, because it touches
every file.

### Rules for every wave-3 brief

- Each test run stays under 2 minutes: `timeout 120`, one test binary
  (`cargo test --release --locked --lib`, `--test <name>`) or one Node file at a time.
  Compiling is not a test run.
- Never point `TMPDIR` inside the repository. The CLI tests run `check --fix` in temp
  folders, and under the repository that rewrites tracked fixtures.
- No performance benchmarks inside a slice. The exceptions are 34b's and 35's budget
  measurements, which 36 reruns on a quiet machine.
- No new dependencies: the dependency lists in `Cargo.toml`, `package.json` and the
  lock files stay as they are.
- Changes are left uncommitted; stop every server you start.

### Owner decisions assumed (wave 3), to confirm

1. **Layout options** live in `crate::layout`. `features::format` re-exports them and
   keeps `Options::from_lsp` (an `lsp_types` adapter), so `layout` has no LSP
   dependency.
2. **`Effects`** is a plain return value, not `#[must_use]`; `batch` and tests ignore
   it.
3. **`ide::Session`** holds `&mut Workspace`, the encoding, `&Settings` and
   `resolve_edits`. RDF and SPARQL `prepareRename` share `Result<Option<_>, String>`,
   and SPARQL's "nothing to rename" stays `null` on the wire.
4. **`Spawn::Inline`** keeps each job's own inline meaning; `Spawn` only replaces the
   bool. A SHACL or workspace-diagnostics thread that cannot start is reported instead
   of panicking: the one failure-path change in the refactors.
5. **Syntax and JSON-LD:** `syntax::parse_with_base` given `Syntax::JsonLd` returns a
   parse with one `syntax` error and no triples. No public dispatcher is added; the
   chaos test routes JSON-LD itself.
6. **File kinds:** the table lives in `src/workspace/kinds.rs`. Today's letter-case
   rules stay: RDF and import-source extensions in any case, SPARQL extensions lower
   case only. `is_turtle` is renamed `is_rdf_file`. Unifying the case rules is a
   follow-up with its own tests.
7. **Public surface:** the consumers are `web/engine`, `examples/`, `tests/` and
   `src/main.rs`. An item only they use stays `pub`, with no `#[doc(hidden)]` and no
   test-only feature.
8. **SHACL model:** `knowledge::Shape` is built from `shacl::model` with today's
   narrower definition. If that costs more per edit than the current builder, 33c
   keeps the builder and documents the narrower definition instead.
9. **Error cap:** Turtle-family syntax errors are capped at 100 per document, as JSON's
   are, with one final error saying so. Parsing goes on past the cap.
10. **Memory budget:** D9's budget is measured on the command line (`format --check`,
    `check --no-shacl`) on the bench's big-file document; the server's RSS rows are
    reported, not gated.
11. **Responses:** on stdio, every request's result is serialised straight to text.
    `server::Embedded` keeps returning `lsp_server::Message`. If that alone misses
    1.3 times, the server may call mimalloc's `mi_collect` after a response over
    1 MB, through an `extern "C"` declaration in `src/main.rs`; no crate is added.
12. **Workspace split:** owner question 9 (a fourth package for the SPARQL syntax core,
    or a sanitizer vendored into SHACL) stays open. 36 records the dependencies as the
    code has them and names the question.
13. **Benchmark baseline:** taken at 8e3f8d7 before W1, outside the repository.

### Already done (dropped or shrunk from the plan)

- **X4-04** (one diagnostics composer): `diagnose::document` is used by the server
  (`src/server.rs:1531-1535`), workspace diagnostics and `batch`, and
  `tests/diagnostics_agree.rs` checks it. 28c only moves the routing.
- **X4-06** (IRI names): `src/syntax/names.rs` (`shorten`, `is_pn_prefix`,
  `declarable_namespace`, `vocabulary_namespace`).
- **X4-08** (imports split, HTTP and cache out): `src/imports/{mod,worker,load,catalog,rdfxml}.rs`
  and `src/fetch.rs`. This also covers X4-02 (3).
- **X4-09** (import format): `imports::load::format_of` reads `Syntax::from_extension`
  first, then sniffs (`src/imports/load.rs:531-537`). 31 does not touch it.
- **X4-10** (cancellation): the SHACL-SPARQL engine polls the job's cancel check
  inside a query (`src/shacl/sparql.rs:367-432`).
- **X4-11, part:** the timers are keyed (`src/server/timers.rs`) and `spawn_guarded`
  exists (`src/server/worker.rs:32`). 29a and 29b keep only the state groups and
  `Spawn`.
- **X4-12, part:** the `text` parameters are gone (`complete(a, k, enc, pos)`,
  `document_symbols(a, enc)`, `quickfix::code_actions(a, k, enc, &p)`). 28a keeps the
  prepare-rename error convention.
- **X4-14, part:** the module index is in the crate doc of `src/lib.rs`. 31 keeps the
  `is_turtle` rename.
- **X4-16, part:** `batch::byte_offset` and `Lines` are gone; `batch` converts
  positions with `analysis::offset_in` (`src/batch.rs:17`, `:767`). The duplicated
  position helpers of `Analysis` and `SparqlAnalysis` (`src/analysis.rs:304-313`,
  `:403-411`) have no ticket.
- **X5-14** (one URI parse per location): `DocLocs` in
  `src/features/navigation.rs:72-95`, with a test that counts the parses (`:691-697`).
  Dropped from 35.
- **Not ticketed, outside D1-D10:** X4-07 (a `Frontend` enum on `Analysis` instead of
  three JSON-LD-only fields) and X4-18 (a note in `query::run`'s doc).

## Owner decisions assumed

1. `turtle-lsp.json` cannot enable the network or raise resource limits; `--network`
   and editor settings still can.
2. `check` exits non-zero when a shape, path or file was skipped (fail closed).
3. Recursion past the depth limit is reported, not silently passed.
4. Turtle, TriG and N-Quads are parsed against the document URI as base (as JSON-LD is).
5. SHACL-SPARQL *target* queries are not held to the pre-binding restrictions.
6. Publishing waits for CI on the tagged commit.

### Added during wave 3

| # | Ticket | Blocked by | Files |
|---|---|---|---|
| 37 | [SHACL checks honour cancellation and the step budget](37-shacl-cancel-in-checks.md) | 33b | `src/shacl/validate.rs` |
| 38 | [Syntax error cap: 1,000 in the editor, none in `check`](38-syntax-error-cap-editor-only.md) | 34a, 32b | `src/syntax/parser.rs`, `src/features/diagnostics.rs` |

## Outcome (2026-10-22)

All 25 tickets landed on `audit-2026-10`, followed by two rounds of fixes from a
whole-branch code review. Departures from the plan:

- **Order:** ticket 21 ran after ticket 15, not alongside it, because both edit
  `src/features/*` call sites. Ticket 23 ran alone after ticket 20 so its benchmark
  numbers were taken on an idle machine.
- **Ticket 04:** `GROUP_CONCAT` memory is not capped (the fix is in `vendor/spareval`).
  The README no longer promises "about 1 GB"; the limit counts rows.
- **Ticket 06:** a SHACL check cut short by the depth limit or the step budget is a
  warning (`shacl-failure`), so `check` exits 0 for it. An ill-formed `sh:path` is an
  error.
- **Ticket 07:** `geof` and `geor` have no terms, so they are no longer registered and
  no longer offered in prefix completion.
- **Ticket 13:** a folder created or renamed into the workspace is not indexed until
  restart.
- **Ticket 14:** private hosts are allowed by the environment variable
  `TURTLE_LSP_ALLOW_PRIVATE_HOSTS`, not a setting (`src/settings.rs` was not owned).
- **Ticket 15 and 14:** each landed as a move commit followed by a change commit
  (ticket 15 as one commit; its step patches were kept outside the repo).
- **Ticket 20:** 2,000 watched files take longer than the initial index (28.7 ms
  against 20.2 ms), short of the ticket's target.
- **Ticket 21:** directional literals exposed a panic in `vendor/spareval`
  (`ExpressionTerm::eq` had no `DirLangStringLiteral` arm). It is fixed by a
  second vendored patch, `dir-lang-string-equality.patch`, which
  `tools/check-vendor.sh` applies with the first.
- **Ticket 22:** `CHANGELOG.md` has an `## Unreleased` section, and
  `test/versions.test.mjs` allows it except in the release job.
- **Review fixes:** a `turtle` configuration section now replaces the
  `initializationOptions` layer. Results produced after the SHACL step budget runs out
  are dropped.

Known defects with no ticket: repeated `UNION` (~4,500) and `FILTER` (~8,000) still
overflow the query evaluator; a JSON-LD span can split a multi-byte character in
malformed input (ignored chaos test); an undecodable LSP message ends the session,
and a large `Content-Length` is allocated up front (todo protocol tests); the
formatter is not idempotent on a dangling `;`; the undeclared-prefix message and
prefix completion always say `@prefix`.
