// SHACL-SPARQL query lint as you type: a malformed sh:select is reported on
// the query literal and the report goes once the query is fixed. The query's
// prefixes come from sh:declare in another workspace file.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { spawnSync } from "node:child_process"
import { startServer, uriOf, tempDir, serverPath } from "./lsp-client.mjs"

const dir = tempDir("shacl-sparql-lint-")
fs.writeFileSync(
  path.join(dir, "prefixes.ttl"),
  `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
<http://example.com/ns> sh:declare [ sh:prefix "ex" ; sh:namespace "http://example.com/ns#"^^xsd:anyURI ] .
`
)

const head = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.com/ns#> .
`
const shapes = (where) => `${head}ex:S a sh:NodeShape ;
    sh:targetNode ex:x ;
    sh:sparql [
        sh:prefixes <http://example.com/ns> ;
        sh:select """
            SELECT $this WHERE {
                ${where}
            }""" ] .
`

let s
before(async () => {
  s = await startServer({ cwd: dir })
  await s.request("workspace/symbol", { query: "" })
})
after(async () => {
  await s?.stop()
  fs.rmSync(dir, { recursive: true, force: true })
})

const sparql = (diags) => diags.filter((d) => String(d.code).startsWith("shacl-sparql"))

test("a malformed query is reported on the literal and clears when fixed", async () => {
  const uri = uriOf(path.join(dir, "shapes.ttl"))
  const broken = shapes("$this ex:p ?v ) .")
  s.open(uri, broken)
  const diags = sparql(await s.diagnosticsFor(uri))
  assert.equal(diags.length, 1, JSON.stringify(diags))
  assert.equal(diags[0].code, "shacl-sparql-syntax")
  assert.equal(diags[0].severity, 1)
  const line = broken.split("\n").findIndex((l) => l.includes("?v )"))
  const character = broken.split("\n")[line].indexOf(")")
  assert.deepEqual(diags[0].range, { start: { line, character }, end: { line, character: character + 1 } })

  s.change(uri, shapes("$this ex:p ?v . FILTER NOT EXISTS { ?v a ex:C }"))
  assert.deepEqual(sparql(await s.diagnosticsFor(uri)), [])
  s.close(uri)
})

// A path this long overflowed the stack and aborted the server's SHACL thread
// while it indexed the folder (no file open), and `check`.
const longPath = shapes(`$this ex:p${"/ex:p".repeat(20000)} ?v .`)

test("a 20,000-step path in sh:select is one error, not an abort", async () => {
  const uri = uriOf(path.join(dir, "long-path.ttl"))
  s.open(uri, longPath)
  const diags = sparql(await s.diagnosticsFor(uri))
  assert.equal(diags.length, 1, JSON.stringify(diags.map((d) => d.message)))
  assert.equal(diags[0].code, "shacl-sparql-syntax")
  assert.equal(diags[0].severity, 1)
  assert.match(diags[0].message, /too large to check \(more than \d+ steps in one property path\)/)
  s.close(uri)
  // The server still answers.
  assert.ok(Array.isArray(await s.request("workspace/symbol", { query: "" })))
})

test("check reports a 20,000-step path in sh:select and exits 1, no abort", () => {
  const ws = tempDir("shacl-sparql-long-")
  try {
    fs.writeFileSync(path.join(ws, "shapes.ttl"), longPath)
    fs.writeFileSync(path.join(ws, "prefixes.ttl"), fs.readFileSync(path.join(dir, "prefixes.ttl")))
    const r = spawnSync(serverPath, ["check", "--root", ws], {
      cwd: ws,
      encoding: "utf8",
      timeout: 60000,
      env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1" },
    })
    assert.equal(r.signal, null, r.stderr)
    assert.equal(r.status, 1, r.stderr)
    const lines = r.stdout.split("\n").filter((l) => l.includes("shacl-sparql"))
    assert.equal(lines.length, 1, r.stdout)
    assert.match(lines[0], /^shapes\.ttl:\d+:\d+: error\[shacl-sparql-syntax\] .*too large to check/)
  } finally {
    fs.rmSync(ws, { recursive: true, force: true })
  }
})

test("a SPARQL target may use MINUS, VALUES and SERVICE as the validator allows", async () => {
  const uri = uriOf(path.join(dir, "targets.ttl"))
  const text = `${head}ex:S a sh:NodeShape ;
    sh:target [ a sh:SPARQLTarget ; sh:prefixes <http://example.com/ns> ;
        sh:select "SELECT ?this WHERE { ?this a ex:C MINUS { ?this ex:gone true } VALUES ?x { 1 } }" ] .
`
  s.open(uri, text)
  assert.deepEqual(sparql(await s.diagnosticsFor(uri)), [])
  // A constraint may not.
  s.change(uri, shapes("$this ex:p ?v MINUS { $this ex:q ?v }"))
  const found = sparql(await s.diagnosticsFor(uri))
  assert.equal(found.length, 1, JSON.stringify(found))
  assert.equal(found[0].code, "shacl-sparql-unsupported")
  s.close(uri)
})
