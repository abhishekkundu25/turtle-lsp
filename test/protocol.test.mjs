// Protocol behaviour that depends on client capabilities: pull diagnostics,
// cancellation, didSave and configuration changes.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
import { startServer, uriOf, fixtures } from "./lsp-client.mjs"

const ws = path.join(fixtures, "workspace")
const scratch = (name) => uriOf(path.join(ws, `__protocol-${name}.ttl`))
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const broken = '@prefix ex: <http://example.org/> .\nex:s ex:p "open .\n'

const pullCapabilities = {
  textDocument: { diagnostic: { dynamicRegistration: false } },
  workspace: { diagnostics: { refreshSupport: true }, configuration: true },
}

describe("pull diagnostics", () => {
  let s
  before(async () => {
    s = await startServer({ capabilities: pullCapabilities })
  })
  after(async () => {
    await s.stop()
  })

  const pull = (uri, previousResultId) =>
    s.request("textDocument/diagnostic", { textDocument: { uri }, previousResultId })

  test("diagnosticProvider and didSave are advertised", () => {
    const caps = s.init.capabilities
    assert.deepEqual(caps.diagnosticProvider, {
      identifier: "turtle",
      interFileDependencies: true,
      workspaceDiagnostics: false,
    })
    assert.deepEqual(caps.textDocumentSync, { openClose: true, change: 2, save: { includeText: false } })
  })

  test("a pull returns the diagnostics, then unchanged", async () => {
    const uri = scratch("pull")
    s.open(uri, broken)
    const first = await pull(uri)
    assert.equal(first.kind, "full")
    assert.ok(first.items.some((d) => d.severity === 1 && d.range.start.line === 1), JSON.stringify(first.items))
    assert.match(first.resultId, /^1:\d+:\d+$/)

    const second = await pull(uri, first.resultId)
    assert.deepEqual(second, { kind: "unchanged", resultId: first.resultId })

    s.change(uri, "@prefix ex: <http://example.org/> .\nex:s ex:p ex:o .\n")
    const third = await pull(uri, first.resultId)
    assert.equal(third.kind, "full")
    assert.notEqual(third.resultId, first.resultId)
    assert.deepEqual(third.items.filter((d) => d.severity <= 2), [])

    s.notify("textDocument/didSave", { textDocument: { uri } })
    s.close(uri)
    await s.request("textDocument/documentHighlight", { textDocument: { uri }, position: { line: 0, character: 0 } })
    assert.deepEqual(s.published, [], "a pull client must not get publishDiagnostics")
  })

  test("a settings change invalidates reports and asks the client to refresh", async () => {
    const uri = scratch("refresh")
    s.open(uri, broken)
    const first = await pull(uri)
    s.serverRequests.length = 0
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { format: { lineWidth: 80 } } } })
    const again = await pull(uri, first.resultId)
    assert.equal(again.kind, "full")
    assert.notEqual(again.resultId, first.resultId)
    for (let i = 0; i < 40 && !s.serverRequests.length; i++) await sleep(25)
    assert.deepEqual(
      s.serverRequests.map((r) => r.method),
      ["workspace/diagnostic/refresh"],
    )
    s.close(uri)
  })

  test("didChangeConfiguration without settings asks for the turtle section", async () => {
    s.serverRequests.length = 0
    s.notify("workspace/didChangeConfiguration", { settings: null })
    for (let i = 0; i < 40 && !s.serverRequests.length; i++) await sleep(25)
    const req = s.serverRequests.find((r) => r.method === "workspace/configuration")
    assert.deepEqual(req?.params, { items: [{ section: "turtle" }] })
    // The server keeps answering after the (null) reply.
    const r = await s.request("textDocument/diagnostic", { textDocument: { uri: scratch("none") } })
    assert.equal(r.kind, "full")
  })
})

describe("pull without refresh support", () => {
  // Neovim 0.11 can pull but cannot be told to pull again, so a change in one
  // file would leave other buffers stale: such clients keep push.
  let s
  before(async () => {
    s = await startServer({ capabilities: { textDocument: { diagnostic: { dynamicRegistration: false } } } })
  })
  after(async () => {
    await s.stop()
  })

  test("the server pushes instead", async () => {
    assert.equal(s.init.capabilities.diagnosticProvider, undefined)
    const uri = scratch("no-refresh")
    s.open(uri, broken)
    const diags = await s.diagnosticsFor(uri)
    assert.ok(diags.some((d) => d.severity === 1))
  })
})

describe("push diagnostics", () => {
  let s
  before(async () => {
    s = await startServer({ initializationOptions: { prefixes: { ex: "http://example.org/" } } })
  })
  after(async () => {
    await s.stop()
  })

  test("no diagnosticProvider without the client capability", () => {
    assert.equal(s.init.capabilities.diagnosticProvider, undefined)
  })

  test("diagnostics are pushed; didSave and didChangeConfiguration are accepted", async () => {
    const uri = scratch("push")
    s.open(uri, "ex:s ex:p ex:o .\n")
    // ex: is described in the fixture workspace, so ex:p and ex:o are linted.
    const notLint = (diags) => diags.filter((d) => d.code !== "undescribed-term")
    assert.deepEqual(notLint(await s.diagnosticsFor(uri)), [], "initializationOptions prefixes apply")

    // Dropping the prefix from the settings makes ex: undeclared.
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { prefixes: {} } } })
    s.notify("textDocument/didSave", { textDocument: { uri } })
    s.change(uri, "ex:s ex:p ex:o .\n")
    const diags = await s.diagnosticsFor(uri)
    assert.ok(diags.some((d) => /ex/.test(d.message) && d.severity === 1), JSON.stringify(diags))
    s.close(uri)
    assert.equal(s.stderr.includes("panicked"), false, s.stderr)
  })
})

describe("cancellation", () => {
  let s
  before(async () => {
    s = await startServer()
  })
  after(async () => {
    await s.stop()
  })

  test("a request cancelled in the same batch gets RequestCancelled", async () => {
    // A large document and a slow request keep the server busy, so the next
    // request and its cancel queue up behind them and arrive in one batch.
    const big = ["@prefix ex: <http://example.org/> ."]
    for (let i = 0; i < 20000; i++) big.push(`ex:s${i} a ex:Thing ; ex:p "v${i}" .`)
    const uri = scratch("cancel")
    // Every write goes through sendRaw, so no frame is split by another.
    s.sendRaw([
      {
        method: "textDocument/didOpen",
        params: { textDocument: { uri, languageId: "turtle", version: 1, text: big.join("\n") } },
      },
      { id: "raw-slow", method: "textDocument/documentSymbol", params: { textDocument: { uri } } },
    ])
    s.sendRaw([
      { id: "raw-cancelled", method: "textDocument/foldingRange", params: { textDocument: { uri } } },
      { method: "$/cancelRequest", params: { id: "raw-cancelled" } },
      { id: "raw-kept", method: "textDocument/foldingRange", params: { textDocument: { uri } } },
    ])
    const cancelled = await s.rawResponse("raw-cancelled")
    assert.equal(cancelled.error?.code, -32800, JSON.stringify(cancelled))
    assert.equal(cancelled.result, undefined)
    const kept = await s.rawResponse("raw-kept")
    assert.ok(Array.isArray(kept.result), "the other request is still answered")
    assert.ok(Array.isArray((await s.rawResponse("raw-slow")).result))
    s.close(uri)
  })
})
