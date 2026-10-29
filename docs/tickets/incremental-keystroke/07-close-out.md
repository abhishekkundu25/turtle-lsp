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

**Status:** done

Budgets (spec, pass/fail):

- [x] Results equal a full rebuild in every chaos iteration and on the
  corpus.
- [x] `big-vocab` keystroke at most 8 ms; `big-file` keystroke at most 8 ms.
- [x] No other keystroke row more than 5% slower; initial index within 10%.
- [x] The big-file RSS rows within 10%.

## Results (2026-10-29)

Full validation on the merged branch, nothing else building: fmt and clippy
clean; lib 830 passed (1 ignored, existing), `timing_` 33, doc 1; all 17
integration files (chaos 35, corpus_edits 4 over 1,361 corpus files); all 49
Node test files (839 tests); latency 11, VS Code 48, browser engine 39.

The bench's keystroke rows now send each keystroke as one ranged change,
as editors do (the server syncs incrementally); before, they re-sent the
whole text, which gives the splice nothing to build on, so a first run
showed no gain. Main and PR #47 were run with the same bench script and
test client. Medians in ms, best of two rounds alternated between the
builds, load about 1.7 to 2.4 from other applications:

| Row | main | PR #47 | branch | vs main |
|---|---:|---:|---:|---:|
| keystroke (30,000 lines) | 3.9 | 4.0 | 1.5 | -62% |
| near-miss keystroke | 1.8 | 1.8 | 1.6 | -11% |
| big-file keystroke (2.4 MB) | 13.4 | 13.8 | 4.9 | -63% |
| big-vocab keystroke (2.4 MB) | 16.1 | 21.0 | 6.1 | -62% |
| prefix keystroke | 182.7 | 181.5 | 177.0 | -3% |
| index ready | 21.8 | 22.7 | 23.1 | +6% |
| typed index | 17.4 | 18.5 | 19.1 | +10% |
| big-file open | 18.4 | 18.6 | 19.2 | +4% |
| big-vocab open | 19.9 | 25.1 | 24.7 | +24% (PR #47's labels) |
| check (202 files) | 88.9 | 89.6 | 91.3 | +3% |
| RSS big file (MB) | 141 | 144 | 107 | -24% |
| RSS near-miss (MB) | 19 | 20 | 21 | +11% |
| RSS typed (MB) | 91 | 92 | 96 | +5% |

Budgets: both 2.4 MB keystrokes under 8 ms; no keystroke row slower; the
initial index within 10% (index ready +6%, typed index +9.8%, of which
PR #47 is +6%); big-file RSS 24% lower. The near-miss RSS (+2 MB) is outside
the spec's RSS budget, which names the big-file rows.
