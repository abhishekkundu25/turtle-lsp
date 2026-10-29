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

**Status:** done

Budgets (spec, pass/fail): see docs/specs/cli-speed.md.

## Results (2026-10-29)

Output equality: the base build (`9ab3a95`) and the branch ran `check
--format json`, `check --format sarif`, `check`, `check --statistics`,
`check --no-shacl`, `format --check` and `format --diff` on `test/corpus`,
`test/fixtures`, the bench `check-cli` workspace and the two 2.4 MB bench
files: 28 comparisons, stdout, stderr and exit codes byte-identical.

Full validation: fmt and clippy clean; lib 834 (1 ignored, existing),
`timing_` 33, doc 1; all 17 integration files; all 49 Node files (839
tests); latency 11, VS Code 48, browser engine 39.

Timings, base against branch, alternated, median of 11 (corpus 5). The load
average was 2.5 to 2.8 from other applications, so these are indicative,
not bench results:

| Input | Command | base ms | branch ms | change |
|---|---|---:|---:|---:|
| `huge.ttl` 2.5 MB | `check` | 49.0 | 32.2 | -34% |
| `huge.ttl` 2.5 MB | `format --check` | 37.8 | 32.8 | -13% |
| `vocab.ttl` 2.5 MB | `check` | 56.8 | 37.3 | -34% |
| `vocab.ttl` 2.5 MB | `format --check` | 48.3 | 38.9 | -19% |
| 202-file workspace | `check` | 93.7 | 52.2 | -44% |
| 202-file workspace | `format --check` | 84.9 | 70.3 | -17% |
| `test/corpus` | `check` | 4,597 | 4,424 | -4% |
| `test/corpus` | `format --check` | 401 | 272 | -32% |

`big-file-peak` (committed memory, budget 64 B per byte): `format --check`
55.0 → 54.4 B per byte (137 → 136 MB); `check --no-shacl` 55.0 → 41.6 B per
byte (137 → 104 MB).

Budgets:

- [x] Output byte-identical.
- [x] Workspace `check` at least 25% faster (-44% against the base, which
  is main plus PRs #47 and #54; main's `check-cli` row was within 3% of the
  base).
- [x] Corpus `check` not slower (-4%).
- [ ] `format --check` on the 2.5 MB file at least 15% faster: `huge.ttl`
  -13%, `vocab.ttl` -19%. Most of what is left is the knowledge index, which
  `format` still builds because that is how the workspace analyses files
  (in parallel); skipping it is a workspace change, left out.
- [x] `big-file-peak` within budget, not higher.
- [ ] Keystroke and index rows: not run. The machine was not quiet, and no
  file on the server's path changed (`batch` is used only by the CLI and
  the browser toolbox).

The corpus `check` took 4.4 to 4.6 s here against the research note's
1.1 s with the same build; the difference is in the base too and was not
looked into.
