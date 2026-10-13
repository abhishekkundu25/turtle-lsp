// Ontology- and shape-aware completion ranking: what the classes, domains,
// ranges and SHACL shapes in the workspace expect at the cursor comes first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const project = path.join(fixtures, "project")
const workspace = path.join(fixtures, "workspace")

let p, w
before(async () => {
  ;[p, w] = await Promise.all([startServer({ cwd: project }), startServer({ cwd: workspace })])
})
after(async () => {
  await Promise.all([p.stop(), w.stop()])
})

let n = 0
async function complete(server, dir, text, needle = text) {
  const uri = uriOf(path.join(dir, `__ranking-${n++}.ttl`))
  server.open(uri, text)
  await server.diagnosticsFor(uri)
  const res = await server.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, needle) })
  return res
}
const labels = (res) => res.items.map((i) => i.label)
const show = (res) => labels(res).slice(0, 12).join(", ")

test("predicate after 'a sd:Asset ;': shape-required property first, then the other shape paths", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:x a sd:Asset ;\n    ")
  assert.deepEqual(labels(res).slice(0, 2), ["sd:designation", "sd:fillsPosition"], show(res))
  const designation = res.items[0]
  assert.match(designation.detail, /^required by sdsh:AssetShape$/, designation.detail)
  assert.equal(designation.kind, 10, "property kind")
  // Domain via the superclass sd:PhysicalObject comes next.
  assert.equal(labels(res)[2], "sd:locatedIn", show(res))
  assert.ok(res.items.length <= 200 && res.isIncomplete)
})

test("predicate ranking applies when a prefix is typed too", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:x a sd:Asset ;\n    sd:")
  assert.deepEqual(labels(res).slice(0, 3), ["sd:designation", "sd:fillsPosition", "sd:locatedIn"], show(res))
})

test("types written inside [ ] rank properties for the blank node", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:y sd:hasPart [ a sd:Asset ; ")
  assert.deepEqual(labels(res).slice(0, 2), ["sd:designation", "sd:fillsPosition"], show(res))
})

test("subject types from the workspace count without 'a' in this statement", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:mill-0517 rdfs:comment \"x\" ;\n    ")
  assert.deepEqual(labels(res).slice(0, 2), ["sd:designation", "sd:fillsPosition"], show(res))
})

test("object of sd:fillsPosition: instances of sd:Position and its subclasses first", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:x a sd:Asset ;\n    sd:fillsPosition ")
  assert.deepEqual(new Set(labels(res).slice(0, 2)), new Set(["obj:pos-mc2", "obj:cell-3"]), show(res))
  assert.equal(res.items.find((i) => i.label === "obj:pos-mc2").detail, "Milling cell 2")
})

test("object range comes from a superproperty without any shape", async () => {
  // sd:fillsPosition has no range of its own; sd:locatedIn's is sd:Position.
  const res = await complete(p, path.join(project, "data"), "obj:z sd:fillsPosition obj:")
  assert.deepEqual(new Set(labels(res).slice(0, 2)), new Set(["obj:pos-mc2", "obj:cell-3"]), show(res))
})

test("sh:in of a sequence path is offered inside the intermediate [ ]", async () => {
  // asset-shapes: sh:path ( sd:fillsPosition sd:hasFunction ) ; sh:in ( fn:Milling )
  const res = await complete(p, path.join(project, "data"), "obj:x a sd:Asset ;\n    sd:fillsPosition [ sd:hasFunction ")
  const first = res.items[0]
  assert.equal(first?.label, "fn:Milling", show(res))
  assert.equal(first.detail, "allowed by sdsh:AssetShape")
})

test("datatype ranges offer no instances", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:x a sd:Asset ;\n    sd:designation ")
  assert.ok(!labels(res).some((l) => /^obj:./.test(l)), show(res))
})

test("foaf:knows object: foaf:Person instances (and of its subclasses) first", async () => {
  const text = `@prefix ex: <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
ex:zed a foaf:Person ;
    foaf:knows `
  const res = await complete(w, workspace, text)
  const top = labels(res).slice(0, 4)
  assert.deepEqual(new Set(top), new Set(["ex:alice", "ex:bob", "ex:dave", "ex:erin"]), show(res))
  assert.ok(!labels(res).includes("ex:zed"), "the subject itself is not offered")
  assert.ok(!top.includes("ex:acme"))
})

test("vocabulary domains rank properties for a vocabulary class", async () => {
  const text = `@prefix ex: <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
ex:zed a ex:Manager ;
    foaf:`
  const res = await complete(w, workspace, text)
  // ex:Manager < ex:Employee < foaf:Person; foaf:knows has domain foaf:Person.
  const knows = labels(res).indexOf("foaf:knows")
  const page = labels(res).indexOf("foaf:page") // domain owl:Thing: not ranked
  assert.ok(knows >= 0 && (page < 0 || knows < page), show(res))
  assert.ok(res.items.slice(0, knows).every((i) => i.kind === 10), show(res))
})

test("no ranking for an untyped subject keeps the old order", async () => {
  const res = await complete(p, path.join(project, "data"), "obj:q ")
  assert.equal(labels(res)[0], "a", show(res))
})
