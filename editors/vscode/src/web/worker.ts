// The server worker of the browser entry: turtle-lsp's engine (engine.wasm,
// fetched from beside this script) behind the language client's JSON-RPC
// messages, which handler.ts handles. Bundled as a classic worker script.

import { createEngine } from "../../../../web/app/engine.js"
import { createHandler, type Engine } from "./handler.ts"

// The parts of a worker's global scope used here (the tsconfig has no WebWorker lib).
const scope = globalThis as unknown as {
  location: { href: string }
  postMessage(message: unknown): void
  onmessage: ((event: { data: unknown }) => void) | null
}

const engine: Promise<Engine> = fetch(new URL("engine.wasm", scope.location.href))
  .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`engine.wasm: HTTP ${r.status}`))))
  .then(createEngine)

const handler = createHandler(engine, (m) => scope.postMessage(m))
scope.onmessage = (event) => handler.receive(event.data)
