// A project of headerless Turtle fragments sharing one prefixes.ttl
// (test/fixtures/project). Covers the "shared prefix header" layout and a
// well-known prefix name (sd:) bound to a project namespace.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const dir = path.join(fixtures, "project")
const fragments = ["core/vocab.ttl", "shapes/asset-shapes.ttl", "data/instances.ttl", "data/observations.ttl"]
const read = (f) => fs.readFileSync(path.join(dir, f), "utf8")
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.message}`).join("\n")

let s
before(async () => {
  s = await startServer({ cwd: dir })
})
after(async () => {
  await s.stop()
})

async function open(f) {
  const uri = uriOf(path.join(dir, f))
  const text = read(f)
  s.open(uri, text)
  return { uri, text, diags: await s.diagnosticsFor(uri) }
}

for (const f of fragments) {
  test(`${f}: no problems with prefixes from prefixes.ttl`, async () => {
    const { diags } = await open(f)
    const problems = diags.filter((d) => d.severity <= 2)
    assert.deepEqual(problems, [], fmt(problems))
  })
}

test("definition jumps from instance data to the class in another fragment", async () => {
  const { uri, text } = await open("data/instances.ttl")
  const locs = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(text, "a sd:Ass") })
  assert.equal(locs.length, 1, JSON.stringify(locs))
  assert.ok(locs[0].uri.endsWith("/core/vocab.ttl"))
  assert.equal(locs[0].range.start.line, 9)
})

test("definition on a prefix goes to its line in prefixes.ttl", async () => {
  const { uri, text } = await open("data/instances.ttl")
  const locs = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(text, "obj:pos-mc2 a", 1) })
  assert.equal(locs.length, 1, JSON.stringify(locs))
  assert.ok(locs[0].uri.endsWith("/prefixes.ttl"))
})

test("hover on a project term shows its label and comment from the vocabulary file", async () => {
  const { uri, text } = await open("data/instances.ttl")
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, "a sd:Ass") })
  const md = h.contents.value
  assert.ok(md.includes("Assets fill positions"), md)
  assert.ok(md.includes("vocab.ttl"), md)
})

test("completion after 'sd:' offers project terms, not SPARQL Service Description", async () => {
  const uri = uriOf(path.join(dir, "data", "__scratch.ttl"))
  const text = "obj:x a sd:\n"
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, "a sd:") })
  const labels = res.items.map((i) => i.label)
  assert.ok(labels.includes("sd:Asset"), labels.join(", "))
  assert.ok(!labels.includes("sd:Service"), "offered sd:Service from the W3C vocabulary")
  assert.equal(labels[0], "sd:Asset", "classes first after 'a'") // Asset < MeasurementPoint < ... by rank, then length
})

test("implementation on a class lists its subclasses and instances", async () => {
  const { uri, text } = await open("core/vocab.ttl")
  const locs = await s.request("textDocument/implementation", { textDocument: { uri }, position: positionOf(text, "sd:PhysicalObj") })
  const where = locs.map((l) => `${path.basename(l.uri)}:${l.range.start.line}`).sort()
  assert.deepEqual(where, ["vocab.ttl:19", "vocab.ttl:9"], where.join(" "))
})
