// Unknown-term check: cost on files with thousands of unknown terms, and terms
// the workspace describes count as known whatever the bundled snapshot says.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf } from "./lsp-client.mjs"
import { apply } from "./query-client.mjs"

const header = `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.org/> .
`
const unknown = (diags) => diags.filter((d) => d.code === "unknown-term")

let s, ws
before(async () => {
  ws = tempDir("turtle-unknown-term-")
  s = await startServer({ cwd: ws })
})
after(async () => {
  try {
    await s?.stop()
  } finally {
    fs.rmSync(ws, { recursive: true, force: true })
  }
})

// How long this takes: latency.test.mjs.
test("10,000 uses of 2,000 distinct unknown terms are each reported", async () => {
  // Locals shaped like real foaf names (6-12 letters), so most candidates are
  // within the length window and the suggestion search does real work.
  const letters = "abcdefghijklmnopqrstuvwxyz"
  const local = (i) => {
    let s = ""
    for (let n = i, k = 0; k < 6 + (i % 7); k++, n = Math.floor(n / 3) + k * 7) s += letters[(n * 31 + k * 17) % 26]
    return `${s}${i}`
  }
  const lines = []
  for (let i = 0; i < 10_000; i++) lines.push(`ex:s${i % 50} foaf:${local(i % 2000)} "v" .`)
  const text = header + lines.join("\n") + "\n"
  const uri = uriOf(path.join(ws, "big.ttl"))
  s.open(uri, text)
  const diags = await s.diagnosticsFor(uri, 30_000)
  assert.equal(unknown(diags).length, 10_000)
  // One per use, on the use: 2,000 distinct terms, each on 5 lines.
  assert.equal(new Set(unknown(diags).map((d) => d.range.start.line)).size, 10_000)
})

test("terms the document describes are known; a typo keeps its warning and fix", async () => {
  const text = `${header}foaf:newTerm a rdf:Property ;
  rdfs:label "a term newer than the bundled snapshot" .
foaf:Gadget a rdfs:Class .
ex:alice a foaf:Gadget ;
  foaf:newTerm "x" ;
  foaf:namee "Alice" ;
  foaf:nmae "Alice" .
`
  const uri = uriOf(path.join(ws, "described.ttl"))
  s.open(uri, text)
  const diags = await s.diagnosticsFor(uri)
  const w = unknown(diags)
  assert.deepEqual(
    w.map((d) => [d.range.start.line, d.message]),
    [
      [9, "'namee' is not defined in the foaf vocabulary <http://xmlns.com/foaf/0.1/>. Did you mean 'name'?"],
      // Swapping two neighbouring letters is one edit.
      [10, "'nmae' is not defined in the foaf vocabulary <http://xmlns.com/foaf/0.1/>. Did you mean 'name'?"],
    ]
  )
  assert.deepEqual(w[0].data, { replace: "foaf:name" })
  const actions = await s.request("textDocument/codeAction", {
    textDocument: { uri },
    range: w[0].range,
    context: { diagnostics: [w[0]] },
  })
  const fix = actions.find((a) => a.title === "Change to 'foaf:name'")
  assert.ok(fix, JSON.stringify(actions))
  assert.equal(apply(text, fix.edit.changes[uri]).split("\n")[9], '  foaf:name "Alice" ;')
})

test("GeoSPARQL functions are not unknown terms, in Turtle or SPARQL", async () => {
  const geof = "http://www.opengis.net/def/function/geosparql/"
  const ttl = uriOf(path.join(ws, "geo.ttl"))
  s.open(ttl, `@prefix geof: <${geof}> .\n@prefix ex: <http://example.org/> .\nex:q ex:uses geof:sfWithin , geof:distance .\n`)
  assert.deepEqual(unknown(await s.diagnosticsFor(ttl)), [])
  const rq = uriOf(path.join(ws, "geo.rq"))
  s.open(rq, `PREFIX geof: <${geof}>\nSELECT * WHERE { ?a ?p ?b FILTER(geof:sfWithin(?a, ?b) && geof:distance(?a, ?b) > 1) }\n`, "sparql")
  assert.deepEqual(unknown(await s.diagnosticsFor(rq)), [])
})

test("a local name with a colon is found: og:audio:album is known, og:audio:albm is not", async () => {
  const text = `@prefix og: <http://ogp.me/ns#> .\n@prefix ex: <http://example.org/> .\nex:s ex:p og:audio:type , og:audio:album , og:audio:albm .\n`
  const uri = uriOf(path.join(ws, "og.ttl"))
  s.open(uri, text)
  const w = unknown(await s.diagnosticsFor(uri))
  assert.deepEqual(
    w.map((d) => d.message),
    ["'audio:albm' is not defined in the og vocabulary <http://ogp.me/ns#>. Did you mean 'audio:album'?"],
  )
  assert.deepEqual(w[0].data, { replace: "og:audio:album" })
})

test("https://schema.org/ is the schema vocabulary", async () => {
  const text = `@prefix s: <https://schema.org/> .\n@prefix ex: <http://example.org/> .\nex:a a s:Person ; s:nme "x" .\nex:b a <https://schema.org/Persn> .\n`
  const uri = uriOf(path.join(ws, "schema-https.ttl"))
  s.open(uri, text)
  const w = unknown(await s.diagnosticsFor(uri))
  assert.deepEqual(
    w.map((d) => [d.range.start.line, d.message]),
    [
      [2, "'nme' is not defined in the schema vocabulary <http://schema.org/>. Did you mean 'name'?"],
      [3, "'Persn' is not defined in the schema vocabulary <http://schema.org/>. Did you mean 'Person'?"],
    ],
  )
})

test("the terms only the specifications define are known (issue #106)", async () => {
  const text = `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .

ex:Small a rdfs:Datatype ;
  owl:onDatatype xsd:integer ;
  owl:withRestrictions ( [ xsd:maxExclusive 10 ] [ xsd:pattern "[0-9]+" ] ) .
ex:ratio rdfs:range owl:rational .
ex:claim rdf:reifies ex:x .
`
  const uri = uriOf(path.join(ws, "issue-106.ttl"))
  s.open(uri, text)
  assert.deepEqual(unknown(await s.diagnosticsFor(uri)), [])
})

test("rdf:_1 and every later membership property are known, in Turtle and SPARQL; rdf:_0, rdf:_01 and rdf:_x are not", async () => {
  const ttl = uriOf(path.join(ws, "container.ttl"))
  s.open(ttl, `${header}ex:c a rdf:Bag ; rdf:_1 ex:a ; rdf:_12 ex:b ; rdf:_0 ex:c ; rdf:_01 ex:d ; rdf:_x ex:e .\n`)
  assert.deepEqual(
    unknown(await s.diagnosticsFor(ttl)).map((d) => d.message.split(" is ")[0]),
    ["'_0'", "'_01'", "'_x'"],
  )
  const rq = uriOf(path.join(ws, "container.rq"))
  s.open(rq, `PREFIX rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#>\nSELECT * WHERE { ?c rdf:_1 ?a ; rdf:_12 ?b ; rdf:_0 ?z }\n`, "sparql")
  assert.deepEqual(unknown(await s.diagnosticsFor(rq)).map((d) => d.message.split(" is ")[0]), ["'_0'"])
})

test("EARL is checked in http://www.w3.org/ns/earl#", async () => {
  const text = `@prefix earl: <http://www.w3.org/ns/earl#> .\n@prefix ex: <http://example.org/> .\nex:a a earl:Assertion .\nex:b a earl:Asertion .\n`
  const uri = uriOf(path.join(ws, "earl.ttl"))
  s.open(uri, text)
  const w = unknown(await s.diagnosticsFor(uri))
  assert.deepEqual(
    w.map((d) => [d.range.start.line, d.message]),
    [[3, "'Asertion' is not defined in the earl vocabulary <http://www.w3.org/ns/earl#>. Did you mean 'Assertion'?"]],
  )
  assert.deepEqual(w[0].data, { replace: "earl:Assertion" })
})

test("hover names the vocabulary only for a term it has", async () => {
  const text = `${header}ex:a rdfs:label "a" ; rdfs:lable "b" ; rdf:_3 ex:c .\n`
  const uri = uriOf(path.join(ws, "hover-vocab.ttl"))
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const lines = text.split("\n")
  const hover = async (word) => {
    const line = lines.findIndex((l) => l.includes(word))
    const h = await s.request("textDocument/hover", {
      textDocument: { uri },
      position: { line, character: lines[line].indexOf(word) + word.length - 1 },
    })
    return h.contents.value
  }
  assert.match(await hover("rdfs:label"), /_From the rdfs vocabulary_/)
  assert.doesNotMatch(await hover("rdfs:lable"), /vocabulary/)
  const member = await hover("rdf:_3")
  assert.match(member, /^\*\*rdf:_3\*\* · property/)
  assert.match(member, /RDF Schema/)
})
