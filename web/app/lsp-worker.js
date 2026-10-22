// The editor's language server: turtle-lsp (engine.wasm) in this worker,
// spoken to in LSP through EmbeddedServer. Messages in:
//   { kind: "initialize" | "request" | "notify" | "fixAll", id, method, params }
//   { kind: "cancel", id }                     the page no longer wants request `id`
// The engine takes one step at a time and a step can take long (40 ms for
// each edit of a large file), so messages wait in a queue: whatever has
// come when the engine is free goes in as one batch (a burst of edits is
// one analysis), less the requests cancelled meanwhile. Out:
//   { type: "response", id, result | error }   to each message with an id
//   { type: "diagnostics", uri, list }         as the server publishes them
//   { type: "serverRequest", method, params }  refreshes the page acts on
//   { type: "idle", busy, ack }                after every message (ack: how many
//                                              were done) and when the server's
//                                              timers end
//   { type: "crash", message }                 the engine restarted
import { createEngine } from "./engine.js"
import { EmbeddedServer, plan } from "./lsp-host.js"

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

const queue = []
let pumping = false

self.onmessage = ({ data }) => {
  queue.push(data)
  pump()
}

// Handles the queue, a batch at a time, then gives the event loop a turn so
// the messages that came meanwhile can join the queue.
async function pump() {
  if (pumping) return
  pumping = true
  try {
    while (queue.length) {
      let loaded = null
      let failure
      try {
        loaded = await ready
      } catch (e) {
        failure = e
      }
      // Whatever came while the engine loaded is in this batch.
      const items = queue.splice(0)
      if (failure) {
        for (const { id } of items) if (id != null) post({ type: "response", id, error: String(failure?.message ?? failure) })
        post({ type: "idle", busy: false, ack: items.filter((i) => i.kind !== "cancel").length })
        continue
      }
      await handle(loaded, items)
      await new Promise((r) => setTimeout(r))
    }
  } finally {
    pumping = false
  }
}

async function handle({ engine, server }, items) {
  const { run, dropped } = plan(items)
  for (const { id } of dropped) post({ type: "response", id, error: "request cancelled" })
  const answers = []
  const attempt = (f) => {
    try {
      return Promise.resolve(f())
    } catch (e) {
      return Promise.reject(e)
    }
  }
  const reply = (id, f) => answers.push(attempt(f).then((result) => post({ type: "response", id, result }), (e) => post({ type: "response", id, error: String(e?.message ?? e) })))
  // Notifications and requests go to the server in one call; initialize and
  // fixAll run alone, in their place in the order.
  let batch = []
  const flush = () => {
    if (!batch.length) return
    const these = batch
    batch = []
    try {
      server.batch(() => {
        for (const { kind, id, method, params } of these) {
          if (kind === "notify") server.notify(method, params)
          else reply(id, () => server.request(method, params))
        }
      })
    } catch {
      // The requests were answered with the error.
    }
  }
  for (const item of run) {
    const { kind, id, params } = item
    if (kind === "notify" || kind === "request") batch.push(item)
    else {
      flush()
      if (kind === "initialize") reply(id, () => server.initialize(params))
      else if (kind === "fixAll") reply(id, () => engine.call("fix_all", { files: params.files, settings: params.settings }))
    }
  }
  flush()
  await Promise.all(answers)
  post({ type: "idle", busy: timersPending, ack: run.length + dropped.length })
}
