# 07: Close-out

**What to build:** README (checks, defaults, `lint.ignore`/`lint.enable` with an example), CHANGELOG `## Unreleased`, VS Code README settings, the editor-and-CLI agreement test with a workspace that triggers the default checks and a `turtle-lsp.json` with both lists, a browser engine test that the default checks appear, the full validation and the benchmark against main on a quiet machine.

**Blocked by:** 01–06.

**Status:** done

- [x] Every suite in AGENTS.md passes; fmt and clippy clean.
- [x] Bench: keystroke-latency rows within 5% of main, initial index of the 200-file workspace within 10%; numbers recorded here.

## Results

Full validation on `f1e8ee7`: lib 788 passed and `timing_` 30 (serially),
doc 1, all 16 integration files (chaos 31), every Node test file (nvim 31,
latency 11), VS Code 48, browser 39.

The first bench run put the initial index at 8x main (181.6 against 22.5 ms)
and keystrokes 30% slower. The fix (`f1e8ee7`) compares only the changes that
can move collisions or term state, keeps the curation counts out of `Entry`,
and reposts text postings once per key.

Bench, p50 ms, best of two, main (`25701b5`) and branch back to back, load
0.98 at the start:

| Row | main | branch | ratio |
|---|---:|---:|---:|
| initial index merged (200 files) | 22.3 | 22.7 | 1.02 |
| keystroke -> diagnostics | 4.4 | 4.6 | 1.05 |
| turtle keystroke | 4.4 | 4.5 | 1.02 |
| jsonld keystroke | 6.3 | 6.4 | 1.02 |
| near-miss keystroke | 1.7 | 1.8 | 1.06 |
| big file keystroke | 15.6 | 16.0 | 1.03 |
| undeclared-prefix keystroke | 181.3 | 178.9 | 0.99 |
| typed completion index (40k) | 18.0 | 18.6 | 1.03 |
| check, 202 files | 87.0 | 89.2 | 1.03 |

The near-miss row is one step of the bench's 0.1 ms resolution; the
fix's own paired runs had it at 1.8 against 1.8.
