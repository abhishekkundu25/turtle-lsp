// The turtle-lsp language server inside the engine, spoken to in LSP. The
// engine runs one step of the server per call and returns what the server
// sent; this host keeps the protocol state: pending requests, published
// diagnostics, the server's timers, and the open documents, which it
// replays into a fresh server if the engine crashes.

// `text` with `range` (LSP: line and UTF-16 character, as JS strings count)
// replaced by `insert`.
function splice(text, range, insert) {
  const at = ({ line, character }) => {
    let i = 0
    for (let l = 0; l < line; l++) {
      const n = text.indexOf("\n", i)
      if (n === -1) return text.length
      i = n + 1
    }
    return Math.min(text.length, i + character)
  }
  return text.slice(0, at(range.start)) + insert + text.slice(at(range.end))
}

const didOpen = ([uri, d]) => ({ jsonrpc: "2.0", method: "textDocument/didOpen", params: { textDocument: { uri, ...d } } })

export class EmbeddedServer {
  // `engine`: from createEngine. `on`: optional handlers, `diagnostics(uri,
  // list)`, `request(method, params)` (a server request; return its result),
  // `notification(method, params)`, `crash(error)` and `timers(pending)`
  // (whether a server timer is pending, after each change).
  constructor(engine, on = {}) {
    this.crashes = [] // when the engine crashed, the last minute
    this.engine = engine
    this.on = on
    this.nextId = 0
    this.pending = new Map()
    this.published = new Map()
    this.open = new Map() // uri -> { languageId, version, text }
    this.timer = null
    this.timerResolvers = []
    this.initParams = null
  }

  async initialize(params) {
    this.initParams = params
    const { messages } = this.engine.call("lsp_start", { params })
    const init = messages.find((m) => m.id === 0 && "result" in m)
    this.send({ jsonrpc: "2.0", method: "initialized", params: {} })
    return init?.result
  }

  // A request's answer. The server answers within the same step, so the
  // promise is settled by the time this returns.
  request(method, params) {
    const id = ++this.nextId
    const done = new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }))
    this.send({ jsonrpc: "2.0", id, method, params })
    return done
  }

  notify(method, params) {
    this.track(method, params)
    this.send({ jsonrpc: "2.0", method, params })
  }

  // The diagnostics last published for `uri`.
  diagnostics(uri) {
    return this.published.get(uri) ?? []
  }

  // Resolves when no timer of the server is pending (tests; idle checks).
  settle() {
    if (!this.timer) return Promise.resolve()
    return new Promise((r) => this.timerResolvers.push(r))
  }

  // ---- internals ----

  track(method, params) {
    const uri = params?.textDocument?.uri
    if (method === "textDocument/didOpen") {
      const d = params.textDocument
      this.open.set(d.uri, { languageId: d.languageId, version: d.version, text: d.text })
    } else if (method === "textDocument/didChange") {
      const doc = this.open.get(uri)
      if (doc) {
        for (const c of params.contentChanges) doc.text = c.range ? splice(doc.text, c.range, c.text) : c.text
        doc.version = params.textDocument.version
      }
    } else if (method === "workspace/didChangeConfiguration") {
      this.config = params
    } else if (method === "textDocument/didClose") {
      this.open.delete(uri)
      this.published.delete(uri)
    }
  }

  send(message) {
    this.run("lsp", { messages: [message] })
  }

  run(op, payload) {
    let out
    try {
      out = this.engine.call(op, payload)
    } catch (e) {
      if (!e.crashed) throw e
      // `state`: "restarted" (with every document), "dropped" (without the
      // documents in `dropped`, which crash it), or "off" (no documents).
      let error = Object.assign(new Error(e.message), { crashed: true, state: "restarted", dropped: [] })
      // Crashes close together mean restarting does not help (a timer that
      // crashes on every run, whatever is open): the third within a minute
      // leaves the server running with no documents until the page reloads.
      const now = Date.now()
      this.crashes = [...this.crashes.filter((t) => now - t < 60_000), now]
      try {
        if (this.crashes.length >= 3) {
          const dropped = [...this.open.keys()]
          this.open.clear()
          for (const uri of dropped) this.forget(uri)
          this.boot([])
          error = Object.assign(error, { state: "off", dropped })
        } else {
          const dropped = this.restart()
          if (dropped.length) error = Object.assign(error, { state: "dropped", dropped })
        }
      } catch (again) {
        error = Object.assign(new Error(`${e.message}; the language server could not be started again (${again.message})`), { crashed: true, state: "off", dropped: [...this.open.keys()] })
      }
      for (const p of this.pending.values()) p.reject(error)
      this.pending.clear()
      this.on.crash?.(error)
      return
    }
    this.schedule(out.due)
    for (const m of out.messages) this.received(m)
  }

  // A fresh server with the open documents, after a crash. If the replay
  // crashes too, the documents are opened one at a time and each one that
  // crashes the server is closed. Returns the URIs of those.
  restart() {
    const docs = [...this.open]
    try {
      this.boot(docs)
      return []
    } catch (e) {
      if (!e.crashed) throw e
    }
    const kept = []
    const dropped = []
    this.boot(kept)
    for (const doc of docs) {
      try {
        this.deliver([didOpen(doc)])
        kept.push(doc)
      } catch (e) {
        if (!e.crashed) throw e
        dropped.push(doc[0])
        this.boot(kept)
      }
    }
    for (const uri of dropped) (this.open.delete(uri), this.forget(uri))
    return dropped
  }

  // A document the server no longer has: its problems are cleared.
  forget(uri) {
    this.published.delete(uri)
    this.on.diagnostics?.(uri, [])
  }

  // Starts a server with the settings and `docs` ([uri, doc] pairs) open.
  boot(docs) {
    this.engine.call("lsp_start", { params: this.initParams })
    const replay = [{ jsonrpc: "2.0", method: "initialized", params: {} }]
    if (this.config) replay.push({ jsonrpc: "2.0", method: "workspace/didChangeConfiguration", params: this.config })
    this.deliver([...replay, ...docs.map(didOpen)])
  }

  // Sends `messages` straight to the engine; a crash is the caller's.
  deliver(messages) {
    const out = this.engine.call("lsp", { messages })
    this.schedule(out.due)
    for (const m of out.messages) this.received(m)
  }

  received(m) {
    if ("id" in m && !("method" in m)) {
      const p = this.pending.get(m.id)
      this.pending.delete(m.id)
      if (!p) return
      if (m.error) p.reject(Object.assign(new Error(m.error.message), { code: m.error.code }))
      else p.resolve(m.result ?? null)
    } else if ("id" in m) {
      // A request from the server (refreshes): answer it.
      let result = null
      try {
        result = this.on.request?.(m.method, m.params) ?? null
      } catch {}
      this.send({ jsonrpc: "2.0", id: m.id, result })
    } else if (m.method === "textDocument/publishDiagnostics") {
      this.published.set(m.params.uri, m.params.diagnostics)
      this.on.diagnostics?.(m.params.uri, m.params.diagnostics)
    } else {
      this.on.notification?.(m.method, m.params)
    }
  }

  // `due`: milliseconds until the server's next timer, or null.
  schedule(due) {
    clearTimeout(this.timer)
    this.timer = null
    if (due == null) {
      for (const r of this.timerResolvers.splice(0)) r()
      this.on.timers?.(false)
      return
    }
    this.timer = setTimeout(() => {
      this.timer = null
      this.run("lsp_tick", {})
      if (!this.timer) {
        for (const r of this.timerResolvers.splice(0)) r()
        this.on.timers?.(false)
      }
    }, Math.max(0, due))
    this.on.timers?.(true)
  }
}
