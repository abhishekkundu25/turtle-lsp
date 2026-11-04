// The server worker of the browser entry: turtle-lsp's engine (engine.wasm)
// behind the language client's JSON-RPC messages, which handler.ts handles.
// Bundled as a classic worker script.
//
// The first message is the engine's URL (a string), from the browser entry:
// VS Code for the Web runs an extension's worker as a blob: worker that
// imports this script, so the script's own URL is nowhere in scope.

import { createEngine } from "../../../../web/app/engine.js"
import { createHandler, type Engine, type Handler } from "./handler.ts"

// The parts of a worker's global scope used here (the tsconfig has no WebWorker lib).
const scope = globalThis as unknown as {
  postMessage(message: unknown): void
  onmessage: ((event: { data: unknown }) => void) | null
}

const load = (url: string): Promise<Engine> =>
  fetch(url)
    .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`engine.wasm: HTTP ${r.status}`))))
    .then(createEngine)

let handler: Handler | undefined
scope.onmessage = (event) => {
  if (handler) handler.receive(event.data)
  else handler = createHandler(typeof event.data === "string" ? load(event.data) : Promise.reject(new Error("no engine URL")), (m) => scope.postMessage(m))
}
