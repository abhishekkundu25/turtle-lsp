// The custom request `turtle/graph` over stdio: the workspace graph, its
// filters, cap and layout (src/features/graph.rs), as a client sees it.
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
ex:Valve a rdfs:Class ; rdfs:subClassOf ex:Part .
ex:feeds a rdf:Property .
`
const data = `${PREFIXES}ex:p1 a ex:Pump ; ex:feeds ex:v1 .
ex:v1 a ex:Valve .
`
const EX = "http://example.org/"
const SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
const InvalidParams = -32602

const ids = (r) => r.nodes.map((n) => n.id).sort()
const edges = (r) => r.edges.map((e) => `${r.nodes[e.source].id} ${e.predicate} ${r.nodes[e.target].id}`).sort()

// Ask until the indexed workspace answers with `nodes` nodes (the initial
// index lands in the background).
async function indexed(s, nodes, timeoutMs = 10000) {
  const end = Date.now() + timeoutMs
  for (;;) {
    const r = await s.request("turtle/graph", { layout: { iterations: 0 } })
    if (r.total.nodes >= nodes) return r
    if (Date.now() > end) throw new Error(`the graph has ${r.total.nodes} nodes, not ${nodes}`)
    await new Promise((res) => setTimeout(res, 50))
  }
}

describe("turtle/graph", () => {
  let s, dir, modelUri, dataUri
  before(async () => {
    dir = tempDir("turtle-lsp-graph-")
    fs.writeFileSync(path.join(dir, "model.ttl"), model)
    fs.writeFileSync(path.join(dir, "data.ttl"), data)
    modelUri = uriOf(path.join(dir, "model.ttl"))
    dataUri = uriOf(path.join(dir, "data.ttl"))
    s = await startServer({ cwd: dir })
    await indexed(s, 6)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("is listed under the experimental capabilities", () => {
    assert.deepEqual(s.init.capabilities.experimental, { "turtle/graph": true, "turtle/schema": true })
  })

  test("answers the workspace's nodes, edges, positions, totals and facets", async () => {
    // No params at all is every default.
    const r = await s.request("turtle/graph")
    assert.deepEqual(ids(r), ["Part", "Pump", "Valve", "feeds", "p1", "v1"].map((n) => EX + n))
    assert.deepEqual(edges(r), [`${EX}Valve ${SUBCLASS} ${EX}Part`, `${EX}p1 ${EX}feeds ${EX}v1`])
    assert.deepEqual(r.total, { nodes: 6, edges: 2 })
    const kind = Object.fromEntries(r.nodes.map((n) => [n.id.slice(EX.length), n.kind]))
    assert.deepEqual(kind, { Part: "class", Pump: "class", Valve: "class", feeds: "property", p1: "individual", v1: "individual" })
    const pump = r.nodes.find((n) => n.id === EX + "Pump")
    assert.equal(pump.label, "Pump")
    assert.equal(pump.name, "ex:Pump")
    assert.equal(pump.uri, modelUri)
    assert.deepEqual(pump.range.start, positionOf(model, "ex:Pump", 0))
    assert.equal(pump.literal, false)
    assert.equal(r.nodes.find((n) => n.id === EX + "p1").uri, dataUri)
    assert.equal(r.edges.find((e) => e.predicate === EX + "feeds").uri, dataUri)
    assert.equal(r.positions.length, 2 * r.nodes.length)
    assert.ok(r.positions.every(Number.isFinite), String(r.positions))
    assert.deepEqual(r.facets.files.map((f) => f.uri).sort(), [dataUri, modelUri].sort())
    assert.ok(r.facets.namespaces.some((n) => n.iri === EX && n.prefix === "ex"), JSON.stringify(r.facets.namespaces))
    assert.deepEqual(r.facets.predicates.map((p) => p.iri).sort(), [EX + "feeds", SUBCLASS].sort())
  })

  test("passes the filters, the focus and the layout's positions through", async () => {
    const classes = await s.request("turtle/graph", { kinds: ["class"] })
    assert.deepEqual(ids(classes), [EX + "Part", EX + "Pump", EX + "Valve"])
    assert.deepEqual(edges(classes), [`${EX}Valve ${SUBCLASS} ${EX}Part`])
    const noSub = await s.request("turtle/graph", { predicates: { exclude: [SUBCLASS] } })
    assert.deepEqual(edges(noSub), [`${EX}p1 ${EX}feeds ${EX}v1`])
    const typed = await s.request("turtle/graph", { types: true })
    assert.ok(edges(typed).includes(`${EX}p1 http://www.w3.org/1999/02/22-rdf-syntax-ns#type ${EX}Pump`))
    const oneFile = await s.request("turtle/graph", { files: [dataUri] })
    assert.deepEqual(ids(oneFile), [EX + "p1", EX + "v1"])
    const focused = await s.request("turtle/graph", { focus: EX + "p1", depth: 1 })
    assert.deepEqual(ids(focused), [EX + "p1", EX + "v1"])
    const literals = await s.request("turtle/graph", { literals: true })
    assert.ok(literals.nodes.some((n) => n.literal && n.name.includes("Pump")))
    // Positions handed back stay where they are with no layout step.
    const positions = { [EX + "p1"]: [10, 20], [EX + "v1"]: [-30, 40] }
    const kept = await s.request("turtle/graph", { focus: EX + "p1", depth: 1, layout: { iterations: 0, positions } })
    kept.nodes.forEach((n, i) => assert.deepEqual(kept.positions.slice(2 * i, 2 * i + 2), positions[n.id]))
  })

  test("at focuses the term at a document position, and wins over focus", async () => {
    const at = (uri, text, needle, offset) => ({ uri, position: positionOf(text, needle, offset) })
    // On the local name or the prefix of a node.
    for (const offset of [1, 5]) {
      const r = await s.request("turtle/graph", { at: at(modelUri, model, "ex:Valve", offset), depth: 1 })
      assert.deepEqual(ids(r), [EX + "Part", EX + "Valve"])
      assert.equal(r.nodes[0].id, EX + "Valve", "the focus comes first")
    }
    // `at` wins over `focus`.
    const r = await s.request("turtle/graph", { focus: EX + "Valve", at: at(dataUri, data, "ex:p1", 4), depth: 1 })
    assert.deepEqual(ids(r), [EX + "p1", EX + "v1"])
    // Off a term (a literal, punctuation, past the end): nothing to follow, null.
    for (const position of [positionOf(model, '"Pump"', 2), positionOf(model, " ; rdfs:label", 1), { line: 99, character: 0 }]) {
      const none = await s.request("turtle/graph", { focus: EX + "p1", depth: 1, at: { uri: modelUri, position } })
      assert.equal(none, null, JSON.stringify(position))
    }
  })

  test("maxNodes caps the nodes, not the totals", async () => {
    const r = await s.request("turtle/graph", { maxNodes: 2 })
    assert.equal(r.nodes.length, 2)
    assert.equal(r.positions.length, 4)
    assert.deepEqual(r.total, { nodes: 6, edges: 2 })
    assert.ok(r.edges.every((e) => e.source < 2 && e.target < 2))
  })

  test("identical requests get identical answers", async () => {
    const params = { types: true, literals: true, layout: { iterations: 100 } }
    const [a, b] = await Promise.all([s.request("turtle/graph", params), s.request("turtle/graph", params)])
    assert.deepEqual(a, b)
    assert.deepEqual(await s.request("turtle/graph", params), a)
  })

  test("invalid parameters are an InvalidParams error, and the server goes on", async () => {
    for (const params of [
      { maxNodes: "many" },
      { kinds: ["planet"] },
      { focus: "" },
      { at: { uri: "file:///nowhere/model.ttl", position: { line: 0, character: 0 } } },
      { at: { position: { line: 0, character: 0 } } },
      { layout: { positions: { [EX + "p1"]: [1e300, 0] } } },
      [1, 2],
    ]) {
      await assert.rejects(s.request("turtle/graph", params), (e) => {
        assert.equal(e.code, InvalidParams, JSON.stringify(params))
        assert.ok(e.message, JSON.stringify(params))
        return true
      })
    }
    s.open(modelUri, model)
    const hover = await s.request("textDocument/hover", {
      textDocument: { uri: modelUri },
      position: positionOf(model, "ex:Pump", 4),
    })
    assert.match(hover.contents.value, /Pump/)
    assert.equal((await s.request("turtle/graph", {})).total.nodes, 6)
  })
})

describe("turtle/graph on a big workspace", () => {
  const N = 20000
  let s, dir
  before(async () => {
    dir = tempDir("turtle-lsp-graph-big-")
    const lines = [PREFIXES]
    for (let i = 0; i < N; i++) lines.push(`ex:n${i} ex:link ex:n${(i * 7 + 1) % N} , ex:n${(i + 1) % N} .`)
    fs.writeFileSync(path.join(dir, "big.ttl"), lines.join("\n") + "\n")
    fs.writeFileSync(path.join(dir, "small.ttl"), model)
    s = await startServer({ cwd: dir })
    await indexed(s, N, 30000)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("a long layout does not hold back other requests", async () => {
    const smallUri = uriOf(path.join(dir, "small.ttl"))
    s.open(smallUri, model)
    await s.diagnosticsFor(smallUri)
    let graphDone = 0
    const t0 = performance.now()
    const graph = s
      .request("turtle/graph", { maxNodes: N, layout: { iterations: 1000 } })
      .then((r) => ((graphDone = performance.now()), r))
    // Let the server take the graph request first.
    await new Promise((r) => setTimeout(r, 50))
    const hover = await s.request("textDocument/hover", {
      textDocument: { uri: smallUri },
      position: positionOf(model, "ex:Pump", 4),
    })
    const hoverDone = performance.now()
    assert.match(hover.contents.value, /Pump/)
    const r = await graph
    assert.equal(r.nodes.length, N)
    assert.ok(graphDone - t0 > 500, `the layout took ${Math.round(graphDone - t0)} ms: too short to tell`)
    assert.ok(hoverDone < graphDone, `hover answered after the graph (${Math.round(hoverDone - t0)} ms, ${Math.round(graphDone - t0)} ms)`)
    assert.ok(hoverDone - t0 < 1000, `hover took ${Math.round(hoverDone - t0)} ms`)
  })
})
