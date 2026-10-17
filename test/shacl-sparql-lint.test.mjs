// SHACL-SPARQL query lint as you type: a malformed sh:select is reported on
// the query literal and the report goes once the query is fixed. The query's
// prefixes come from sh:declare in another workspace file.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir } from "./lsp-client.mjs"

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
