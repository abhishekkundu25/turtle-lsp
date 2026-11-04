# Tickets: VS Code for the Web (#52)

Spec: `docs/specs/web-extension.md`. Paths below are under `editors/vscode/`
unless they start with `.github/` or `docs/`. Lock files are in **bold**.

| # | Ticket | Blocked by | Files |
|---|---|---|---|
| 01 | [Shared activation](01-shared-activation.md) | — | `src/extension.ts`, `src/activate.ts` (new), `src/server.ts`, `src/client.ts` (new), `src/graph.ts`, `test/find-server.test.mjs` |
| 02 | [Server worker](02-server-worker.md) | — | `src/web/worker.ts`, `src/web/uris.ts`, `src/web/layers.ts` (all new), `test/web-uris.test.mjs`, `test/web-layers.test.mjs`, `test/web-worker.test.mjs` (new) |
| 03 | [Browser entry and build](03-browser-entry.md) | 01, 02 | `src/web/extension.ts` (new), **`package.json`** (`browser`, `virtualWorkspaces`, `build` script), `test/manifest.test.mjs` |
| 04 | [Headless web test in CI](04-headless-web-test.md) | 03 | `test/web/` (new), `test/fixtures/web/` (new), **`package.json`** (one devDependency, one script), **`package-lock.json`**, `.github/workflows/ci.yml` |
| 05 | [Web .vsix and release](05-web-vsix.md) | 03 | `scripts/package.mjs`, `.vscodeignore`, `.github/workflows/release.yml` |
| 06 | [Close-out](06-close-out.md) | 04, 05 | `README.md`, `editors/vscode/README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/specs/web-extension.md`, deletes `docs/tickets/web-extension/` |

## Waves

1. **01, 02** together: disjoint files (01 the existing `src/*.ts`, 02 only new files under `src/web/` and new tests).
2. **03** alone: it joins both and owns `package.json`.
3. **04, 05** together: only 04 edits `package.json` and `package-lock.json`; 05 edits the packaging script, `.vscodeignore` and the release workflow; their workflows are different files.
4. **06** alone: the only ticket that edits README, CHANGELOG, ARCHITECTURE and the spec.

## Log

Departures from the plan, as they happen.

- 01: also changed one import line in `test/manifest.test.mjs` (owned by 03), since `settingsFrom` moved to `src/client.ts`; a re-export would only forward. "Served" is now `vscode.languages.match` against the document selector, so a wider browser selector widens the start-on-open and the graph refresh too. Open for 03: layered `turtle-lsp.json` on configuration changes needs a settings hook in `ClientSetup`; the graph panel's `reveal()` opens only `file:` and `untitled:` URIs; the RDF glob is written out in `activate.ts` and `graph.ts`.
- 02: the protocol code is in `src/web/handler.ts` (testable without a Worker), beside the 22-line `worker.ts`. Sent back for two root-cause fixes: `tsconfig.json` allows `.ts` and JS imports instead of `@ts-ignore`; `Embedded::due_in_ms` returns `None` once the server has ended (it kept a timer due after `shutdown`, so the host ticked forever) and `EmbeddedServer` gains `stop()`, instead of the handler calling the host's internal `schedule(null)`. This adds `src/server.rs`, `web/app/lsp-host.js` and `web/test/host.test.mjs` to 02's files; the spec's budget line now names the one Rust change.
- Follow-ups found so far (not in scope): `src/web/handler.ts` repeats the queue and batching of `web/app/lsp-worker.js` in JSON-RPC form, and one shared queue module in `web/app` could serve both; after a crash that ends "dropped" or "off", the layers still count those documents as on the server; disk files open with languageId "", so a language mode chosen later in the editor does not reach the server; the wasm build warns about thread-only code of `turtle/query` (#74), already on main.
