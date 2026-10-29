# 03: Close-out

**What to build:**

- Output equality: the base build (`feat/incremental-keystroke`) and the
  branch run `check --format json`, `check`, `check --statistics` and
  `format --check` on `test/corpus`, `test/fixtures` and the bench
  `check-cli` workspace; outputs and exit codes are byte-identical.
- Docs: CHANGELOG (Unreleased), ARCHITECTURE.md (the two batch options),
  README if it names CLI timings.
- Full validation on a quiet machine: fmt, clippy, every Rust suite, every
  Node file, latency, VS Code, browser.
- Bench, main against the branch: `check-cli`, `big-file-peak`, the
  keystroke and index rows; time `check` and `format --check` on the 2.5 MB
  file and `test/corpus`.

**Blocked by:** 01, 02.

**Status:** todo

Budgets (spec, pass/fail): see docs/specs/cli-speed.md.
