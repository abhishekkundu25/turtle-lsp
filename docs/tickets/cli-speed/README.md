# CLI speed tickets

Spec: docs/specs/cli-speed.md. Research: docs/research/cli-speed.md. Built
on the branch `feat/cli-speed`, one PR, stacked on PR #54 (incremental
keystroke), which edits the same batch and workspace code.

| # | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|
| 01 | `check` skips the formatter verdict; columns from the analysis's line index | — | **src/batch.rs**; src/cli/check.rs; web/engine/src/api.rs |
| 02 | `format` skips diagnostics and import resolution | 01 | **src/batch.rs**; src/cli/format.rs |
| 03 | Close-out: output equality against the base build, docs, full validation, bench | 01, 02 | README.md; CHANGELOG.md; ARCHITECTURE.md; docs/research/cli-speed.md |

Every ticket edits `src/batch.rs`'s `Options` and `analyse_in`, so they run
one at a time, in order. Waves: 01; then 02; then 03.

Scope cut from the research's ranked list: options 1 to 3 only. The
parallel per-file loop (4) and the SHACL work (5) are out: small gains for
their code today.

## Departures

- 02: `format` still builds the knowledge index (that is how the workspace
  analyses files in parallel) and does not keep tokens, so peak memory
  does not move. The `format --check` budget is met on `vocab.ttl` (-19%)
  and missed on `huge.ttl` (-13%).
- The orchestrator built 01 and 02 directly instead of in slices: all three
  tickets edit `src/batch.rs` in order, and each was a few dozen lines.
