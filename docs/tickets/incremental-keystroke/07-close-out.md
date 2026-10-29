# 07: Close-out

**What to build:**

- Corpus test: every corpus file, opened and then edited at a few
  positions (through the oracle), matches a reopen. With `CI=true` and no
  corpus it fails, as the other corpus tests do.
- Docs: CHANGELOG (Unreleased), README and editors/vscode/README.md (the
  new setting), ARCHITECTURE.md (the splice module, the oracle, the check
  classes).
- Full validation on a quiet machine, nothing else running: fmt, clippy,
  every Rust suite, every Node file, latency, VS Code, browser.
- Bench, main against the branch, best of two: every keystroke row,
  `big-file` and `big-vocab` open and keystroke, the RSS rows.

**Blocked by:** 01–06.

**Status:** open

Budgets (spec, pass/fail):

- [x] Results equal a full rebuild in every chaos iteration and on the
  corpus.
- [ ] `big-vocab` keystroke at most 8 ms; `big-file` keystroke at most 8 ms.
- [ ] No other keystroke row more than 5% slower; initial index within 10%.
- [ ] The big-file RSS rows within 10%.
