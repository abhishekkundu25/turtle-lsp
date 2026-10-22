# 36: Wave 3 close-out: docs, full validation, benchmark comparison

**What to build:** after tickets 26a to 35, the docs describe a code base that no
longer exists, and no one has run the whole suite or compared the performance against
the start of the wave. Bring the docs up to date with the code as it now is:
- `ARCHITECTURE.md`: the module table, which says `ide` and `layout` are empty; the
  seams, which gain `ide::Session` as the in-process request interface and lose the
  claim that requests are routed in `server.rs`; workspace invalidation through
  `Effects`; the file-kind table; `Spawn` in "Threads"; the stdio transport of 35; the
  "Files most changes touch" list.
- `README.md`, wherever it states memory or performance figures or the module layout.
- `CHANGELOG.md`, under `## Unreleased`: what users see, which is the error cap (34a),
  lower memory (34b, 35) and nothing else.
- `AGENTS.md`, where a command or rule changed (for example the bench's `big-file-peak`
  scenario).
- `docs/specs/workspace-split.md`, "Further Notes" (an untracked draft). Today it says
  "the SHACL engine depends only on the parser, and the parser on no other module",
  which was false at the audit (X4-02, M21). Restate the dependencies as the code has
  them after wave 3: the parser depends on no other module (30); `layout` sits at the
  bottom (26a, 26b); SHACL still depends on `rdf`, the SPARQL lexer and validator, and
  `jsonld::json` (for `rdf:JSON`) and `clock`. Name owner question 9 as still open: a
  fourth package for the SPARQL syntax core, or a sanitizer vendored into SHACL (owner
  decision 12).

Then run the full validation and the benchmark comparison, and write the wave's outcome
into `docs/tickets/audit-2026-10/README.md` (an "Outcome (wave 3)" section after the
existing "Outcome") and the departures into `docs/audit/02-refactor-plan.md` (local
only).

**Files owned:** `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `AGENTS.md`,
`docs/specs/workspace-split.md`, `docs/tickets/audit-2026-10/README.md` (the new outcome
section only), `docs/audit/02-refactor-plan.md` (departures log only). No other ticket
edits these files.
**Hands off:** all code, tests, `bench/perf.mjs`, workflows. A defect found here is
reported with a reproduction, not fixed.

**Blocked by:** every wave-3 ticket: 26a, 26b, 27a, 27b, 28a, 28b, 28c, 29a, 29b, 30,
31, 32a, 32b, 33a, 33b, 33c, 34a, 34b, 35. Runs alone, last.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. The benchmark runs only here, on a quiet machine (load average
under 1, nothing else building). No new dependencies. Leave the changes uncommitted.
Stop every server you start; leave no temp folders or caches behind.

- [ ] Docs: each statement in the files above that names a module, a seam, a figure or
      a command matches the code (spot-check each against `src/lib.rs`'s module index
      and the files it names). `RELEASE_TAG` unset, `test/versions.test.mjs` green
      (it allows `## Unreleased`).
- [ ] Full validation on the merged tree: `cargo fmt --check`; `cargo clippy --release
      --locked --all-targets -- -D warnings`; `cargo test --release --locked --lib` and
      each integration test binary on its own under `timeout 120`; every
      `test/*.test.mjs` file on its own
      (`timeout 120 node --test --test-timeout=30000 test/<file>.test.mjs`) and
      `timeout 120 npm run test:latency`; `node web/build.mjs` and each
      `web/test/*.test.mjs`; `(cd editors/vscode && timeout 120 node --test
      test/manifest.test.mjs)`; `tools/check-vendor.sh`. Report the pass, fail and todo
      counts against the end of wave 2.
- [ ] Resource hygiene after the runs: no `turtle-lsp` process left (`pgrep -fl
      turtle-lsp`), CPU idle, no `turtle-*` folders left in the temp directory, the
      user's import cache untouched.
- [ ] Benchmark comparison: `node bench/perf.mjs --json AFTER.json` on the quiet
      machine, then `node bench/perf.mjs --compare BEFORE.json AFTER.json
      --report-only`, where `BEFORE.json` is the run taken at 8e3f8d7 before wave 3
      (README, "Before wave 3"). If there is none, build 8e3f8d7 from
      `git archive 8e3f8d7 | tar -x -C <scratch>` outside the repository, with its own
      target directory, and run its bench there; the repository's history is not
      touched. Paste the comparison table into the outcome section. A gated row over
      1.5 times is listed as a regression.
- [ ] Budgets rerun on the quiet machine: `peak-format-big` and `peak-check-big` at
      most 40 B per input byte (34b); `rss-symbols-after` at most 1.3 times
      `rss-symbols-before` (35). Both are reported with the machine and the load
      average.
