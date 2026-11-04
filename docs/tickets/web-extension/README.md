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
