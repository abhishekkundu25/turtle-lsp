// The turtle-lsp language server inside the engine, spoken to in LSP. The
// engine runs one step of the server per call and returns what the server
// sent; this host keeps the protocol state: pending requests, published
// diagnostics, the server's timers, and the open documents, which it
// replays into a fresh server if the engine crashes.

export class EmbeddedServer {
  // `engine`: from createEngine. `on`: optional handlers, `diagnostics(uri,
  // list)`, `request(method, params)` (a server request; return its result),
  // `notification(method, params)`, `crash(error)` and `timers(pending)`
  // (whether a server timer is pending, after each change).
  constructor(engine, on = {}) {
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
      const full = params.contentChanges.findLast((c) => !c.range)
      if (doc && full && params.contentChanges.every((c) => !c.range)) {
        doc.text = full.text
        doc.version = params.textDocument.version
      }
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
      this.restart()
      for (const p of this.pending.values()) p.reject(e)
      this.pending.clear()
      this.on.crash?.(e)
      return
    }
    this.schedule(out.due)
    for (const m of out.messages) this.received(m)
  }

  // A fresh server with the open documents, after a crash.
  restart() {
    this.engine.call("lsp_start", { params: this.initParams })
    const replay = [{ jsonrpc: "2.0", method: "initialized", params: {} }]
    for (const [uri, d] of this.open) {
      replay.push({ jsonrpc: "2.0", method: "textDocument/didOpen", params: { textDocument: { uri, ...d } } })
    }
    const out = this.engine.call("lsp", { messages: replay })
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
