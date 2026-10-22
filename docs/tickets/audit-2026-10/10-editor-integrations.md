# 10: Editor integrations

**What to fix:** VS Code extension, Neovim config and `build.lua`.

VS Code (`editors/vscode/src/extension.ts`, `package.json`):
- `turtle-lsp.json` has no effect for any key the extension knows: `serverSettings()`
  sends `c.get(...)` for every key (the contributed default when unset), and
  `synchronize.configurationSection` pushes the whole section after start; the server
  stacks file → init → config, so defaults beat the project file (`extension.ts:40-53`,
  `:79-81`). Reproduced with `format.lineWidth`. README:467-484 presents the file as
  shared settings for every editor. Note: this accidental override is what keeps the
  network off in untrusted workspaces today; ticket 03 makes the server refuse the file's
  `imports.network`, so this fix must land after 03 and must also send
  `imports.network: false` whenever `!workspace.isTrusted`.
- "Turtle: Restart Language Server" throws after a failed start (`client.stop()` in
  `vscode-languageclient` 10.1.2 throws unless Running), so `start()` never runs; start
  and restart are not serialised; a configured path is never checked for existence
  (`:147-156`, `:25-26`, `:145-167`).
- The PATH fallback joins relative/empty PATH entries, which spawn resolves against the
  workspace cwd (`:35-36`). `turtle.query.*`, `shacl.maxTriples`, `diagnostics.workspace`
  are not in `restrictedConfigurations` (`package.json:270-296`).
- `serverSettings()` is a hand-copied key list that has drifted (`query.maxBufferedRows`
  missing); the manifest test's expected table too.

Neovim (`lsp/turtle_lsp.lua`, `plugin/turtle_lsp.lua`, `test/nvim/spec.lua`):
- `root_markers = { "prefixes.ttl", ".git" }`: the nearest `prefixes.ttl` wins over the
  repo root, so a nested `data/prefixes.ttl` splits the project into two servers (SHACL
  stops seeing data; cross-file features break). Reproduced with `vim.fs.root`.
- `.turtle` files get no filetype. `spec.lua` carries its own drifted copy of the config
  instead of loading the shipped files.

`build.lua`:
- Removes the old binary before extracting; `fs_copyfile` result unchecked;
  `vim.system():wait()` with no timeout; a 404 (version drift, or `version = "*"` between
  tag push and asset upload) raises instead of falling back to `cargo`; checksums come
  from the same release as the asset; `curl` without `--proto =https`.

**Files owned:** `editors/vscode/src/extension.ts`, new `editors/vscode/src/server.ts`,
`editors/vscode/package.json` (`restrictedConfigurations`, `untrustedWorkspaces.description`),
`editors/vscode/test/manifest.test.mjs`, `editors/vscode/test/suite.cjs`, new
`editors/vscode/test/find-server.test.mjs`, `lsp/turtle_lsp.lua`, `plugin/turtle_lsp.lua`,
`build.lua`, `test/nvim/spec.lua`, `test/nvim-plugin.test.mjs`, new
`test/fixtures/nested-prefixes/`.
**Hands off:** `README.md` (ticket 22), `src/**`, `.github/**`, `editors/vscode/syntaxes/`
and language configurations (later ticket).

**Blocked by:** ticket 03 (server refuses `imports.network` from the file).

**Status:** ready-for-agent

- [ ] `settingsFor(trusted)` builds init options from `package.json`'s
      `contributes.configuration.properties` using `inspect()` (only user-set values),
      adds `imports.network: false` when untrusted, and is also used as the
      `didChangeConfiguration` payload (middleware). Manifest test: the key set equals
      the keys `src/settings.rs` applies (grep-derived list).
- [ ] `ServerHandle` (`src/server.ts`) with serialised `ensureStarted()`, `restart()`,
      `dispose()`; `findServer(opts)` as a pure function: configured path must exist and
      be absolute; PATH entries absolute only; candidates inside a workspace folder
      skipped when untrusted. `find-server.test.mjs` with a stubbed fs. Suite check:
      bad path, then restart with a good path, ends Running.
- [ ] `restrictedConfigurations` gains `turtle.query.timeoutSeconds`, `query.maxRows`,
      `query.maxBufferedRows`, `shacl.maxTriples`, `diagnostics.workspace`.
- [ ] Neovim: `root_markers = { "turtle-lsp.json", ".git", "prefixes.ttl" }`; `turtle`
      extension added; `spec.lua` puts the repo on `rtp` and uses
      `vim.lsp.config("turtle_lsp", { cmd = … })` + `vim.lsp.enable`. New fixture
      `test/fixtures/nested-prefixes/` with `data/prefixes.ttl`; check that a file in
      `data/` and one in `shapes/` share one client.
- [ ] `build.lua`: extract into a temp dir under `bin/` then rename; check
      `fs_copyfile`; `timeout` on `vim.system`; on 404 fall back to `build()` when
      `cargo` exists with a clear message; `--proto =https --tlsv1.2` unless
      `TURTLE_LSP_RELEASE_URL` is set (then warn); `gh attestation verify` when `gh` is
      on PATH. `nvim-plugin.test.mjs`: a `win32-x64` asset, a 404-then-no-cargo message,
      a leak check via `test/procs.mjs`.
- [ ] `node --test test/grammar.test.mjs test/manifest.test.mjs` (in `editors/vscode`),
      `npm run typecheck`, `test/nvim.test.mjs`, `test/nvim-plugin.test.mjs` green. The
      Electron suite (`node test/run.mjs`) is run once if VS Code is already downloaded
      under `.vscode-test/`.
