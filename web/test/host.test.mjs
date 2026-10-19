// The page's LSP host (lsp-host.js) in Node with the real engine wrapped so
// that it crashes on demand: ranged edits are tracked, and after a crash a
// fresh server gets the open documents as they are now.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { EmbeddedServer } from "../app/lsp-host.js"

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
  const host = new EmbeddedServer(engine, { crash: (e) => crashes.push(e.message) })
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
  assert.match(crashes[0], /b\.ttl/)
  assert.deepEqual([...host.open.keys()], [a, c])
  await host.settle()
  assert.deepEqual(seen(a), before.a)
  assert.deepEqual(seen(c), before.c)
  assert.deepEqual(host.diagnostics(b), [])
  const fmt = await host.request("textDocument/formatting", { textDocument: { uri: c }, options: { tabSize: 4, insertSpaces: true } })
  assert.ok(Array.isArray(fmt))
  assert.equal(crashes.length, 1)
})
