# 09: Web engine seam and worker

**What to fix:** the browser build's engine operations and worker protocol.

- Run Query and Fix All run with `Settings::default()`, not the user's settings
  (`web/engine/src/api.rs:102,162,194`; `web/app/query-pane.js:106`;
  `web/app/lsp-worker.js:41`). Reproduced: a prefix set in Settings → Prefixes gives 0
  diagnostics in the editor but the query fails with "Undeclared prefix". Fix All applies
  lints the user turned off and formats at the default width. Query limits are constants
  (`query-pane.js:7-8`).
- The `query` request is not validated at the seam: a huge `timeout` panics
  `Duration::from_secs_f64` and aborts the instance; `maxRows` `as usize` wraps 2^32 to 0
  (`api.rs:185-188`). Reproduced.
- The LSP client has no cancellation and no coalescing: every keystroke posts its own
  `didChange` and each provider queues a request; the worker runs them one by one, so the
  browser loses the native server's "one analysis per burst" property. Measured: 40 ms
  per `didChange` on a 500 KB file in wasm; diagnostics settle ~950 ms after edits
  (`web/app/editor-lsp.js:56-83,160-316`; `lsp-worker.js:19-32`;
  `lsp-host.js:96-98`).
- `LspClient` and the Toolbox worker set no `onerror`/`onmessageerror`: a dead worker
  leaves promises pending forever (`editor-lsp.js:37-46`; `query-pane.js:42-45` does it
  right). `fetchImport` reads an unbounded response and throws outside the try on a bad
  IRI (`editor.js:692-713`: note only; `editor.js` is ticket 17's).
- The host's shadow copy splits lines on `\n` only (`lsp-host.js:9-20`).

**Files owned:** `web/engine/src/api.rs`, `web/app/query-pane.js`,
`web/app/lsp-worker.js`, `web/app/lsp-host.js`, `web/app/editor-lsp.js`,
`web/test/engine.test.mjs`, `web/test/host.test.mjs`, `web/test/editor-query.test.mjs`,
`web/test/lsp.test.mjs`.
**Hands off:** `web/app/editor.js`, `web/app/app.js`, `web/build.mjs` (ticket 17),
`src/**`, `.github/**` (ticket 08).

**Blocked by:** nothing. The worktree needs one `node web/build.mjs` (it builds the wasm
with cargo) before the web tests run; the Playwright files need Chrome.

**Status:** ready-for-agent

- [ ] `query`, `fix_all` and `analyse` accept a `settings` object parsed with the same
      deserializer as `turtle.*`; `query-pane.js` and `LspClient.fixAll` send
      `settings.values()`, including `query.timeoutSeconds` and `query.maxRows`.
- [ ] `timeout` via `Duration::try_from_secs_f64` → an error result; `maxRows` via
      `usize::try_from(..).unwrap_or(usize::MAX)`; 0 documented or rejected.
- [ ] `lsp-worker.js` drains its queue before each engine call: consecutive `didChange`s
      for one URI are merged into one `lsp` batch (`api.rs` already takes a message
      array); requests whose Monaco token was cancelled are dropped and a
      `$/cancelRequest` is posted.
- [ ] `onerror`/`onmessageerror` on both workers: reject pending, clear `inflight`,
      toast, recreate (the `drop()` pattern from `query-pane.js`).
- [ ] `splice` splits on `/\r\n?|\n/`; `didOpen` sends `model.getValue()`.
- [ ] Tests: `engine.test.mjs`: a settings-only prefix query succeeds; `timeout: 1e300`
      returns an error, not `EngineCrash`; `maxRows: 4294967296` returns all rows.
      `host.test.mjs`: 50 queued `didChange`s produce one publication and fewer engine
      calls. `lsp.test.mjs` parity unchanged. `editor-query.test.mjs`: a settings-only
      prefix.
