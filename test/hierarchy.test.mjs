// Type hierarchy for classes and properties, across workspace files and
// well-known vocabularies. Workspace: a copy of test/fixtures/project.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

let ws, s
before(async () => {
  ws = tempDir("turtle-hierarchy-")
  fs.cpSync(path.join(fixtures, "project"), ws, { recursive: true })
  s = await startServer({ cwd: ws })
})
after(async () => {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

const file = (rel) => uriOf(path.join(ws, rel))

async function prepare(uri, text, needle, at) {
  const items = await s.request("textDocument/prepareTypeHierarchy", {
    textDocument: { uri },
    position: positionOf(text, needle, at),
  })
  assert.ok(items?.length === 1, JSON.stringify(items))
  return items[0]
}
const names = (items) => items.map((i) => i.name)

test("typeHierarchyProvider is advertised", () => {
  assert.equal(s.init.capabilities.typeHierarchyProvider, true)
})

test("supertypes of sd:MeasurementPoint: the workspace class and sosa:Sensor", async () => {
  const uri = file("data/instances.ttl")
  const text = fs.readFileSync(path.join(ws, "data/instances.ttl"), "utf8")
  s.open(uri, text)
  const item = await prepare(uri, text, "a sd:Measurem")
  assert.equal(item.name, "sd:MeasurementPoint")
  assert.equal(item.kind, 5) // Class
  assert.equal(item.detail, "Measurement point")
  // Located on its description in core/vocab.ttl, selecting the subject.
  assert.equal(item.uri, file("core/vocab.ttl"))
  assert.equal(item.selectionRange.start.line, 19)
  assert.ok(item.range.end.line > item.range.start.line, JSON.stringify(item.range))

  const supers = await s.request("typeHierarchy/supertypes", { item })
  assert.deepEqual(names(supers), ["sd:PhysicalObject", "sosa:Sensor"])
  const sensor = supers[1]
  assert.equal(sensor.detail, "from the sosa vocabulary")
  // A borrowed term is shown where the workspace refers to it.
  assert.equal(sensor.uri, file("core/vocab.ttl"))
  assert.equal(sensor.selectionRange.start.line, 20)
  assert.equal(supers[0].uri, file("core/vocab.ttl"))
  assert.equal(supers[0].selectionRange.start.line, 5)
})

test("subtypes of sd:PhysicalObject include sd:Asset; walking down and up again ends", async () => {
  const uri = file("core/vocab.ttl")
  const text = fs.readFileSync(path.join(ws, "core/vocab.ttl"), "utf8")
  s.open(uri, text)
  const item = await prepare(uri, text, "sd:PhysicalOb")
  const subs = await s.request("typeHierarchy/subtypes", { item })
  assert.ok(names(subs).includes("sd:Asset"), names(subs).join())
  assert.ok(names(subs).includes("sd:MeasurementPoint"), names(subs).join())
  const asset = subs.find((i) => i.name === "sd:Asset")
  assert.deepEqual(await s.request("typeHierarchy/subtypes", { item: asset }), [])
  assert.deepEqual(names(await s.request("typeHierarchy/supertypes", { item: asset })), ["sd:PhysicalObject"])
  assert.deepEqual(await s.request("typeHierarchy/supertypes", { item }), [])
})

test("property hierarchy: sd:fillsPosition is a sub property of sd:locatedIn", async () => {
  const uri = file("data/instances.ttl")
  const text = fs.readFileSync(path.join(ws, "data/instances.ttl"), "utf8")
  const item = await prepare(uri, text, "sd:fillsPos")
  assert.equal(item.kind, 7) // Property
  const supers = await s.request("typeHierarchy/supertypes", { item })
  assert.deepEqual(names(supers), ["sd:locatedIn"])
  assert.equal(supers[0].kind, 7)
  const subs = await s.request("typeHierarchy/subtypes", { item: supers[0] })
  assert.deepEqual(names(subs), ["sd:fillsPosition"])
})

test("cycles are harmless and names use the asking file's prefixes", async () => {
  const uri = file("data/__cycle.ttl")
  const text = [
    "@prefix c: <https://ontology.example.com/plant/core/> .",
    "c:A rdfs:subClassOf c:B .",
    "c:B rdfs:subClassOf c:A , c:B .",
    "",
  ].join("\n")
  s.open(uri, text)
  const a = await prepare(uri, text, "c:A r", 1)
  assert.equal(a.name, "c:A")
  assert.equal(a.uri, uri)
  const up = await s.request("typeHierarchy/supertypes", { item: a })
  assert.deepEqual(names(up), ["c:B"])
  // B's supers are A and B itself; B is not its own super type.
  assert.deepEqual(names(await s.request("typeHierarchy/supertypes", { item: up[0] })), ["c:A"])
  assert.deepEqual(names(await s.request("typeHierarchy/subtypes", { item: up[0] })), ["c:A"])
  s.close(uri)
})

test("a vocabulary class with no workspace reference beyond the asking file", async () => {
  const uri = file("data/__vocab.ttl")
  const text = "obj:x a prov:Person .\n"
  s.open(uri, text)
  const item = await prepare(uri, text, "prov:Pers")
  assert.equal(item.uri, uri)
  assert.equal(item.detail, "from the prov vocabulary")
  // prov:Agent is the vocabulary's super class; nothing in the workspace refers
  // to it, so there is no location to show and it is left out.
  assert.deepEqual(await s.request("typeHierarchy/supertypes", { item }), [])
  s.close(uri)
})

test("a literal or a plain instance has no type hierarchy", async () => {
  const uri = file("data/instances.ttl")
  const text = fs.readFileSync(path.join(ws, "data/instances.ttl"), "utf8")
  for (const needle of ['"Milling ce', "obj:mill-05"]) {
    const r = await s.request("textDocument/prepareTypeHierarchy", {
      textDocument: { uri },
      position: positionOf(text, needle),
    })
    assert.equal(r, null, needle)
  }
})
