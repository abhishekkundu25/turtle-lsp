// The custom request `turtle/schema` over stdio, as a client sees it: the
// capability, the parameters checked at the boundary, `at` resolved on the
// server's main loop, and the answer computed off it (src/server.rs,
// src/ide/display.rs). What the answer holds is src/features/schema.rs's.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir, positionOf } from "./lsp-client.mjs"

const PREFIXES = `@prefix ex: <http://example.org/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
const model = `${PREFIXES}ex:Pump a rdfs:Class ; rdfs:label "Pump" .
ex:feeds rdfs:domain ex:Pump ; rdfs:range ex:Pump .
`
const data = `${PREFIXES}ex:p1 a ex:Pump ; ex:feeds ex:p2 .
ex:p2 a ex:Pump .
`
const EX = "http://example.org/"
const InvalidParams = -32602

// What every answer has, whatever the workspace.
function wellFormed(r) {
  assert.ok(Array.isArray(r.classes), JSON.stringify(r))
  assert.equal(typeof r.total.classes, "number")
  assert.equal(typeof r.imported, "number")
  assert.ok(r.classes.length <= r.total.classes, JSON.stringify(r))
  if (r.focus !== undefined) assert.ok(r.focus < r.classes.length, JSON.stringify(r))
}

// The graph of the indexed workspace (the initial index lands in the
// background): ask until `nodes` nodes are in.
async function indexed(s, nodes, timeoutMs = 10000) {
  const end = Date.now() + timeoutMs
  for (;;) {
    const r = await s.request("turtle/graph", { layout: { iterations: 0 } })
    if (r.total.nodes >= nodes) return r
    if (Date.now() > end) throw new Error(`the graph has ${r.total.nodes} nodes, not ${nodes}`)
    await new Promise((res) => setTimeout(res, 50))
  }
}

describe("turtle/schema", () => {
  let s, dir, modelUri, dataUri
  before(async () => {
    dir = tempDir("turtle-lsp-schema-")
    fs.writeFileSync(path.join(dir, "model.ttl"), model)
    fs.writeFileSync(path.join(dir, "data.ttl"), data)
    modelUri = uriOf(path.join(dir, "model.ttl"))
    dataUri = uriOf(path.join(dir, "data.ttl"))
    s = await startServer({ cwd: dir })
    await indexed(s, 4)
    s.open(modelUri, model)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("is listed under the experimental capabilities, beside turtle/graph", () => {
    assert.deepEqual(s.init.capabilities.experimental, { "turtle/graph": true, "turtle/schema": true })
  })

  test("answers with no params, empty params and every parameter", async () => {
    wellFormed(await s.request("turtle/schema"))
    wellFormed(await s.request("turtle/schema", {}))
    wellFormed(
      await s.request("turtle/schema", {
        files: [modelUri, dataUri],
        focus: EX + "Pump",
        maxClasses: 2000,
        maxRows: 512,
        imports: true,
      }),
    )
  })

  test("identical requests get identical answers", async () => {
    const params = { focus: EX + "Pump" }
    const [a, b] = await Promise.all([s.request("turtle/schema", params), s.request("turtle/schema", params)])
    assert.deepEqual(a, b)
    assert.deepEqual(await s.request("turtle/schema", params), a)
  })

  test("at is resolved in an open document and in one only indexed, on a term or off one", async () => {
    for (const at of [
      { uri: modelUri, position: positionOf(model, "ex:Pump", 4) },
      { uri: modelUri, position: positionOf(model, "ex:feeds", 4) },
      { uri: dataUri, position: positionOf(data, "ex:Pump", 4) },
    ]) {
      wellFormed(await s.request("turtle/schema", { at }))
    }
    // Off a term (a literal, past the end): nothing to follow, null.
    for (const at of [
      { uri: modelUri, position: positionOf(model, "Pump\"", 2) },
      { uri: modelUri, position: { line: 99, character: 0 } },
    ]) {
      assert.equal(await s.request("turtle/schema", { at }), null, JSON.stringify(at))
    }
  })

  test(
    "at on a class focuses its box, and wins over focus",
    async () => {
      const r = await s.request("turtle/schema", {
        focus: EX + "feeds",
        at: { uri: dataUri, position: positionOf(data, "ex:Pump", 4) },
      })
      assert.equal(r.classes[r.focus]?.id, EX + "Pump", JSON.stringify(r))
    },
  )

  test("invalid parameters are an InvalidParams error, and the server goes on", async () => {
    for (const params of [
      { maxClasses: 0 },
      { maxClasses: 2001 },
      { maxRows: 0 },
      { maxRows: 513 },
      { maxClasses: "many" },
      { imports: "yes" },
      { at: { uri: "file:///nowhere/model.ttl", position: { line: 0, character: 0 } } },
      { at: { uri: modelUri } },
      { at: "ex:Pump" },
      [1, 2],
    ]) {
      await assert.rejects(s.request("turtle/schema", params), (e) => {
        assert.equal(e.code, InvalidParams, JSON.stringify(params))
        assert.ok(e.message, JSON.stringify(params))
        return true
      })
    }
    const hover = await s.request("textDocument/hover", {
      textDocument: { uri: modelUri },
      position: positionOf(model, "ex:Pump", 4),
    })
    assert.match(hover.contents.value, /Pump/)
    wellFormed(await s.request("turtle/schema", {}))
  })

  test("a hover sent while turtle/schema runs is answered", async () => {
    const schemas = Array.from({ length: 4 }, () => s.request("turtle/schema", { imports: true }))
    const hover = await s.request("textDocument/hover", {
      textDocument: { uri: modelUri },
      position: positionOf(model, "ex:Pump", 4),
    })
    assert.match(hover.contents.value, /Pump/)
    for (const r of await Promise.all(schemas)) wellFormed(r)
  })
})

describe("turtle/schema examples", () => {
  const shapes = `${PREFIXES}@prefix sh: <http://www.w3.org/ns/shacl#> .
ex:Pump a rdfs:Class .
ex:PumpShape a sh:NodeShape ; sh:targetClass ex:Pump ;
  sh:property [ sh:path ex:serial ; sh:minCount 1 ] .
`
  const pumps = `${PREFIXES}ex:p1 a ex:Pump ; ex:serial "A1" .
ex:p2 a ex:Pump .
ex:p3 a ex:Pump ;
  rdfs:label "no serial" .
`
  let s, dir, pumpsUri
  before(async () => {
    dir = tempDir("turtle-lsp-schema-examples-")
    fs.writeFileSync(path.join(dir, "shapes.ttl"), shapes)
    fs.writeFileSync(path.join(dir, "pumps.ttl"), pumps)
    pumpsUri = uriOf(path.join(dir, "pumps.ttl"))
    s = await startServer({ cwd: dir })
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  // The text a range covers, one-line ranges only.
  const at = (text, range) => {
    assert.equal(range.start.line, range.end.line, JSON.stringify(range))
    return text.split("\n")[range.start.line].slice(range.start.character, range.end.character)
  }

  test("a required property's row names the instances lacking it, with their places", async () => {
    let row
    const end = Date.now() + 10000
    while (!row?.missing) {
      if (Date.now() > end) throw new Error(`no missing serials: ${JSON.stringify(row)}`)
      await new Promise((res) => setTimeout(res, 50))
      const r = await s.request("turtle/schema", { focus: EX + "serial" })
      row = r.classes[r.focus]?.rows[r.focusRow]
    }
    assert.equal(row.missing, 2)
    assert.deepEqual(row.examples.map((e) => [e.kind, e.uri, e.name]), [
      ["missing", pumpsUri, "ex:p2"],
      ["missing", pumpsUri, "ex:p3"],
    ])
    const lines = pumps.split("\n").length
    for (const e of row.examples) {
      assert.ok(e.range.end.line < lines, JSON.stringify(e))
      assert.equal(at(pumps, e.range), e.name)
    }
  })

  test("only one class lists places: the one `examples` names, else the focus, else none", async () => {
    const places = (r) => r.classes.filter((c) => c.rows.some((row) => row.examples?.length)).map((c) => c.id)
    assert.deepEqual(places(await s.request("turtle/schema", {})), [])
    const asked = await s.request("turtle/schema", { examples: EX + "Pump" })
    assert.deepEqual(places(asked), [EX + "Pump"])
    assert.equal(asked.focus, undefined, "examples is not a focus")
    assert.deepEqual(places(await s.request("turtle/schema", { examples: EX + "nothing" })), [])
  })
})
