// turtle/graph through the browser engine's language server (lsp-host.js's
// EmbeddedServer, which the editor's worker runs): the capability, the nodes
// and edges of a small workspace, and InvalidParams for a bad request. Run
// `npm run web:build` first.
import { test, before } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { EmbeddedServer } from "../app/lsp-host.js"

const here = path.dirname(fileURLToPath(import.meta.url))
const A = "file:///work/a.ttl"
const B = "file:///work/b.ttl"
const files = {
  [A]: `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:Device a owl:Class .
ex:Pump a owl:Class ;
    rdfs:label "Pump" ;
    rdfs:subClassOf ex:Device .
`,
  [B]: `@prefix ex: <http://example.org/> .

ex:p1 a ex:Pump ;
    ex:feeds ex:p2 .
ex:p2 a ex:Pump .
`,
}

let server, init
before(async () => {
  server = new EmbeddedServer(await createEngine(fs.readFileSync(path.join(here, "..", "app", "engine.wasm"))))
  init = await server.initialize({ processId: null, rootUri: null, capabilities: {} })
  for (const [uri, text] of Object.entries(files)) server.notify("textDocument/didOpen", { textDocument: { uri, languageId: "turtle", version: 1, text } })
  await server.settle()
})

test("the engine's server lists turtle/graph under its experimental capabilities", () => {
  assert.ok(init.capabilities.experimental?.["turtle/graph"], JSON.stringify(init.capabilities.experimental))
})

test("turtle/graph gives the workspace's nodes and edges, with where each is defined (catches: the request not routed in the engine, nodes without uri/range)", async () => {
  const res = await server.request("turtle/graph", { layout: { iterations: 20 } })
  const byId = new Map(res.nodes.map((n, i) => [n.id, { ...n, i }]))
  const pump = byId.get("http://example.org/Pump")
  assert.ok(pump, JSON.stringify(res.nodes.map((n) => n.id)))
  assert.equal(pump.kind, "class")
  assert.equal(pump.label, "Pump")
  assert.equal(pump.name, "ex:Pump")
  assert.equal(pump.uri, A)
  assert.deepEqual(pump.range.start, { line: 5, character: 0 })
  for (const id of ["http://example.org/Device", "http://example.org/p1", "http://example.org/p2"]) assert.ok(byId.has(id), id)
  const edge = (s, p, o) => res.edges.find((e) => e.source === byId.get(s)?.i && e.target === byId.get(o)?.i && e.predicate === p)
  const sub = edge("http://example.org/Pump", "http://www.w3.org/2000/01/rdf-schema#subClassOf", "http://example.org/Device")
  assert.ok(sub, JSON.stringify(res.edges))
  assert.equal(sub.uri, A)
  assert.equal(edge("http://example.org/p1", "http://example.org/feeds", "http://example.org/p2")?.uri, B)
  // rdf:type edges and literals are off by default.
  assert.equal(res.edges.some((e) => e.predicate.endsWith("#type")), false)
  assert.equal(res.nodes.some((n) => n.literal), false)
  assert.equal(res.positions.length, res.nodes.length * 2)
  assert.ok(res.positions.every(Number.isFinite))
  assert.deepEqual(res.total, { nodes: res.nodes.length, edges: res.edges.length })
  assert.deepEqual(res.facets.files.map((f) => f.uri).sort(), [A, B])
})

test("turtle/graph with an empty focus is an InvalidParams error, and the server keeps answering (catches: a panic in the engine, a bad request taken as no focus)", async () => {
  const error = await server.request("turtle/graph", { focus: "" }).then(
    () => null,
    (e) => e
  )
  assert.ok(error, "the request failed")
  assert.equal(error.code, -32602, error.message)
  const res = await server.request("turtle/graph", { files: [B], layout: { iterations: 0 } })
  const ids = new Set(res.nodes.map((n) => n.id))
  assert.ok(ids.has("http://example.org/p1") && ids.has("http://example.org/p2"), JSON.stringify([...ids]))
  assert.equal(ids.has("http://example.org/Device"), false, "a.ttl's triples are left out")
  assert.ok(res.edges.every((e) => e.uri === B))
})
