# 05: Web .vsix and release

Status: done. Blocked by: 03.

## What to build

Packaging and publishing of the browser build (spec: "Packaging").

- `scripts/package.mjs`: `VSCE_TARGET=web` builds the engine
  (`node web/build.mjs`, engine only) instead of the binary, bundles the
  extension with `engine.wasm` in `out/web/`, and packages
  `turtle-lsp-web-<version>.vsix`. Other targets unchanged.
- `.vscodeignore`: the web `.vsix` holds `out/web/` with the `.wasm`, and
  the platform ones do not carry the `.wasm`.
- `.github/workflows/release.yml`: a job that builds the web `.vsix`
  (wasm target installed) and uploads it as a `vsix-web` artifact that the
  publish job publishes with the others, and the GitHub release lists.

## Acceptance

- [ ] `VSCE_TARGET=web node scripts/package.mjs` produces a `.vsix` under 8 MB whose listing (`unzip -l`) has `extension/out/web/extension.js`, the worker and `engine.wasm`, and no `server/` binary.
- [ ] A platform `.vsix` built locally has no `.wasm`.
- [ ] `release.yml` parses; the publish job's artifact pattern includes the web artifact.
