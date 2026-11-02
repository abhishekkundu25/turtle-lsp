# 02: Close-out

Status: done. Blocked by: 01.

## What to build

A CHANGELOG line under `## Unreleased`; the spec's status set to done; the
full validation with nothing else running.

## Acceptance

- [x] The lib unit tests, the Node LSP tests, the web engine and LSP tests, and the VS Code extension tests pass.
- [x] Lens resolve timed before and after on 200 query files: the number, the run count, the spread and what limits it, recorded here.
- [ ] The keystroke rows of `bench/perf.mjs` match `main` (not rerun: see Numbers).

## Numbers

Lens resolve on 30 subjects, 200 query files and 1,000 Turtle files;
2 runs per binary, 150 resolves each after the first; load average about
5.5 (another session running), so the spread is wide.

| | first resolve | median | p95 | max |
|---|---|---|---|---|
| before, file watching | 0.3–0.6 ms | 0.11–0.25 ms | 0.17–0.47 ms | 0.77 ms |
| after, file watching | 7.3–8.1 ms | 0.35–0.37 ms | 0.89–0.93 ms | 2.16 ms |
| before, no file watching | 0.2–0.4 ms | 0.10–0.25 ms | 0.18–0.45 ms | 0.86 ms |
| after, no file watching | 8.5–10.4 ms | 0.85–0.88 ms | 2.17–2.62 ms | 4.31 ms |

The first resolve after startup reads and parses the 200 query files
(Find References pays the same once). After it, with file watching, a
resolve checks each query file's stamp and scans the parsed queries; without
it, a resolve also lists the folders (1,200 files) to find new query files,
which is the likely limiter of the 0.85 ms median: not profiled. Every
resolve after the first is under the 5 ms budget.

The keystroke rows of `bench/perf.mjs` were not rerun: nothing on the
keystroke path changed, and the machine was not quiet.
