# 03: Browser entry and build

Status: todo. Blocked by: 01, 02.

## What to build

The extension's browser entry (spec: "One extension, two entries",
"Workspace files", "Settings", "Run query").

- `src/web/extension.ts`: starts the worker from the extension's
  `out/web/` folder, builds a `LanguageClient` from
  `vscode-languageclient/browser` through the shared activation (01), with
  the same document selector widened to the workspace's schemes.
- Reads the workspace's RDF files (the desktop glob, `node_modules`
  excluded, at most 2,000 files and 50 MB) with `vscode.workspace.fs`,
  sends them as `turtle/web/files` with the workspace folders, and keeps
  them current with a file system watcher. Past a cap it shows one
  message saying how many files it read.
- `turtle-lsp.json` at the first root: read at start and on change, layered
  under the user's settings; a parse error is shown once.
- Drops the Run query lens in the client's middleware.
- `package.json`: `browser` entry, `virtualWorkspaces` supported
  `"limited"` with a description, and the `build` script bundling the
  browser entry (`--platform=browser`, webworker conditions) and the
  worker, and copying `web/app/engine.wasm` into `out/web/` when present.
- `test/manifest.test.mjs`: the browser entry exists after a build, the
  virtual workspace capability is limited, the browser glob equals the
  `workspaceContains` glob.

## Acceptance

- [ ] `npm run typecheck`, `npm run build` and `node --test test/*.test.mjs` pass.
- [ ] The headless desktop suite `node test/run.mjs` passes.
- [ ] A manual run with `npx @vscode/test-web --extensionDevelopmentPath=. <folder>` (not committed) shows diagnostics, hover and a cross-file definition; the report says what was checked.
