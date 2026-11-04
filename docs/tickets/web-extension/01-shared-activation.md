# 01: Shared activation

Status: todo. Blocked by: none.

## What to build

A refactor with no change in behaviour, so a browser entry can reuse the
desktop extension's activation (spec: "Shared activation").

- Move what `activate` does that is not about finding and starting the
  binary into a platform-neutral `src/activate.ts`: the restart and
  references commands, the graph command and `turtle.graphAvailable`, the
  start on the first served document, the settings sent to the server and
  the configuration middleware. It takes a factory that builds the
  language client (`BaseLanguageClient` from `vscode-languageclient`) from
  the shared client options.
- `src/extension.ts` keeps the desktop entry: finding the binary, the
  server options, the trust restart for `turtle.server.path`, and calls the
  shared activation with its factory. It still exports `activate`,
  `deactivate` and the `TurtleApi` the tests use.
- Move `ServerHandle`, `settingsFrom` and `configurationChange` from
  `src/server.ts` to a new `src/client.ts` that imports nothing from Node;
  `src/server.ts` keeps `findServer`.
- `src/graph.ts`: no Node import. The nonce comes from
  `crypto.getRandomValues`; the client type is the base client.
- Nothing in `activate.ts`, `client.ts` or `graph.ts` imports `node:*` or
  `vscode-languageclient/node`.

## Acceptance

- [ ] `npm run typecheck` and `npm run build` pass in `editors/vscode`.
- [ ] `node --test test/*.test.mjs` passes (imports of moved functions updated, no assertion removed).
- [ ] The headless desktop suite `node test/run.mjs` passes.
- [ ] `grep -n "node:" src/activate.ts src/client.ts src/graph.ts` finds nothing.
