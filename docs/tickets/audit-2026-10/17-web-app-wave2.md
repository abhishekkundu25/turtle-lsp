# 17: Web app wave 2

**What to fix:** the editor and toolbox pages (`web/app/editor.js` 1,322 lines,
`web/app/app.js`).

- Two tabs overwrite each other's workspace: each loads the single IndexedDB key
  `"current"` once and rewrites the whole workspace 300 ms after any change, including a
  cursor move and every diagnostics publish (`store.js:31-42`, `editor.js:203-237, 433,
  1219, 1244`). `db()` opens a connection per call and never closes it.
- Fix All / Format All apply results computed for an older text over newer edits, with
  no undo in the toolbox (`app.js:467-478`, `editor.js:742-765`; `diff.js:270-291` has
  the right pattern).
- Toolbox `problemHtml` re-splits the whole file per problem: ~288 ms per 1,000
  problems on 500 KB, 20,000 diagnostics → ~6 s freeze per analysis; no cap
  (`app.js:345-351`). Every `input` event rebuilds the highlighted HTML of the whole
  file synchronously (`:571-578`); stale analyses are computed then discarded (`:63-91`).
- No CSP meta on either page; 30+ `innerHTML` sinks, all escaped today
  (`index.html`, `toolbox.html`).
- The wasm is 9.6 MB (2.75 MB gz) with no size budget, no `wasm-opt`; `build.mjs` copies
  7.6 MB of unused Monaco language workers; each worker compiles the module again.
- Seven copies of `esc()`, two static-file servers (one in `serve.mjs`, one in nine
  tests), two file-walker copies with different limits, two position helpers.
- `web/prototype/` (424 KB) is dead; `web:prototype` still serves it.
- Browser tests wait with fixed sleeps tied to `SAVE_DELAY_MS`.
- `fetchImport` reads an unbounded response and throws outside its try on a bad IRI
  (`editor.js:692-713`).

**Files owned:** `web/app/editor.js` (split: new `web/app/workspace-store.js`,
`web/app/paths.js`, `web/app/files.js`), `web/app/store.js`, `web/app/app.js`, new
`web/app/dom.js`, `web/app/text.js`, `web/test/static-server.mjs`, `web/serve.mjs`,
`web/build.mjs`, `web/app/index.html`, `web/app/toolbox.html`, `web/app/{diff,settings,
search,hierarchy}.js` (esc call sites), `web/test/*.test.mjs` (except the four owned by
ticket 09), `web/prototype/` (delete), `package.json` (`web:prototype` script removal),
`docs/specs/web-*.md` (the three prototype references).
**Hands off:** `web/engine/**`, `web/app/{lsp-worker,lsp-host,editor-lsp,query-pane}.js`
(ticket 09), `src/**`.

**Blocked by:** ticket 09; ticket 11 (`package.json`).

**Status:** ready-for-agent

- [ ] `workspace-store.js`: per-file records with a revision, one cached connection
      closed on `versionchange`, a `navigator.locks` (or BroadcastChannel) owner so a
      second tab is read-only with a notice; persist only on content/structure changes.
- [ ] Version checks before applying Fix All / Format All results (editor
      `getVersionId`, toolbox text snapshot); skip or re-run changed files.
- [ ] Toolbox: `lineStarts` once per file, problem list capped (500 + "show more"), one
      analysis in flight plus one pending snapshot, highlight only the visible window.
- [ ] CSP `<meta http-equiv="Content-Security-Policy">` on both pages (`script-src
      'self'` + hashes for inline scripts, `worker-src 'self'`, `object-src 'none'`,
      `base-uri 'none'`, `connect-src` for the fetch quick fix); Monaco verified to run.
- [ ] `build.mjs` skips `vs/language/{typescript,css,html}`, prints gzip size, fails
      over a budget; copies Monaco `LICENSE` and `ThirdPartyNotices.txt`.
- [ ] One `esc` in `dom.js`; one static server in `web/test/static-server.mjs` used by
      `serve.mjs` (with a stream error handler); shared `files.js` walkers with one set of
      limits; `text.js` position helpers. `grep -c "const esc =" web/app` is 1.
- [ ] `fetchImport` caps by `content-length`/stream and wraps `new URL` in the try.
- [ ] Tests wait on a `document.body.dataset.saved` signal, not sleeps; a two-tab test;
      a 5,000-diagnostic toolbox render under 500 ms. Delete `web/prototype/` and the
      script; update the three spec references. `web/test` green.
