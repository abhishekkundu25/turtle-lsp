// The turtle-lsp language server inside the engine, spoken to in LSP. The
// engine runs one step of the server per call and returns what the server
// sent; this host keeps the protocol state: pending requests, published
// diagnostics, the server's timers, and the open documents, which it
// replays into a fresh server if the engine crashes.

// Where line `line` (0-based) starts in `text`, or -1 when `text` has fewer
// lines. LSP ends a line with \n, \r\n or a lone \r.
function lineStart(text, line) {
  if (line === 0) return 0
  const eol = /\r\n?|\n/g
  for (let l = 1, m; (m = eol.exec(text)); l++) if (l === line) return m.index + m[0].length
  return -1
}

// `text` with `range` (LSP: line and UTF-16 character, as JS strings count)
// replaced by `insert`.
function splice(text, range, insert) {
  const at = ({ line, character }) => {
    const i = lineStart(text, line)
    return i < 0 ? text.length : Math.min(text.length, i + character)
  }
  return text.slice(0, at(range.start)) + insert + text.slice(at(range.end))
}

// Two didChange notifications of one document, in order, as one.
function mergeChanges(a, b) {
  if (a.method !== "textDocument/didChange" || b.method !== "textDocument/didChange") return null
  if (a.params.textDocument.uri !== b.params.textDocument.uri) return null
  const changes = [...a.params.contentChanges, ...b.params.contentChanges]
  // A change with no range is the whole text: what came before it is gone.
  let from = 0
  changes.forEach((c, i) => c.range || (from = i))
  return { ...b, params: { ...b.params, contentChanges: changes.slice(from) } }
}

// What a worker does with the messages that came while it was busy: those
// whose request was cancelled (`{ kind: "cancel", id }`, which follows the
// request) are `dropped`, the rest are to `run`, in order.
export function plan(items) {
  const cancelled = new Set(items.filter((i) => i.kind === "cancel").map((i) => i.id))
  const run = []
  const dropped = []
  for (const item of items) {
    if (item.kind === "cancel") continue
    ;(item.id != null && cancelled.has(item.id) ? dropped : run).push(item)
  }
  return { run, dropped }
}

// Keeps `state` ({ open, config }) in step with what the page sent: the open
// documents as edited so far (uri -> { languageId, version, text }), and the
// last configuration. What a fresh server needs to take over.
export function track(state, method, params) {
  const uri = params?.textDocument?.uri
  if (method === "textDocument/didOpen") {
    const d = params.textDocument
    state.open.set(d.uri, { languageId: d.languageId, version: d.version, text: d.text })
  } else if (method === "textDocument/didChange") {
    const doc = state.open.get(uri)
    if (doc) {
      for (const c of params.contentChanges) doc.text = c.range ? splice(doc.text, c.range, c.text) : c.text
      doc.version = params.textDocument.version
    }
  } else if (method === "workspace/didChangeConfiguration") {
    state.config = params
  } else if (method === "textDocument/didClose") {
    state.open.delete(uri)
  }
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
    this.queued = null // the messages of a batch, while one is being made
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

  // Runs `f`, which sends messages (notify, request), and hands the server
  // all of them in one engine call, so a burst of edits costs one analysis:
  // consecutive didChange notifications of a document become one. The
  // answers to its requests are there when this returns.
  batch(f) {
    if (this.queued) return f()
    this.queued = []
    try {
      f()
    } finally {
      const messages = this.queued
      this.queued = null
      if (messages.length) {
        try {
          this.run("lsp", { messages })
        } catch (e) {
          // The engine refused the batch: its requests will not be answered.
          for (const p of this.pending.values()) p.reject(e)
          this.pending.clear()
          throw e
        }
      }
    }
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
    track(this, method, params)
    if (method === "textDocument/didClose") this.published.delete(params?.textDocument?.uri)
  }

  send(message) {
    if (!this.queued) return this.run("lsp", { messages: [message] })
    const merged = this.queued.length && !("id" in message) ? mergeChanges(this.queued.at(-1), message) : null
    if (merged) this.queued[this.queued.length - 1] = merged
    else this.queued.push(message)
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
