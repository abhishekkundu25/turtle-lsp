// Reference lenses: textDocument/codeLens and codeLens/resolve.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir } from "./lsp-client.mjs"

const VOCAB = `@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:Person a rdfs:Class .

ex:name a ex:Property ;
    rdfs:domain ex:Person .

ex:alice a ex:Person ;
    ex:name "Alice" .
`
const DATA = `@prefix ex: <http://example.org/> .
ex:bob a ex:Person ;
    ex:name "Bob" ;
    ex:knows ex:alice .
`
const JSONLD = `{
  "@context": {"ex": "http://example.org/"},
  "@id": "ex:carol",
  "@type": "ex:Person",
  "ex:knows": {"@id": "ex:alice"}
}
`

let ws, s
const at = (name) => uriOf(path.join(ws, name))
before(async () => {
  ws = tempDir("turtle-lens-")
  fs.writeFileSync(path.join(ws, "vocab.ttl"), VOCAB)
  fs.writeFileSync(path.join(ws, "data.ttl"), DATA)
  fs.writeFileSync(path.join(ws, "carol.jsonld"), JSONLD)
  fs.writeFileSync(path.join(ws, "q.rq"), "PREFIX ex: <http://example.org/>\nSELECT * { ?s a ex:Person }\n")
  s = await startServer({ cwd: ws })
  s.open(at("vocab.ttl"), VOCAB)
  await s.diagnosticsFor(at("vocab.ttl"))
})
after(async () => {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

const lenses = (uri) => s.request("textDocument/codeLens", { textDocument: { uri } })
const resolveAll = (ls) => Promise.all(ls.map((l) => s.request("codeLens/resolve", l)))

test("the server offers lenses with resolve, and no showReferences command", () => {
  assert.deepEqual(s.init.capabilities.codeLensProvider, { resolveProvider: true })
  const cmds = s.init.capabilities.executeCommandProvider?.commands ?? []
  assert.ok(!cmds.includes("turtle.showReferences"), cmds.join(", "))
})

test("one lens per described subject, on its first description, with no command", async () => {
  const ls = await lenses(at("vocab.ttl"))
  assert.deepEqual(
    ls.map((l) => [l.range.start.line, l.range.start.character, l.range.end.character]),
    [
      [3, 0, 9],
      [5, 0, 7],
      [8, 0, 8],
    ],
  )
  for (const l of ls) {
    assert.equal(l.command, undefined)
    assert.equal(l.data.uri, at("vocab.ttl"))
  }
  assert.deepEqual(
    ls.map((l) => l.data.iri),
    ["http://example.org/Person", "http://example.org/name", "http://example.org/alice"],
  )
})

test("resolve counts the workspace's references, declarations left out", async () => {
  const ls = await resolveAll(await lenses(at("vocab.ttl")))
  // ex:Person: vocab domain, alice's type, bob's type, carol's @type.
  // ex:name: alice's and bob's property. ex:alice: data.ttl and carol.jsonld.
  assert.deepEqual(
    ls.map((l) => l.command.title),
    ["4 references", "2 references", "2 references"],
  )
})

test("the lens command is turtle.showReferences with uri, position and locations", async () => {
  const [, name] = await resolveAll(await lenses(at("vocab.ttl")))
  assert.equal(name.command.command, "turtle.showReferences")
  const [uri, pos, locs] = name.command.arguments
  assert.equal(uri, at("vocab.ttl"))
  assert.deepEqual(pos, { line: 5, character: 0 })
  assert.deepEqual(
    locs.map((l) => [path.basename(new URL(l.uri).pathname), l.range.start.line]).sort(),
    [
      ["data.ttl", 2],
      ["vocab.ttl", 9],
    ],
  )
})

test("a single reference reads 1 reference", async () => {
  const uri = at("__one.ttl")
  const text = "@prefix ex: <http://example.org/> .\nex:solo ex:p ex:o .\nex:x ex:p ex:solo .\n"
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const ls = await resolveAll(await lenses(uri))
  assert.deepEqual(
    ls.map((l) => [l.data.iri, l.command.title]),
    [
      ["http://example.org/solo", "1 reference"],
      ["http://example.org/x", "0 references"],
    ],
  )
  s.close(uri)
})

test("JSON-LD lenses are the nodes with an @id the document describes", async () => {
  const uri = at("carol.jsonld")
  s.open(uri, JSONLD, "jsonld")
  await s.diagnosticsFor(uri)
  const ls = await resolveAll(await lenses(uri))
  assert.deepEqual(
    ls.map((l) => [l.data.iri, l.range.start.line, l.command.title]),
    [["http://example.org/carol", 2, "0 references"]],
  )
})

test("SPARQL documents get no reference lenses, only Run query", async () => {
  const uri = at("q.rq")
  s.open(uri, fs.readFileSync(path.join(ws, "q.rq"), "utf8"), "sparql")
  const ls = await lenses(uri)
  assert.deepEqual(ls.map((l) => l.command?.command), ["turtle.runQuery"])
})

test("codeLens.references off gives no lenses", async () => {
  const off = await startServer({ cwd: ws, initializationOptions: { codeLens: { references: false } } })
  try {
    const uri = at("vocab.ttl")
    off.open(uri, VOCAB)
    await off.diagnosticsFor(uri)
    assert.deepEqual(await off.request("textDocument/codeLens", { textDocument: { uri } }), [])
  } finally {
    await off.stop()
  }
})

// The 30,000-line document of bench/perf.mjs: 5000 subjects of 6 lines.
function bigDoc(n) {
  let out = `@prefix ex: <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix schema: <https://schema.org/> .
`
  for (let i = 0; i < n; i++) {
    out += `
ex:p${i} a foaf:Person ;
    foaf:name "Person ${i}" ;
    rdfs:comment """A longer
multi-line description ${i}""" ;
    foaf:knows ex:p${(i + 1) % n} .
`
  }
  return out
}

// How long the lenses of this document take: latency.test.mjs.
test("lenses on a 30,000-line document: one per subject, each resolved", { timeout: 60_000 }, async () => {
  const dir = tempDir("turtle-lens-big-")
  const big = await startServer({ cwd: dir })
  try {
    const text = bigDoc(5000)
    const uri = uriOf(path.join(dir, "big.ttl"))
    big.open(uri, text)
    await big.diagnosticsFor(uri, 30_000)
    const ls = await big.request("textDocument/codeLens", { textDocument: { uri } })
    assert.equal(ls.length, 5000)
    // Unresolved in the list: the counts come with codeLens/resolve.
    assert.ok(ls.every((l) => l.command === undefined || l.command === null), JSON.stringify(ls[0]))
    const resolved = await Promise.all(ls.map((l) => big.request("codeLens/resolve", l)))
    assert.ok(resolved.every((l) => l.command.title === "1 reference"))
  } finally {
    await big.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
