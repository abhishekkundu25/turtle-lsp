// Unknown-term check: cost on files with thousands of unknown terms, and terms
// the workspace describes count as known whatever the bundled snapshot says.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf } from "./lsp-client.mjs"

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
  assert.equal(fix.edit.changes[uri][0].newText, "foaf:name")
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
