// The page's LSP host (lsp-host.js) in Node with the real engine wrapped so
// that it crashes on demand: ranged edits are tracked, and after a crash a
// fresh server gets the open documents as they are now.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { EmbeddedServer, plan } from "../app/lsp-host.js"
import { LspClient, currentSettings } from "../app/editor-lsp.js"

const wasm = fs.readFileSync(path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "app", "engine.wasm"))
const URI = "file:///work/a.ttl"
const TEXT = "@prefix ex: <https://e.org/> .\n\nex:a ex:b ex:c .\n"

test("ranged edits are kept, and a crash restarts the server with the documents as edited", async () => {
  const real = await createEngine(wasm)
  let crashNext = false
  const engine = {
    calls: [],
    call(op, payload) {
      this.calls.push({ op, payload })
      if (crashNext && op === "lsp") {
        crashNext = false
        throw Object.assign(new Error("unreachable"), { crashed: true })
      }
      return real.call(op, payload)
    },
  }
  const crashes = []
  const host = new EmbeddedServer(engine, { crash: (e) => crashes.push(e.message) })
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  host.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: "all" } } } })
  host.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: TEXT } })
  // Insert a broken statement at line 3, then fix a word on line 2 by range.
  host.notify("textDocument/didChange", {
    textDocument: { uri: URI, version: 2 },
    contentChanges: [{ range: { start: { line: 3, character: 0 }, end: { line: 3, character: 0 } }, text: "ex:x ex:y .\n" }],
  })
  host.notify("textDocument/didChange", {
    textDocument: { uri: URI, version: 3 },
    contentChanges: [{ range: { start: { line: 2, character: 8 }, end: { line: 2, character: 14 } }, text: "ex:dd" }],
  })
  const want = "@prefix ex: <https://e.org/> .\n\nex:a ex:ex:dd .\nex:x ex:y .\n"
  assert.equal(host.open.get(URI).text, want)
  await host.settle()
  const before = host.diagnostics(URI).map((d) => [d.range.start.line, d.message])

  crashNext = true
  await assert.rejects(host.request("textDocument/hover", { textDocument: { uri: URI }, position: { line: 0, character: 1 } }), /unreachable/)
  assert.equal(crashes.length, 1)
  await host.settle()
  // The fresh server has the edited text: same diagnostics, and answers again.
  assert.deepEqual(host.diagnostics(URI).map((d) => [d.range.start.line, d.message]), before)
  assert.ok(before.some(([line]) => line === 3), JSON.stringify(before))
  const fmt = await host.request("textDocument/formatting", { textDocument: { uri: URI }, options: { tabSize: 4, insertSpaces: true } })
  assert.ok(Array.isArray(fmt))
  // The settings came back too.
  assert.deepEqual(host.config, { settings: { turtle: { inlayHints: { labels: "all" } } } })
  assert.equal(engine.calls.filter((c) => c.op === "lsp" && c.payload.messages.some((m) => m.method === "workspace/didChangeConfiguration")).length, 2)
})

test("a document that crashes the replay too is left out, and the server keeps working for the others", async () => {
  const real = await createEngine(wasm)
  let crashNext = false
  // Once armed, opening a document that holds POISON crashes the engine.
  let armed = false
  const poisoned = (messages) =>
    messages.some((m) => m.method === "textDocument/didOpen" && m.params.textDocument.text.includes("POISON"))
  const engine = {
    call(op, payload) {
      if (op === "lsp" && (crashNext || (armed && poisoned(payload.messages)))) {
        crashNext = false
        throw Object.assign(new Error("unreachable"), { crashed: true })
      }
      return real.call(op, payload)
    },
  }
  const crashes = []
  const host = new EmbeddedServer(engine, { crash: (e) => crashes.push(e) })
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  const [a, b, c] = ["a", "b", "c"].map((n) => `file:///work/${n}.ttl`)
  host.notify("textDocument/didOpen", { textDocument: { uri: a, languageId: "turtle", version: 1, text: TEXT } })
  host.notify("textDocument/didOpen", { textDocument: { uri: b, languageId: "turtle", version: 1, text: "# POISON\n" + TEXT } })
  host.notify("textDocument/didOpen", { textDocument: { uri: c, languageId: "turtle", version: 1, text: "ex:x ex:y .\n" } })
  await host.settle()
  // Where and what, not the data (it counts the files using a namespace).
  const seen = (uri) => host.diagnostics(uri).map((d) => [d.range.start.line, d.message])
  const before = { a: seen(a), c: seen(c) }
  assert.ok(before.c.length > 0)

  armed = true
  crashNext = true
  await assert.rejects(host.request("textDocument/hover", { textDocument: { uri: a }, position: { line: 0, character: 1 } }), /unreachable/)
  assert.equal(crashes.length, 1)
  assert.equal(crashes[0].state, "dropped")
  assert.deepEqual(crashes[0].dropped, [b])
  assert.deepEqual([...host.open.keys()], [a, c])
  await host.settle()
  assert.deepEqual(seen(a), before.a)
  assert.deepEqual(seen(c), before.c)
  assert.deepEqual(host.diagnostics(b), [])
  const fmt = await host.request("textDocument/formatting", { textDocument: { uri: c }, options: { tabSize: 4, insertSpaces: true } })
  assert.ok(Array.isArray(fmt))
  assert.equal(crashes.length, 1)
})

test("crashes that a restart does not stop (a timer) turn the server off instead of looping", async () => {
  const real = await createEngine(wasm)
  // Once armed, every timer step crashes, whatever is open, as a SHACL run
  // that panics after each pause in typing would.
  let armed = false
  let ticks = 0
  const engine = {
    call(op, payload) {
      if (op === "lsp_tick") ticks++
      if (armed && op === "lsp_tick") throw Object.assign(new Error("panic in a timer"), { crashed: true })
      return real.call(op, payload)
    },
  }
  const crashes = []
  const host = new EmbeddedServer(engine, { crash: (e) => crashes.push(e) })
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  host.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: TEXT } })
  await host.settle()
  armed = true
  for (let i = 0; i < 5; i++) host.run("lsp_tick", {})
  assert.deepEqual(crashes.map((e) => e.state), ["restarted", "restarted", "off", "off", "off"].slice(0, crashes.length))
  assert.equal(crashes[2].state, "off")
  assert.deepEqual(crashes[2].dropped, [URI])
  // Off: nothing is open, so no timer is due and nothing crashes on its own.
  assert.equal(host.open.size, 0)
  const before = ticks
  await new Promise((r) => setTimeout(r, 1500))
  assert.equal(ticks, before, "no timer runs while the server is off")
  // It still answers.
  assert.ok(await host.request("textDocument/hover", { textDocument: { uri: URI }, position: { line: 2, character: 1 } }).then(() => true, () => true))
})

// ---- a burst of edits is one analysis ----------------------------------------------

// The real engine, counting its calls.
async function counted() {
  const real = await createEngine(wasm)
  const engine = { calls: [], call: (op, payload) => (engine.calls.push(op), real.call(op, payload)) }
  return engine
}
const edit = (i) => ({
  textDocument: { uri: URI, version: 2 + i },
  contentChanges: [{ range: { start: { line: 3 + i, character: 0 }, end: { line: 3 + i, character: 0 } }, text: `ex:s${i} nope:p ex:o${i} .\n` }],
})

test("50 queued edits go to the engine in one call and publish as one edit does", async () => {
  const run = async (burst, n = 50) => {
    const engine = await counted()
    const published = []
    const host = new EmbeddedServer(engine, { diagnostics: (uri, list) => published.push(list.length) })
    await host.initialize({ processId: null, rootUri: null, capabilities: {} })
    host.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: TEXT } })
    await host.settle()
    const before = { calls: engine.calls.filter((op) => op === "lsp").length, published: published.length }
    const send = () => {
      for (let i = 0; i < n; i++) host.notify("textDocument/didChange", edit(i))
    }
    if (burst) host.batch(send)
    else send()
    await host.settle()
    return {
      calls: engine.calls.filter((op) => op === "lsp").length - before.calls,
      published: published.length - before.published,
      text: host.open.get(URI).text,
      version: host.open.get(URI).version,
      diagnostics: host.diagnostics(URI).map((d) => [d.range.start.line, d.message]),
    }
  }
  const one = await run(true)
  const each = await run(false)
  const single = await run(false, 1)
  assert.equal(one.calls, 1)
  assert.equal(each.calls, 50)
  // As many publications as one edit gives (the step's, then the timer's),
  // where an engine call for each edit publishes after each.
  assert.equal(one.published, single.published)
  assert.ok(each.published >= 50, `${each.published}`)
  // The same document and the same problems either way.
  assert.equal(one.text, each.text)
  assert.equal(one.version, 51)
  assert.deepEqual(one.diagnostics, each.diagnostics)
  assert.equal(one.diagnostics.length, 50)
})

test("a batch keeps the order of its messages, and a full text drops the edits before it", async () => {
  const engine = await counted()
  const sent = []
  const host = new EmbeddedServer({ call: (op, p) => (sent.push(p), engine.call(op, p)) })
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  host.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: TEXT } })
  const other = "file:///work/b.ttl"
  host.notify("textDocument/didOpen", { textDocument: { uri: other, languageId: "turtle", version: 1, text: TEXT } })
  sent.length = 0
  let hover
  host.batch(() => {
    host.notify("textDocument/didChange", edit(0))
    host.notify("textDocument/didChange", edit(1))
    host.notify("textDocument/didChange", { textDocument: { uri: URI, version: 4 }, contentChanges: [{ text: "# all new\n" }] })
    host.notify("textDocument/didChange", { textDocument: { uri: other, version: 2 }, contentChanges: [{ text: "# b\n" }] })
    host.notify("textDocument/didChange", { textDocument: { uri: URI, version: 5 }, contentChanges: [{ range: { start: { line: 0, character: 2 }, end: { line: 0, character: 5 } }, text: "OLD" }] })
    hover = host.request("textDocument/hover", { textDocument: { uri: URI }, position: { line: 0, character: 1 } })
  })
  await hover
  assert.equal(sent.length, 1)
  const methods = sent[0].messages.map((m) => [m.method, m.params?.textDocument?.uri, m.params?.textDocument?.version])
  assert.deepEqual(methods, [
    ["textDocument/didChange", URI, 4],
    ["textDocument/didChange", other, 2],
    ["textDocument/didChange", URI, 5],
    ["textDocument/hover", URI, undefined],
  ])
  assert.deepEqual(sent[0].messages[0].params.contentChanges, [{ text: "# all new\n" }])
  assert.equal(host.open.get(URI).text, "# OLD new\n")
  assert.equal(host.open.get(other).text, "# b\n")
})

test("a batch that the engine refuses fails its requests instead of leaving them waiting", async () => {
  const real = await createEngine(wasm)
  const refuse = (op, p) => {
    if (op === "lsp" && p.messages.some((m) => m.method === "textDocument/hover")) throw new Error("bad LSP messages")
    return real.call(op, p)
  }
  const host = new EmbeddedServer({ call: refuse })
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  let hover
  assert.throws(() => host.batch(() => (hover = host.request("textDocument/hover", { textDocument: { uri: URI }, position: { line: 0, character: 0 } }))), /bad LSP messages/)
  await assert.rejects(hover, /bad LSP messages/)
})

test("a document's lines end with \\n, \\r\\n or \\r when edits are applied to the host's copy", async () => {
  const engine = await counted()
  const host = new EmbeddedServer(engine)
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  const text = "# a\r\n# b\r# c\n# d\r\n"
  host.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text } })
  const at = (line, character) => ({ line, character })
  const change = (version, start, end, insert) =>
    host.notify("textDocument/didChange", { textDocument: { uri: URI, version }, contentChanges: [{ range: { start, end }, text: insert }] })
  change(2, at(1, 2), at(1, 3), "B")
  change(3, at(2, 2), at(2, 3), "C")
  change(4, at(3, 2), at(3, 3), "D")
  assert.equal(host.open.get(URI).text, "# a\r\n# B\r# C\n# D\r\n")
})

test("settings, the open documents and a restart: a burst before a crash is replayed as edited", async () => {
  const real = await createEngine(wasm)
  let crashNext = false
  const engine = {
    call(op, payload) {
      if (crashNext && op === "lsp") {
        crashNext = false
        throw Object.assign(new Error("unreachable"), { crashed: true })
      }
      return real.call(op, payload)
    },
  }
  const crashes = []
  const host = new EmbeddedServer(engine, { crash: (e) => crashes.push(e.state) })
  await host.initialize({ processId: null, rootUri: null, capabilities: {} })
  host.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: TEXT } })
  crashNext = true
  let hover
  host.batch(() => {
    for (let i = 0; i < 5; i++) host.notify("textDocument/didChange", edit(i))
    hover = host.request("textDocument/hover", { textDocument: { uri: URI }, position: { line: 0, character: 1 } })
  })
  await assert.rejects(hover, /unreachable/)
  assert.deepEqual(crashes, ["restarted"])
  await host.settle()
  assert.equal(host.diagnostics(URI).length, 5)
})

// ---- the worker: what waits is handled as one batch ------------------------------------

test("plan: a cancelled request is dropped, a cancel for one already answered is ignored", () => {
  const items = [
    { kind: "request", id: 1 },
    { kind: "notify", method: "m" },
    { kind: "request", id: 2 },
    { kind: "cancel", id: 1 },
    { kind: "cancel", id: 99 },
    { kind: "fixAll", id: 3 },
  ]
  const { run, dropped } = plan(items)
  assert.deepEqual(dropped.map((i) => i.id), [1])
  assert.deepEqual(run.map((i) => i.id ?? i.method), ["m", 2, 3])
})

test("the worker takes what has queued as one batch: edits merge, cancelled requests are skipped, each message is acknowledged", async () => {
  const posted = []
  globalThis.self = { postMessage: (m) => posted.push(m) }
  const realFetch = globalThis.fetch
  globalThis.fetch = async () => ({ ok: true, arrayBuffer: async () => wasm })
  try {
    await import("../app/lsp-worker.js")
    const send = (m) => self.onmessage({ data: m })
    const acked = () => posted.reduce((n, m) => n + (m.type === "idle" && m.ack ? Number(m.ack) : 0), 0)
    const until = async (f) => {
      for (let i = 0; i < 500 && !f(); i++) await new Promise((r) => setTimeout(r, 10))
      assert.ok(f(), JSON.stringify(posted.slice(-3)).slice(0, 300))
    }
    send({ kind: "initialize", id: 1, params: { processId: null, rootUri: null, capabilities: {} } })
    send({ kind: "notify", method: "textDocument/didOpen", params: { textDocument: { uri: URI, languageId: "turtle", version: 1, text: TEXT } } })
    await until(() => acked() === 2)
    assert.ok(posted.some((m) => m.type === "response" && m.id === 1 && m.result.capabilities))
    posted.length = 0
    // Everything arrives while the engine is busy: 50 edits, two requests,
    // and the page gives up on the first.
    for (let i = 0; i < 50; i++) send({ kind: "notify", method: "textDocument/didChange", params: edit(i) })
    const hover = { textDocument: { uri: URI }, position: { line: 0, character: 1 } }
    send({ kind: "request", id: 2, method: "textDocument/hover", params: hover })
    send({ kind: "request", id: 3, method: "textDocument/hover", params: hover })
    send({ kind: "cancel", id: 2 })
    await until(() => acked() === 52)
    const answer = (id) => posted.find((m) => m.type === "response" && m.id === id)
    assert.equal(answer(2).error, "request cancelled")
    assert.equal(answer(3).error, undefined)
    // The edits were one analysis: its few publications, not 51.
    await until(() => posted.findLast((m) => m.type === "idle")?.busy === false)
    const published = posted.filter((m) => m.type === "diagnostics" && m.uri === URI)
    assert.ok(published.length > 0 && published.length <= 3, String(published.length))
    assert.equal(published.at(-1).list.length, 50)
    // Fix All runs apart from the server, with the settings it is given.
    posted.length = 0
    send({ kind: "fixAll", id: 4, params: { files: [{ name: "b.ttl", text: "foaf:a foaf:b foaf:c .\n" }], settings: { "turtle.prefixes": { foaf: "https://e.org/foaf#" } } } })
    await until(() => acked() === 1)
    assert.equal(answer(4).result.applied, 0)
  } finally {
    delete globalThis.self
    globalThis.fetch = realFetch
  }
})

// ---- LspClient -------------------------------------------------------------------------

class FakeWorker {
  static all = []
  constructor(url, options) {
    Object.assign(this, { url, options, sent: [], terminated: false })
    FakeWorker.all.push(this)
  }
  postMessage(m) {
    this.sent.push(m)
  }
  terminate() {
    this.terminated = true
  }
}
async function fakeClient() {
  FakeWorker.all = []
  globalThis.Worker = FakeWorker
  const client = new LspClient("worker.js")
  const events = { crash: [], busy: [] }
  client.on("crash", (message, info) => events.crash.push({ message, ...info }))
  client.on("busy", (b) => events.busy.push(b))
  return { client, events, worker: () => FakeWorker.all.at(-1) }
}
const settled = (p) => p.then(() => "resolved", (e) => e)
const die = (w, message = "boom") => w.onerror({ message, preventDefault() {} })

test("a worker that dies fails what waits on it, and a new one takes over with the documents as edited", async () => {
  const { client, events, worker } = await fakeClient()
  const first = worker()
  const init = client.initialize({ processId: null, capabilities: {} })
  first.onmessage({ data: { type: "response", id: 1, result: { capabilities: {} } } })
  await init
  client.notify("workspace/didChangeConfiguration", { settings: { turtle: { prefixes: { zz: "https://e.org/" } } } })
  client.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: "a\r\nb\n" } })
  client.notify("textDocument/didChange", {
    textDocument: { uri: URI, version: 2 },
    contentChanges: [{ range: { start: { line: 1, character: 0 }, end: { line: 1, character: 1 } }, text: "B" }],
  })
  const hover = client.request("textDocument/hover", { textDocument: { uri: URI } })
  assert.ok(client.busy)
  die(first)
  assert.match((await settled(hover)).message, /the language server stopped: boom/)
  assert.equal(first.terminated, true)
  // Only what the new worker was sent is waited for.
  assert.equal(client.inflight, 3)
  assert.deepEqual(events.busy.slice(-4), [false, true, true, true])
  assert.equal(events.crash.length, 1)
  assert.equal(events.crash[0].state, "restarted")
  assert.match(events.crash[0].message, /boom/)
  // The new worker is started like the first, and has what the page had sent.
  const second = worker()
  assert.notEqual(second, first)
  assert.deepEqual(second.sent.map((m) => [m.kind, m.method]), [
    ["initialize", undefined],
    ["notify", "workspace/didChangeConfiguration"],
    ["notify", "textDocument/didOpen"],
  ])
  assert.deepEqual(second.sent[0].params, { processId: null, capabilities: {} })
  assert.equal(second.sent[2].params.textDocument.text, "a\r\nB\n")
  assert.equal(second.sent[2].params.textDocument.version, 2)
  // Late words from the dead worker are not heard; the new one is.
  first.onmessage({ data: { type: "idle", busy: false, ack: 9 } })
  assert.equal(client.inflight, 3)
  second.onmessage({ data: { type: "idle", busy: false, ack: 3 } })
  assert.equal(client.inflight, 0)
  const again = client.request("textDocument/hover", { textDocument: { uri: URI } })
  second.onmessage({ data: { type: "response", id: client.nextId, result: null } })
  assert.equal(await again, null)
})

test("a message the worker cannot read is a death too, and the third in a minute leaves the language server off", async () => {
  const { client, events, worker } = await fakeClient()
  client.initialize({ processId: null }).catch(() => {})
  worker().onmessageerror({ preventDefault() {} })
  assert.deepEqual(events.crash.map((c) => c.state), ["restarted"])
  die(worker())
  assert.deepEqual(events.crash.map((c) => c.state), ["restarted", "restarted"])
  const count = FakeWorker.all.length
  client.notify("textDocument/didOpen", { textDocument: { uri: URI, languageId: "turtle", version: 1, text: "x" } })
  die(worker())
  assert.deepEqual(events.crash.map((c) => c.state), ["restarted", "restarted", "off"])
  assert.deepEqual(events.crash[2].dropped, [URI])
  assert.equal(FakeWorker.all.length, count)
  assert.equal(client.inflight, 0)
  assert.match((await settled(client.request("textDocument/hover", {}))).message, /is off/)
  client.notify("textDocument/didChange", { textDocument: { uri: URI, version: 2 }, contentChanges: [{ text: "y" }] })
  assert.equal(client.inflight, 0)
})

test("a worker that fails to load before it was started is off, with nothing to start again", async () => {
  const { client, events, worker } = await fakeClient()
  die(worker(), "no such file")
  assert.deepEqual(events.crash.map((c) => c.state), ["off"])
  assert.equal(FakeWorker.all.length, 1)
})

test("a request whose token is cancelled fails with Canceled at once and tells the worker; an answer that comes after is ignored", async () => {
  const { client, worker } = await fakeClient()
  const token = (cancelled = false) => ({
    isCancellationRequested: cancelled,
    disposed: false,
    onCancellationRequested(f) {
      this.cancel = f
      return { dispose: () => (this.disposed = true) }
    },
  })
  const t = token()
  const p = client.request("textDocument/hover", {}, t)
  const id = client.nextId
  assert.equal(t.disposed, false)
  t.cancel()
  const e = await settled(p)
  assert.equal(e.name, "Canceled")
  assert.deepEqual(worker().sent.at(-1), { kind: "cancel", id })
  worker().onmessage({ data: { type: "response", id, error: "request cancelled" } })
  worker().onmessage({ data: { type: "idle", busy: false, ack: 1 } })
  assert.equal(client.inflight, 0)
  // Cancelled before it was sent: nothing is.
  const sent = worker().sent.length
  assert.equal((await settled(client.request("textDocument/hover", {}, token(true)))).name, "Canceled")
  assert.equal(worker().sent.length, sent)
  // Answered: the token is let go.
  const t2 = token()
  const q = client.request("textDocument/hover", {}, t2)
  worker().onmessage({ data: { type: "response", id: client.nextId, result: 5 } })
  assert.equal(await q, 5)
  assert.equal(t2.disposed, true)
})

test("Fix All is asked with the settings last sent to the server", async () => {
  const { client, worker } = await fakeClient()
  const settings = { turtle: { format: { lineWidth: 60 } } }
  client.notify("workspace/didChangeConfiguration", { settings })
  assert.deepEqual(currentSettings(), settings)
  const files = [{ name: "a.ttl", text: "" }]
  client.fixAll(files).catch(() => {})
  assert.deepEqual(worker().sent.at(-1), { kind: "fixAll", params: { files, settings }, id: client.nextId })
})
