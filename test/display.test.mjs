// Semantic tokens and label inlay hints over stdio, on test/fixtures/display:
// model.ttl describes the terms (classes, properties, labels), data.ttl uses
// them, so classes, properties and labels come from another file.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const dir = path.join(fixtures, "display")
const dataPath = path.join(dir, "data.ttl")
const data = fs.readFileSync(dataPath, "utf8")
const uri = uriOf(dataPath)

// Decode LSP semantic tokens into { text, type, mods, line, character }.
// `utf8` selects the position encoding the server was given.
function decode(result, legend, text, utf8 = false) {
  const lines = text.split("\n")
  const units = (l) => (utf8 ? Buffer.from(l, "utf8") : l)
  const slice = (l, a, b) => (utf8 ? units(l).subarray(a, b).toString("utf8") : l.slice(a, b))
  const out = []
  let line = 0
  let character = 0
  const d = result.data
  for (let i = 0; i < d.length; i += 5) {
    if (d[i] > 0) character = 0
    line += d[i]
    character += d[i + 1]
    const mods = legend.tokenModifiers.filter((_, bit) => d[i + 4] & (1 << bit))
    out.push({ text: slice(lines[line], character, character + d[i + 2]), type: legend.tokenTypes[d[i + 3]], mods, line, character })
  }
  return out
}

const pick = (tokens, text) => tokens.filter((t) => t.text === text).map((t) => [t.type, t.mods.join(",")])
const labels = (hints) => hints.map((h) => [h.label, h.position.line, h.position.character])
const wholeFile = { start: { line: 0, character: 0 }, end: { line: 999, character: 0 } }

describe("display (UTF-16 positions)", () => {
  let s
  let legend
  before(async () => {
    s = await startServer({ cwd: dir })
    legend = s.init.capabilities.semanticTokensProvider.legend
    s.open(uri, data)
    await s.diagnosticsFor(uri)
  })
  after(async () => {
    await s.stop()
  })

  test("advertises semantic tokens (full + range) with the standard legend, and inlay hints", () => {
    const p = s.init.capabilities.semanticTokensProvider
    assert.equal(p.full, true)
    assert.equal(p.range, true)
    assert.deepEqual(legend.tokenTypes, ["namespace", "class", "property", "enumMember", "type", "keyword"])
    assert.deepEqual(legend.tokenModifiers, ["declaration", "deprecated", "defaultLibrary", "undescribed"])
    assert.equal(s.init.capabilities.inlayHintProvider, true)
  })

  test("tokens classify prefixes, classes, properties, individuals, datatypes and keywords", async () => {
    const t = decode(await s.request("textDocument/semanticTokens/full", { textDocument: { uri } }), legend, data)
    assert.deepEqual(pick(t, "@prefix"), [["keyword", ""]])
    assert.deepEqual(pick(t, "PREFIX"), [["keyword", ""]])
    assert.deepEqual(pick(t, "a"), [["keyword", ""], ["keyword", ""]])
    assert.deepEqual(pick(t, "ex").slice(0, 2), [["namespace", "declaration"], ["namespace", ""]])
    assert.deepEqual(pick(t, "vcard")[0], ["namespace", "declaration"])
    // ex:Pump is an owl:Class in model.ttl.
    assert.deepEqual(pick(t, "Pump"), [["class", ""]])
    // owl:DatatypeProperty / owl:ObjectProperty in model.ttl.
    assert.deepEqual(pick(t, "P0042"), [["property", ""], ["property", ""]])
    assert.deepEqual(pick(t, "locatedIn"), [["property", ""]])
    assert.deepEqual(pick(t, "hasEmail"), [["property", "defaultLibrary"]])
    assert.deepEqual(pick(t, "pump1"), [["enumMember", "declaration"]])
    assert.deepEqual(pick(t, "<http://www.w3.org/2001/XMLSchema#decimal>"), [["type", "defaultLibrary"]])
  })

  test("deprecated vocabulary terms and undescribed own-namespace terms carry modifiers", async () => {
    const t = decode(await s.request("textDocument/semanticTokens/full", { textDocument: { uri } }), legend, data)
    assert.deepEqual(pick(t, "Email"), [["class", "deprecated,defaultLibrary"]])
    // https://example.com/plant# is an own namespace; ex:hall7 is described nowhere.
    assert.deepEqual(pick(t, "hall7"), [["enumMember", "undescribed"]])
  })

  test("tokens are sorted, never overlap and use UTF-16 columns", async () => {
    const t = decode(await s.request("textDocument/semanticTokens/full", { textDocument: { uri } }), legend, data)
    for (let i = 1; i < t.length; i++) {
      const [a, b] = [t[i - 1], t[i]]
      assert.ok(b.line > a.line || b.character >= a.character + a.text.length, `${a.text} overlaps ${b.text}`)
    }
    const cafe = t.find((x) => x.text === "café")
    assert.deepEqual([cafe.type, cafe.mods], ["enumMember", ["declaration"]])
    const after = t.filter((x) => x.line === cafe.line && x.text === "P0042")
    assert.deepEqual(after.map((x) => x.character), [positionOf(data, "ex:café ex:", 11).character])
  })

  test("a range request returns only the tokens in the range", async () => {
    const line = positionOf(data, "    ex:locatedIn", 0).line
    const range = { start: { line, character: 0 }, end: { line, character: 100 } }
    const res = await s.request("textDocument/semanticTokens/range", { textDocument: { uri }, range })
    const t = decode(res, legend, data)
    assert.deepEqual(t.map((x) => x.text), ["ex", "locatedIn", "ex", "hall7"])
    assert.ok(t.every((x) => x.line === line))
  })

  test("inlay hints: the label after an opaque name, from another file; none for readable names", async () => {
    const hints = await s.request("textDocument/inlayHint", { textDocument: { uri }, range: wholeFile })
    const p0042 = positionOf(data, "ex:P0042")
    const second = positionOf(data, "ex:café ex:P0042")
    assert.deepEqual(labels(hints), [
      ["pump pressure", p0042.line, p0042.character],
      ["pump pressure", second.line, second.character],
    ])
    assert.equal(hints[0].paddingLeft, true)
    assert.equal(hints[0].tooltip, "<https://example.com/plant#P0042>")
    assert.equal(hints[0].kind, undefined)
  })

  test("inlay hints only for the requested range", async () => {
    const line = positionOf(data, "ex:café", 0).line
    const range = { start: { line, character: 0 }, end: { line: line + 1, character: 0 } }
    const hints = await s.request("textDocument/inlayHint", { textDocument: { uri }, range })
    assert.equal(hints.length, 1)
    assert.equal(hints[0].position.line, line)
  })

  test("didChangeConfiguration switches labels to all, then off", async () => {
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: "all" } } } })
    const all = await s.request("textDocument/inlayHint", { textDocument: { uri }, range: wholeFile })
    const texts = all.map((h) => h.label)
    for (const l of ["Pump", "pump pressure", "located in"]) assert.ok(texts.includes(l), `${l} missing: ${texts}`)
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: "off" } } } })
    assert.deepEqual(await s.request("textDocument/inlayHint", { textDocument: { uri }, range: wholeFile }), [])
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: "opaque" } } } })
  })
})

describe("display (UTF-8 positions, initializationOptions)", () => {
  let s
  before(async () => {
    s = await startServer({
      cwd: dir,
      capabilities: { general: { positionEncodings: ["utf-8", "utf-16"] }, workspace: { inlayHint: { refreshSupport: true } } },
      initializationOptions: { inlayHints: { labels: "all" } },
    })
    s.open(uri, data)
    await s.diagnosticsFor(uri)
  })
  after(async () => {
    await s.stop()
  })

  test("columns and lengths are bytes", async () => {
    assert.equal(s.init.capabilities.positionEncoding, "utf-8")
    const legend = s.init.capabilities.semanticTokensProvider.legend
    const t = decode(await s.request("textDocument/semanticTokens/full", { textDocument: { uri } }), legend, data, true)
    const cafe = t.find((x) => x.text === "café")
    const p = t.find((x) => x.line === cafe.line && x.text === "P0042")
    assert.equal(p.character, Buffer.byteLength("ex:café ex:"))
  })

  test("initializationOptions labels: all; hint positions are bytes", async () => {
    const hints = await s.request("textDocument/inlayHint", { textDocument: { uri }, range: wholeFile })
    assert.ok(hints.some((h) => h.label === "located in"), labels(hints).join(" | "))
    const line = positionOf(data, "ex:café", 0).line
    const h = hints.find((x) => x.position.line === line)
    assert.equal(h.position.character, Buffer.byteLength("ex:café ex:P0042"))
  })

  test("the client is asked to refresh inlay hints once the workspace is indexed", async () => {
    for (let i = 0; i < 40 && !s.serverRequests.length; i++) await new Promise((r) => setTimeout(r, 25))
    assert.ok(s.serverRequests.some((r) => r.method === "workspace/inlayHint/refresh"))
  })
})
