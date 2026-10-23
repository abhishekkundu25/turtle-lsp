# Chaos hardening tickets

Spec: docs/specs/chaos-hardening.md. Tracking issue: #23 (one GitHub issue
per ticket, numbers in the table). Built with parallel slices on one
integration branch. Nothing is implemented until the plan is agreed.

| # | Issue | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|---|
| 01 | #16 | `token_edits` with re-lex; shorten, extract, expand (D core) | - | **src/syntax/names.rs** (`spaced` replaced by `token_edits`); src/features/refactor.rs; src/sparql/features/actions.rs (shorten and expand hunks) |
| 02 | #15 | Declaration insertion as a token splice (A) | #16 | **src/syntax/names.rs** (`declaration_point`, `insertion_point`, `declarations_edit`); src/sparql/features/edits.rs; src/sparql/features/completion.rs; src/features/quickfix.rs tests; src/features/refactor.rs tests (Extract cases) |
| 03 | #24 | Renames, "Change to", removal and fix composition (D rest) | #16 | src/features/navigation.rs; src/sparql/features/rename.rs; src/features/quickfix.rs (`removal`, "Change to"); src/sparql/features/actions.rs (quick-fix and removal hunks); src/batch.rs |
| 04 | #25 | SPARQL lexer: `<` after an operand is the operator (D precondition) | - | src/sparql/lexer.rs |
| 05 | #17 | Formatter: one pass reaches the fixed point (C) | - | src/layout/doc.rs; src/layout/printer.rs; src/features/format.rs; tests/format.rs; test/format.test.mjs |
| 06 | #18 | Vendor `spargebra` unchanged, exact pin, golden of parsed queries (E step 1) | - | new vendor/spargebra/; **Cargo.toml**, **Cargo.lock**, **web/engine/Cargo.toml**, **web/engine/Cargo.lock**; tools/check-vendor.sh; AGENTS.md (Release step 3 only); new tests/sparql_golden.rs and its data file |
| 07 | #26 | Left-factor the grammar (E root cause) | #18 | vendor/spargebra/src/parser.rs; vendor/spargebra/PATCH.md and the upstream diff; src/sparql/diagnostics.rs (message tests, `MAX_NESTING` re-measurement); **src/sparql/tests.rs** (new test fns) |
| 08 | #19 | Parse fuel in the vendored grammar (E backstop) | #26 | vendor/spargebra/src/parser.rs (`ParserState` fuel); src/sparql/diagnostics.rs (the "too complex" message wiring); **src/sparql/tests.rs** (new test fns); test/shacl-sparql-lint.test.mjs; test/cli.test.mjs; web/test/engine.test.mjs |
| 09 | #20 | Harness: same base, watchdog, diff report, `CHAOS_ONLY`, exemption deleted (B) | #17 | **tests/chaos.rs** (format step, `failure_report`, watchdog, the `known` block and the pending test) |
| 10 | #27 | JSON-LD multi-byte spans: fix the producers, full corpus in the shard | - | src/jsonld/** (span producers), src/semantic.rs (`encode_with`); **tests/chaos.rs** (the `is_ascii` filter and the ignored test only) |
| 11 | #28 | TriG one-line graph block parses in linear time | - | src/syntax/parser.rs; **tests/chaos.rs** (the ignored test only) |
| 12 | #21 | Harness: splice, token-sequence, `fix_all`, external prefixes, SPARQL and rename graph checks, squeeze, snippets | #15, #16, #24, #26, #20, #17 | **tests/chaos.rs** |
| 13 | #22 | Close-out: 20 runs, replays, budget from measurement, CHANGELOG, docs | #15 to #28 | CHANGELOG.md; AGENTS.md (chaos sentence); README.md; this README (results); tests/chaos.rs (the budget constant only) |

Waves:

1. 01, 04, 05, 06, 10, 11 together (six worktrees). 10 and 11 both edit
   tests/chaos.rs in separate hunks (10: the `is_ascii` filter at the top of
   `mutated_jsonld` and the first ignored test; 11: the last ignored test).
2. 02, 03, 07, 09 together once their blockers land (02 and 03 after 01; 07
   after 06; 09 after 05).
3. 08 (after 07) and 12 (after 02, 03, 07, 09) together.
4. 13.

Lock files and how the edits stay apart:

- src/syntax/names.rs: 01 replaces `spaced` and adds `token_edits`; 02 then
  edits `declaration_point`, adds `insertion_point` and routes
  `declarations_edit` through `token_edits`. Sequential.
- src/sparql/features/actions.rs: 01 edits the shorten and expand hunks; 03
  the quick-fix and removal hunks; sequential anyway (03 after 01).
- tests/chaos.rs: 10 and 11 in wave 1 (separate hunks, above); 09 alone in
  wave 2; 12 alone in wave 3; 13 changes one constant.
- src/sparql/tests.rs: 07 and 08 append separate blocks, sequential.
- src/sparql/diagnostics.rs: 07 then 08, sequential.
- Cargo.toml, Cargo.lock, web/engine/Cargo.toml, web/engine/Cargo.lock: 06 only.
- No ticket adds a field to the public `PrefixDecl` (the chaos test builds
  it as a struct literal and the doctest lists it).

## Saved failing inputs (replay green before each class closes)

All under target/tmp (gitignored) as `chaos-turtle-<seed>-<iteration>.<ext>`;
the SPARQL inputs are in the scratchpad. Replay one: `CHAOS_SEED=<seed>
CHAOS_ITERS=200000 CHAOS_ONLY=<iteration> cargo test --release --locked
--test chaos -- mutated_<format>` (ticket 09 adds `CHAOS_ONLY`).

| class | seed | iteration | file |
|---|---|---|---|
| A | 1792760239 | 11540 | chaos-turtle-1792760239-11540.trig |
| A | 1792783996 | 73609 | chaos-turtle-1792783996-73609.trig |
| A | 1792823591 | 28769 | chaos-turtle-1792823591-28769.trig |
| A | 1792777890 | 112031 | chaos-turtle-1792777890-112031.ttl |
| B | 1792807753 | 10348 | chaos-turtle-1792807753-10348.ttl |
| B | 1792815672 | 35229 | chaos-turtle-1792815672-35229.trig |
| B | 1792823591 | 95806 | chaos-turtle-1792823591-95806.trig |
| B | 1792831510 | 110860 | chaos-turtle-1792831510-110860.trig |
| B | 1792825404 | 55759 | chaos-turtle-1792825404-55759.trig |
| B | 1792833323 | 13323 | chaos-turtle-1792833323-13323.trig |
| B | 1792841242 | 177942 | chaos-turtle-1792841242-177942.trig |
| C | 1792815672 | 195134 | chaos-turtle-1792815672-195134.ttl |
| C | 1792841242 | 123532 | chaos-turtle-1792841242-123532.ttl |
| C | 1792817485 | 49915 | chaos-turtle-1792817485-49915.trig |
| D | 1792815672 | 149959 | chaos-turtle-1792815672-149959.ttl |
| E | 1792799834 | 107183 | `SELECT (AVG(` + `REGEX(` x 223 + `DISTINCT ?x) AS ?y) {}` |
| E | 1792817485 | 107745 | `SELECT (:REGEX(` + `REGEX(` x 188 + ... (through `iriOrFunction`) |

## Results

Filled in by ticket 13: the 20 acceptance runs (seed, time per format, five
slowest iterations), the watchdog budget derived from them, the benchmark
medians against main, and the two nightly runs.

## Departures from the plan

- 2026-10-23: first draft reviewed; eight blocking findings folded in
  (static guard table replaced by fuel, character rule replaced by re-lex,
  removal and `fix_all` added, browser manifest added, exact pin, old
  ignores brought into scope, lock plan reworked). Issues #15 to #22 were
  created before the review and rewritten; #24 to #28 added.
