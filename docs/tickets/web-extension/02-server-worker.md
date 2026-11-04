# 02: Server worker

Status: done. Blocked by: none.

## What to build

The web worker that runs the language server for the browser entry (spec:
"The server worker", "URIs", "Disk and editor documents"). Only new files.

- `src/web/uris.ts`, pure: given the workspace folders' URIs, map each to
  `file:///workspace/<n>/` and back. Applies to a whole protocol message:
  every string that starts with a mapped prefix, and object keys (the
  `changes` of a workspace edit). Untitled and unknown URIs pass through.
- `src/web/layers.ts`, pure: the disk and editor layers per URI and the
  messages to the server each event yields: client `didOpen`, `didChange`,
  `didClose`; disk `changed` and `deleted`; and `replay()` (the union, for a
  restart). The table in the spec decides each case. Versions sent to the
  server rise monotonically per URI.
- `src/web/worker.ts`: loads `engine.wasm` (fetched beside the worker
  script) with `web/app/engine.js`, drives it with `EmbeddedServer` and
  `plan` from `web/app/lsp-host.js` (imported, not copied), and speaks
  JSON-RPC over `postMessage` with the language client: requests answered
  under the client's ids, `$/cancelRequest` dropping queued requests,
  messages that arrive while the engine is busy batched into one call,
  server requests forwarded, notifications passed on, the custom
  notification `turtle/web/files` (`{ workspaceFolders?, changed: [{uri,
  text}], deleted: [uri] }`) handled by the layers and never sent to the
  server, every URI mapped both ways. An engine crash restarts with
  `replay()` and sends `window/showMessage` saying so.
- Keep the worker's protocol shell thin: the decisions live in the two pure
  modules.

## Acceptance

- [ ] `test/web-uris.test.mjs`: round trip of a hover, a definition, a workspace edit with `changes` and `documentChanges`, a diagnostics notification; untitled unchanged; two folders.
- [ ] `test/web-layers.test.mjs`: each row of the spec's table, a restart replay, a repeated `changed` with the same text yields nothing.
- [ ] `test/web-worker.test.mjs` (Node, real `web/app/engine.wasm` built by `node web/build.mjs`; the worker's message handler driven directly): initialize, diagnostics on open, hover, a cancelled queued request, a definition into a disk-only file, closing a file keeps it found.
- [ ] `npm run typecheck` passes; the Node tests above pass under `timeout 120`.
