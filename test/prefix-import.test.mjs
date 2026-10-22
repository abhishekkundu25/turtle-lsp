// The import-aware prefix fix: for an undeclared prefix whose namespace an
// ontology in the workspace describes, an ontology document gets a second fix
// that also adds `owl:imports` of that ontology.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf, positionOf } from "./lsp-client.mjs"

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

// ---------- which declarations are in scope where ----------

const OWL = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
const FOAF = "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n"

async function noProblemsAfter(name, fixed) {
  const uri = uriOf(path.join(ws, name))
  s.open(uri, fixed)
  const diags = await s.diagnosticsFor(uri)
  s.close(uri)
  const bad = diags.filter((d) => ["syntax", "undeclared-prefix", "duplicate-prefix", "redefined-prefix"].includes(d.code))
  assert.deepEqual(bad, [], fixed)
}

test("the fix declares the prefix above its first use, not after the last declaration", async () => {
  const text = `${OWL}\nfoaf:x a owl:Thing .\n\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\nrdfs:a a owl:Thing .\n`
  const { uri, fixes } = await fixesFor("mid.ttl", text, "foaf")
  const fixed = apply(text, fixes[0].edit.changes[uri])
  assert.equal(fixed, `${OWL}${FOAF}\nfoaf:x a owl:Thing .\n\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\nrdfs:a a owl:Thing .\n`)
  await noProblemsAfter("mid-fixed.ttl", fixed)
})

test("with no declaration above the use the fix goes to the top, under the header comment", async () => {
  const text = `# header\nfoaf:x a <http://x/T> .\n${OWL}`
  const { uri, fixes } = await fixesFor("top.ttl", text, "foaf")
  const fixed = apply(text, fixes[0].edit.changes[uri])
  assert.equal(fixed, `# header\n${FOAF}foaf:x a <http://x/T> .\n${OWL}`)
  await noProblemsAfter("top-fixed.ttl", fixed)
})

test("inserted lines follow a CRLF file's line ending, imports included", async () => {
  const crlf = (t) => t.replace(/\n/g, "\r\n")
  const bare = /(?<!\r)\n/
  const text = crlf(`${OWL}\nfoaf:x a owl:Thing .\n`)
  const { uri, fixes } = await fixesFor("crlf.ttl", text, "foaf")
  const fixed = apply(text, fixes[0].edit.changes[uri])
  assert.equal(fixed, crlf(`${OWL}${FOAF}\nfoaf:x a owl:Thing .\n`))
  const ontology = crlf(B)
  const both = await fixesFor("crlf-b.ttl", ontology, "a")
  const withImport = apply(ontology, both.fixes[1].edit.changes[both.uri])
  assert.match(withImport, /<http:\/\/ex\.org\/b> owl:imports <http:\/\/ex\.org\/a> \./)
  assert.doesNotMatch(withImport, bare)
})

test("completion adds the @prefix of a vocabulary declared only further down", async () => {
  const text = `<http://x/a> <http://x/p> foaf:Pers .\n${FOAF}`
  const uri = uriOf(path.join(ws, "later.ttl"))
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  try {
    const list = await s.request("textDocument/completion", {
      textDocument: { uri },
      position: positionOf(text, "foaf:Pers"),
    })
    const items = Array.isArray(list) ? list : list.items
    const person = items.find((i) => i.label === "foaf:Person")
    assert.ok(person, items.map((i) => i.label).join(", "))
    assert.deepEqual(person.additionalTextEdits, [
      { range: { start: { line: 0, character: 0 }, end: { line: 0, character: 0 } }, newText: FOAF },
    ])
  } finally {
    s.close(uri)
  }
})

test("definition and hover agree on which declaration of a redefined prefix is in scope", async () => {
  const text = "@prefix ex: <http://a.example/> .\nex:x ex:p ex:o .\n@prefix ex: <http://b.example/> .\nex:y ex:p ex:o .\n"
  const uri = uriOf(path.join(ws, "redefined.ttl"))
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  try {
    for (const [needle, ns, line] of [
      ["ex:x", "http://a.example/", 0],
      ["ex:y", "http://b.example/", 2],
    ]) {
      const position = positionOf(text, needle, 1)
      const defs = await s.request("textDocument/definition", { textDocument: { uri }, position })
      assert.deepEqual(defs.map((d) => d.range.start.line), [line], needle)
      const hover = await s.request("textDocument/hover", { textDocument: { uri }, position })
      assert.ok(hover.contents.value.includes(`<${ns}>`), `${needle}: ${hover.contents.value}`)
    }
  } finally {
    s.close(uri)
  }
})

test("only a file named prefixes.ttl may leave prefixes unused", async () => {
  fs.mkdirSync(path.join(ws, "sub"), { recursive: true })
  const unused = async (name) => {
    const uri = uriOf(path.join(ws, name))
    s.open(uri, "@prefix zz: <http://zz.example/> .\n")
    const diags = await s.diagnosticsFor(uri)
    s.close(uri)
    return diags.filter((d) => d.code === "unused-prefix").length
  }
  assert.equal(await unused("myprefixes.ttl"), 1)
  assert.equal(await unused("sub/prefixes.ttl"), 0)
})

test("a duplicate prefix's fix is titled for what it removes", async () => {
  const text = `${OWL}${OWL}<http://x/a> a owl:Thing .\n`
  const uri = uriOf(path.join(ws, "dup.ttl"))
  s.open(uri, text)
  const diags = (await s.diagnosticsFor(uri)).filter((d) => d.code === "duplicate-prefix")
  assert.equal(diags.length, 1)
  const actions = await s.request("textDocument/codeAction", {
    textDocument: { uri },
    range: diags[0].range,
    context: { diagnostics: diags },
  })
  s.close(uri)
  assert.deepEqual(actions.filter((a) => a.kind === "quickfix").map((a) => a.title), ["Remove duplicate prefix"])
  const fix = actions.find((a) => a.kind === "quickfix")
  assert.equal(apply(text, fix.edit.changes[uri]), `${OWL}<http://x/a> a owl:Thing .\n`)
})
