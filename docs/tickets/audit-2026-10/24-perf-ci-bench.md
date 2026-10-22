# 24: Perf CI and bench

**What to fix:** nothing tracks performance. `bench/perf.mjs` is manual; CI never runs
it; there is no stored baseline or threshold. The only CI perf assertion,
`tests/jsonld_budget.rs`, is relative to Turtle, so a regression that slows both passes.
The bench's workload is the friendly one: one homogeneous 700 KB Turtle shape, no typed
completion over a big workspace, no partly-described namespace, no file over 700 KB, no
TriG datasets with shapes, no RSS after large responses, no CLI case, no
diagnostics-heavy input. `pct(·, 95)` over 15 samples is the maximum; the first server
does not wait for the initial index; temp workspaces are not removed on throw; `rss()`
shells out to `ps`. The corpus fetch has no checksums, six plain-`http` URLs, moving
targets, and `set -u` without `-e`/`pipefail`.

**Files owned:** `bench/perf.mjs` (JSON output; stress scenarios: 1,000 near-miss IRIs
vs a 7k-term namespace, a 2.4 MB file keystroke, typed completion over 40k described
subjects, 200 TriG datasets with one shape, RSS after `documentSymbol` on a 100k-triple
file; honest p95 (n ≥ 20 or "max"); wait on `workspace/symbol` before measuring;
`try/finally`; a `check` CLI case; a 20k-line undeclared-prefix file), new perf job in
`.github/workflows/ci.yml` (non-blocking on PRs, uploads results on `main`, fails on a
documented ratio against the last `main` artefact), `tools/corpus/fetch.sh`,
`tools/corpus/fetch-jsonld.sh`, `tools/corpus/sources.tsv` (SHA-256 column, https where
the host supports it, `--proto '=https'`, `set -eo pipefail`, versioned URLs).
**Hands off:** `src/**`, `README.md` (ticket 22 regenerates the perf sentences from
this bench's output), other workflow files.

**Blocked by:** ticket 08 (workflow lock).

**Status:** ready-for-agent

- [ ] `node bench/perf.mjs --json out.json` writes every row with n, p50, p95/max,
      RSS; the stress scenarios above are rows; the first server waits for the index;
      cleanup in `finally`; `rss()` works on Windows (`process.memoryUsage` of the child
      via `/proc` or `wmic`/`tasklist` fallback, or skip with a note).
- [ ] CI job on `ubuntu-latest` (noted as noisy): runs the bench, uploads
      `bench-results.json`, compares with the latest `main` artefact, fails over a
      documented ratio (e.g. 1.5× on keystroke p50) on `main` only; instruction counts
      via `perf stat` where available for steadier numbers.
- [ ] Corpus scripts: checksum column verified after download with a warning on
      change; https; `pipefail`; `--max-time` kept.
- [ ] `npm run bench` runs locally under 2 minutes with nothing else running.
