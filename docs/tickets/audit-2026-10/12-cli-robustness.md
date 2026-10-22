# 12: CLI robustness

**What to fix:** `turtle-lsp check`/`format` as a CI gate and a write-back tool.

- Directory scans stop at `MAX_FILES` (20,000) silently and in filesystem order
  (`src/workspace.rs:868-893`); files over 50 MB are dropped silently. Reproduced:
  20,010 good files + 7 bad in sibling folders → "11 errors in 5 files", exit as if done;
  two bad files never checked.
- One non-UTF-8 or unreadable file aborts the whole run with exit 2 and no report
  (`src/cli/files.rs:169-185`).
- `format` and `check --fix` write with `std::fs::write` (truncate then write, not
  atomic), never check the file is unchanged since read, and stop at the first write
  error with no summary (`src/cli/format.rs:52`, `src/cli/check.rs:44-54`). CRLF,
  symlinks and permissions are handled correctly today: keep that.
- SARIF `artifactLocation.uri` is the raw path (no percent-encoding; files outside the
  root get an absolute path with `uriBaseId: %SRCROOT%`) (`src/cli/output.rs:353-410`).
- `--select`/`--ignore` accept unknown codes (a typo filters everything, exit 0); a
  mistyped subcommand silently starts the language server and blocks on stdin;
  `--version` prints to stderr; `help check` prints top-level usage; `-` is not stdin;
  `format --diff` labels absolute paths `a//abs`; `--statistics` ignores `--format`;
  `.pre-commit-hooks.yaml` regex omits `.turtle` and `.jsonld`; `format --help` omits
  JSON-LD.

**Files owned:** `src/cli/{files,format,check,output,args,mod}.rs`, `src/main.rs`,
`src/workspace.rs` (`scan_roots_matching` only: return `Scan { paths, truncated,
too_large }` with sorted entries, one walk), `.pre-commit-hooks.yaml`,
`test/cli.test.mjs`, `test/query-cli.test.mjs`.
**Hands off:** `src/batch.rs` (ticket 19), `src/server.rs`, README.

**Blocked by:** ticket 03 (`check.rs` lines 28-38, `args.rs` duration) and tickets 13/14
(`src/workspace.rs` lock order: 13 → 14 → 12).

**Status:** ready-for-agent

- [ ] `files::write_back(path, old, new)`: compare length+mtime with what was read,
      write a temp file in the same folder, copy the mode, resolve the symlink, rename
      over; per-file errors collected; the run finishes; summary printed; exit 2 if any
      write failed.
- [ ] Read/decode failures become per-file problems (`io`/`encoding`, error) and the run
      continues; exit 2 at the end.
- [ ] Scan truncation and skipped large files: stderr warning naming the cap, exit 2
      (or a CLI-only higher cap: decide and document). The server keeps its behaviour.
- [ ] SARIF: percent-encoded segments; outside-root files as absolute `file:` URIs with
      no `uriBaseId`. Unit test with a space and `#` in the path, and an outside file.
- [ ] Unknown `--select`/`--ignore` code → exit 2 (shared codes table used by
      `output.rs::rule_description`); bare unknown word → exit 2; `--version` to stdout
      outside server mode (check `test/lsp-client.mjs` `warmUp()`); `help <cmd>`;
      `--statistics` with `--format` rejected; `-` either implemented with
      `--stdin-filename` or rejected clearly; `a//abs` label fixed; hooks regex and
      `FORMAT_USAGE` include `turtle` and `jsonld`.
- [ ] e2e in `cli.test.mjs`: read-only file, symlink target kept, non-UTF-8 next to
      good files, file changed between read and write (mtime), truncation with a small
      cap via an env/test hook, SARIF URIs, bad `--select`, `--version` stdout.
