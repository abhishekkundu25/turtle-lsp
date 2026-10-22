# 03: Settings trust and duration parsing

**What to fix:** two defects in how settings are read.

1. A cloned repository can turn on network fetching. `turtle-lsp.json` at the first
   workspace root is the first layer (`src/server.rs:263-269`), every layer reaches the
   `["imports","network"]` arm (`src/settings.rs:163`), and `turtle-lsp check` honours
   the file too (`src/cli/check.rs:30-37`). The server then GETs every `owl:imports` and
   JSON-LD `@context` IRI. VS Code is safe only because its extension always sends
   `imports.network` in `initializationOptions`; Neovim sends none, and CI `check` on a
   pull request runs requests the author chose. The file can also set `query.*` limits to
   0 (unlimited) and raise `shacl.maxTriples`, and is read with no size cap
   (`settings.rs:22-34`). README:668 says the file layer cannot do this.
2. `Duration::from_secs_f64` panics on finite values beyond ~1.8e19
   (`src/settings.rs:176-179`, `src/cli/args.rs:303-311`). `{"query":{"timeoutSeconds":
   1e300}}` in the file crashes the server at start and every CLI command (exit 101);
   `--timeout 1e300` does the same. Reproduced.

**Files owned:** `src/settings.rs`, `src/cli/args.rs`, `src/cli/check.rs` (lines 28-38
only), `src/server.rs` (the `Settings::from_layers` call in `configure` and the one in
the `didChangeConfiguration` handler only), `test/settings-file.test.mjs`,
`test/cli.test.mjs` (new cases only).
**Hands off:** the rest of `src/server.rs`; `web/engine/src/api.rs` (ticket 09 handles
its own duration).

**Blocked by:** nothing. Ticket 10 (VS Code extension) depends on this landing first.

**Status:** ready-for-agent

- [ ] `Settings::from_layers` takes `(Layer, &Value)` pairs; `Layer::File` ignores
      `imports.network`, clamps `query.timeoutSeconds`, `query.maxRows`,
      `query.maxBufferedRows` and `shacl.maxTriples` to no more permissive than the
      defaults, records what it ignored, and the server/CLI emit one warning naming the
      keys. `read_settings_file` reads at most 1 MB.
- [ ] One `fn seconds(n: f64) -> Option<Duration>` using `try_from_secs_f64`, used by
      settings and `args`; an out-of-range value is "needs a number of seconds" (CLI
      exit 2) or ignored with a warning (settings). Unit tests with `1e300`, `NaN`,
      negative, `0`.
- [ ] Node: a workspace with `turtle-lsp.json` `{"imports":{"network":true}}`, an
      `owl:imports <http://127.0.0.1:PORT/x>` and **no** init options: the local listener
      receives 0 requests; `check --root` on the same folder: 0 requests;
      `check --network`: 1 request. `check` with `timeoutSeconds: 1e300` in the file
      exits 2 with a message, not 101.
- [ ] `cargo test --release` (settings tests), `test/settings-file.test.mjs`,
      `test/imports.test.mjs`, `test/cli.test.mjs`, `test/query-cli.test.mjs` green.
