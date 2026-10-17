// The import-aware prefix fix: for an undeclared prefix whose namespace an
// ontology in the workspace describes, an ontology document gets a second fix
// that also adds `owl:imports` of that ontology.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf } from "./lsp-client.mjs"

const A = `@prefix a: <http://ex.org/a#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

<http://ex.org/a> a owl:Ontology .

a:Pump a owl:Class ; rdfs:label "Pump" .
a:Valve a owl:Class ; rdfs:label "Valve" .
`

let ws, s
before(async () => {
  ws = tempDir("turtle-prefix-import-")
  fs.writeFileSync(path.join(ws, "a.ttl"), A)
  s = await startServer({ cwd: ws })
  await s.request("workspace/symbol", { query: "" })
})
after(async () => {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

async function fixesFor(name, text, prefix) {
  const uri = uriOf(path.join(ws, name))
  s.open(uri, text)
  const diags = (await s.diagnosticsFor(uri)).filter((d) => d.code === "undeclared-prefix")
  assert.equal(diags.length, 1, JSON.stringify(diags))
  const actions = await s.request("textDocument/codeAction", {
    textDocument: { uri },
    range: diags[0].range,
    context: { diagnostics: diags },
  })
  const fixes = actions.filter((a) => a.title.startsWith(`Add @prefix ${prefix}:`) || a.title.startsWith(`Add prefix '${prefix}:'`))
  return { uri, fixes }
}

function offsetOf(text, pos) {
  const lines = text.split("\n")
  let off = 0
  for (let i = 0; i < pos.line; i++) off += lines[i].length + 1
  return off + pos.character
}

function apply(text, edits) {
  const sorted = edits
    .map((e, i) => ({ start: offsetOf(text, e.range.start), end: offsetOf(text, e.range.end), newText: e.newText, i }))
    .sort((x, y) => y.start - x.start || y.i - x.i)
  for (const e of sorted) text = text.slice(0, e.start) + e.newText + text.slice(e.end)
  return text
}

const B = `@prefix owl: <http://www.w3.org/2002/07/owl#> .

<http://ex.org/b> a owl:Ontology ;
  owl:versionInfo "1" .

<http://ex.org/b#p1> a a:Pump .
`

test("an ontology using an undeclared prefix for a workspace ontology's namespace gets both fixes", async () => {
  const { uri, fixes } = await fixesFor("b.ttl", B, "a")
  assert.equal(fixes.length, 2, fixes.map((f) => f.title).join("\n"))
  assert.match(fixes[0].title, /^Add @prefix a: <http:\/\/ex\.org\/a#> \(used in \d+ files?\)$/)
  assert.equal(fixes[1].title, "Add prefix 'a:' and import <http://ex.org/a>")
  assert.equal(fixes[0].isPreferred, true)
  const plain = fixes[0].edit.changes[uri]
  const both = fixes[1].edit.changes[uri]
  assert.deepEqual(both[0], plain[0], "same prefix declaration edit")

  const fixed = apply(B, both)
  assert.match(fixed, /^@prefix a: <http:\/\/ex\.org\/a#> \.$/m)
  assert.match(fixed, /<http:\/\/ex\.org\/b> owl:imports <http:\/\/ex\.org\/a> \./)
  const fixedUri = uriOf(path.join(ws, "b-fixed.ttl"))
  s.open(fixedUri, fixed)
  const diags = await s.diagnosticsFor(fixedUri)
  const bad = diags.filter((d) => d.code === "syntax" || d.code === "undeclared-prefix")
  assert.deepEqual(bad, [], fixed)
})

test("without an owl: prefix the import uses the full predicate IRI", async () => {
  const text = `<http://ex.org/c> a <http://www.w3.org/2002/07/owl#Ontology> .
<http://ex.org/c#x> a a:Valve .
`
  const { uri, fixes } = await fixesFor("c.ttl", text, "a")
  assert.equal(fixes.length, 2, fixes.map((f) => f.title).join("\n"))
  const fixed = apply(text, fixes[1].edit.changes[uri])
  assert.ok(
    fixed.includes("<http://ex.org/c> <http://www.w3.org/2002/07/owl#imports> <http://ex.org/a> ."),
    fixed,
  )
  const fixedUri = uriOf(path.join(ws, "c-fixed.ttl"))
  s.open(fixedUri, fixed)
  const bad = (await s.diagnosticsFor(fixedUri)).filter((d) => d.code === "syntax" || d.code === "undeclared-prefix")
  assert.deepEqual(bad, [], fixed)
})

test("an ontology that already imports it gets only the plain fix", async () => {
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .

<http://ex.org/d> a owl:Ontology ;
  owl:imports <http://ex.org/a> .

<http://ex.org/d#p1> a a:Pump .
`
  const { fixes } = await fixesFor("d.ttl", text, "a")
  assert.equal(fixes.length, 1, fixes.map((f) => f.title).join("\n"))
  assert.match(fixes[0].title, /^Add @prefix a: <http:\/\/ex\.org\/a#>/)
})

test("a data file without owl:Ontology gets only the plain fix", async () => {
  const text = "<http://ex.org/data#p1> a a:Pump .\n"
  const { fixes } = await fixesFor("data.ttl", text, "a")
  assert.equal(fixes.length, 1, fixes.map((f) => f.title).join("\n"))
  assert.match(fixes[0].title, /^Add @prefix a: <http:\/\/ex\.org\/a#>/)
})

test("a bundled vocabulary gets no import fix", async () => {
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .

<http://ex.org/e> a owl:Ontology .

<http://ex.org/e#x> foaf:name "x" .
`
  const { fixes } = await fixesFor("e.ttl", text, "foaf")
  assert.ok(fixes.length >= 1)
  assert.ok(fixes.every((f) => f.title.startsWith("Add @prefix foaf:")), fixes.map((f) => f.title).join("\n"))
})
