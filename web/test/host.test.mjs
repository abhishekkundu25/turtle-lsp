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
    call(op, payload) {
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
})
