// The editor's language server: turtle-lsp (engine.wasm) in this worker,
// spoken to in LSP through EmbeddedServer. Messages in:
//   { kind: "initialize" | "request" | "notify" | "fixAll", id, method, params }
// out:
//   { type: "response", id, result | error }   to each message with an id
//   { type: "diagnostics", uri, list }         as the server publishes them
//   { type: "serverRequest", method, params }  refreshes the page acts on
//   { type: "idle", busy, ack }                after every message (ack) and
//                                              when the server's timers end
//   { type: "crash", message }                 the engine restarted
import { createEngine } from "./engine.js"
import { EmbeddedServer } from "./lsp-host.js"

const post = (m) => self.postMessage(m)
let timersPending = false

const ready = fetch(new URL("engine.wasm", import.meta.url))
  .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`engine.wasm: HTTP ${r.status}`))))
  .then(createEngine)
  .then((engine) => ({
    engine,
    server: new EmbeddedServer(engine, {
      diagnostics: (uri, list) => post({ type: "diagnostics", uri, list }),
      request: (method, params) => post({ type: "serverRequest", method, params }),
      crash: (e) => post({ type: "crash", message: e.message, state: e.state, dropped: e.dropped }),
      timers: (pending) => {
        timersPending = pending
        if (!pending) post({ type: "idle", busy: false, ack: false })
      },
    }),
  }))

self.onmessage = async ({ data: { kind, id, method, params } }) => {
  let reply
  try {
    const { engine, server } = await ready
    let result = null
    if (kind === "initialize") result = await server.initialize(params)
    else if (kind === "request") result = await server.request(method, params)
    else if (kind === "notify") server.notify(method, params)
    else if (kind === "fixAll") result = engine.call("fix_all", { files: params.files })
    reply = { type: "response", id, result }
  } catch (e) {
    reply = { type: "response", id, error: String(e?.message ?? e) }
  }
  if (id != null) post(reply)
  post({ type: "idle", busy: timersPending, ack: true })
}
