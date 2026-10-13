// TriG and N-Triples documents (test/fixtures/datasets): a headerless TriG
// file and a Turtle vocabulary sharing prefixes.ttl, and an N-Triples file,
// which gets no prefixes. Documents are opened with their LSP language id.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const dir = path.join(fixtures, "datasets")
const read = (f) => fs.readFileSync(path.join(dir, f), "utf8")
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.message}`).join("\n")
const problems = (diags) => diags.filter((d) => d.severity <= 2)
const languageOf = (f) => (f.endsWith(".trig") ? "trig" : f.endsWith(".nt") ? "ntriples" : "turtle")

let s
before(async () => {
  s = await startServer({ cwd: dir })
})
after(async () => {
  await s.stop()
})

let version = 1
async function openAs(uri, text, languageId) {
  s.conn.sendNotification("textDocument/didOpen", { textDocument: { uri, languageId, version: version++, text } })
  return { uri, text, diags: await s.diagnosticsFor(uri) }
}
const open = (f) => openAs(uriOf(path.join(dir, f)), read(f), languageOf(f))
const where = (locs) => locs.map((l) => `${path.basename(l.uri)}:${l.range.start.line}`).sort()

for (const f of ["graphs.trig", "vocab.ttl", "people.nt"]) {
  test(`${f}: no problems`, async () => {
    const { diags } = await open(f)
    assert.deepEqual(problems(diags), [], fmt(problems(diags)))
  })
}

test("definition from a .trig graph block lands on the class in a .ttl file", async () => {
  const { uri, text } = await open("graphs.trig")
  const locs = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(text, "a dx:Pers") })
  assert.deepEqual(where(locs), ["vocab.ttl:1"])
})

test("definition from an .nt file lands on subjects inside TriG graph blocks", async () => {
  const { uri, text } = await open("people.nt")
  const locs = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(text, "person/alice>", 3) })
  assert.deepEqual(where(locs), ["graphs.trig:14", "graphs.trig:5"])
})

test("a graph name is defined by its block and by statements about it", async () => {
  const { uri, text } = await open("graphs.trig")
  const position = positionOf(text, "dct:source g:peo")
  const defs = await s.request("textDocument/definition", { textDocument: { uri }, position })
  assert.deepEqual(where(defs), ["graphs.trig:1", "graphs.trig:4"])
  const refs = await s.request("textDocument/references", { textDocument: { uri }, position, context: { includeDeclaration: false } })
  assert.deepEqual(where(refs), ["graphs.trig:24"])
})

test("hover in a .trig file shows the label and comment from the vocabulary", async () => {
  const { uri, text } = await open("graphs.trig")
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, "a dx:Pers") })
  assert.match(h.contents.value, /Someone the dataset describes/)
})

test("completion inside a graph block offers workspace terms", async () => {
  const text = "g:new {\n    p:alice dx:\n}\n"
  const { uri } = await openAs(uriOf(path.join(dir, "__scratch.trig")), text, "trig")
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, "p:alice dx:") })
  const labels = res.items.map((i) => i.label)
  assert.ok(labels.includes("dx:knows") && labels.includes("dx:name"), labels.join(", "))
})

test("syntax errors in a graph block are reported and the rest still parses", async () => {
  const text = 'g:x {\n    p:a dx:name "open .\n    p:b dx:name "B" .\n'
  const { diags } = await openAs(uriOf(path.join(dir, "__broken.trig")), text, "trig")
  const msgs = diags.map((d) => d.message).join("\n")
  assert.match(msgs, /Unterminated string/)
  assert.match(msgs, /Unclosed '\{'/)
})

test("an .nt file with @prefix gets an error", async () => {
  const text = "@prefix ex: <http://example.org/> .\n<http://example.org/a> <http://example.org/p> <http://example.org/o> .\n"
  const { diags } = await openAs(uriOf(path.join(dir, "__prefix.nt")), text, "ntriples")
  const errors = diags.filter((d) => d.severity === 1)
  assert.ok(
    errors.some((d) => /N-Triples does not allow directives/.test(d.message) && d.range.start.line === 0),
    fmt(diags)
  )
})

test("prefixes.ttl does not apply to N-Triples", async () => {
  const text = "p:alice <https://data.example.net/def/name> \"Alice\" .\n"
  const uri = uriOf(path.join(dir, "__pname.nt"))
  const { diags } = await openAs(uri, text, "ntriples")
  assert.match(diags.map((d) => d.message).join("\n"), /not prefixed names/)
  // Resolved through prefixes.ttl, hover would describe the IRI.
  const hover = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, "p:al") })
  assert.equal(hover, null)
})

test("the language id decides the syntax", async () => {
  const text = "@prefix ex: <http://example.org/> .\nex:g { ex:a ex:p ex:o }\n"
  const trig = await openAs("untitled:dataset-1", text, "trig")
  assert.deepEqual(trig.diags.filter((d) => d.severity === 1), [], fmt(trig.diags))
  const turtle = await openAs("untitled:dataset-2", text, "turtle")
  assert.ok(turtle.diags.some((d) => d.severity === 1), "TriG block accepted as Turtle")
})

test("graph blocks fold", async () => {
  const { uri } = await open("graphs.trig")
  const ranges = await s.request("textDocument/foldingRange", { textDocument: { uri } })
  const spans = ranges.map((r) => `${r.startLine}-${r.endLine}`)
  for (const want of ["4-10", "13-15", "18-19", "22-24"]) assert.ok(spans.includes(want), `${want} not in ${spans.join(" ")}`)
})

test("outline shows graph blocks as containers of their subjects", async () => {
  const { uri } = await open("graphs.trig")
  const syms = await s.request("textDocument/documentSymbol", { textDocument: { uri } })
  const outline = syms.map((x) => `${x.name}${x.children ? `[${x.children.map((c) => c.name).join(" ")}]` : ""}`)
  assert.deepEqual(outline, ["g:people", "g:people[p:alice p:bob]", "g:links[p:alice p:bob]", "{ } default graph[p:carol]", "_:draft[p:dave]"])
  const people = syms.find((x) => x.children && x.name === "g:people")
  assert.equal(people.kind, 3) // Namespace
})

test("the workspace index covers .trig and .nt files", async () => {
  const dave = await s.request("workspace/symbol", { query: "dave" })
  assert.ok(dave.some((x) => x.location.uri.endsWith("/graphs.trig")), JSON.stringify(dave))
  const erin = await s.request("workspace/symbol", { query: "erin" })
  assert.ok(erin.some((x) => x.location.uri.endsWith("/people.nt")), JSON.stringify(erin))
})

test("N-Triples gets no prefix help: no completions, no add-prefix fix", async () => {
  const uri = uriOf(path.join(dir, "__nt-prefix.nt"))
  const text = "<http://example.com/a> foaf:name \"A\" .\n"
  const { diags } = await openAs(uri, text, "ntriples")
  assert.ok(diags.some((d) => d.severity === 1), fmt(diags))
  assert.equal(diags.filter((d) => d.code === "undeclared-prefix").length, 0, fmt(diags))
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, "foaf:") })
  const items = Array.isArray(res) ? res : res.items
  assert.deepEqual(items, [])
})
