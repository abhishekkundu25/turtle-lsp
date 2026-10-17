// SHACL-SPARQL constraints through the server: results appear on save, on the
// triple they are about, with sh:message templates filled from the solution.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir } from "./lsp-client.mjs"

const SHAPES = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix ex: <http://example.com/plant#> .

ex:prefixes sh:declare [ sh:prefix "ex" ; sh:namespace "http://example.com/plant#"^^xsd:anyURI ] .

ex:UnitShape a sh:NodeShape ;
    sh:targetClass ex:Signal ;
    sh:sparql [
        a sh:SPARQLConstraint ;
        sh:prefixes ex:prefixes ;
        sh:message "Signal {$this} measures {?value} but has no unit" ;
        sh:select """
            SELECT $this ?value WHERE {
                $this ex:measures ?value .
                FILTER NOT EXISTS { $this ex:unit ?u }
            }
        """
    ] .
`
const GOOD = `@prefix ex: <http://example.com/plant#> .
ex:s1 a ex:Signal ; ex:measures ex:Temperature ; ex:unit ex:Celsius .
`
const BAD = `@prefix ex: <http://example.com/plant#> .
ex:s1 a ex:Signal ; ex:measures ex:Temperature .
`
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const sparql = (d) => d.filter((x) => String(x.code).startsWith("shacl-violation"))

async function until(s, uri, ok, ms = 5000) {
  const end = Date.now() + ms
  for (;;) {
    const d = await s.diagnosticsFor(uri, ms)
    if (ok(d)) return d
    if (Date.now() > end) throw new Error(`timed out; last: ${JSON.stringify(d)}`)
    await sleep(25)
  }
}

test("a SPARQL constraint's violation appears on save, with its message filled in", async () => {
  const dir = tempDir("turtle-sparql-")
  fs.writeFileSync(path.join(dir, "shapes.ttl"), SHAPES)
  fs.writeFileSync(path.join(dir, "data.ttl"), GOOD)
  const s = await startServer({ cwd: dir, initializationOptions: { shacl: { onType: false } } })
  const uri = uriOf(path.join(dir, "data.ttl"))
  try {
    await s.request("workspace/symbol", { query: "" })
    s.open(uri, GOOD)
    assert.deepEqual(sparql(await s.diagnosticsFor(uri)), [])

    s.change(uri, BAD)
    fs.writeFileSync(path.join(dir, "data.ttl"), BAD)
    s.notify("textDocument/didSave", { textDocument: { uri } })
    const [d] = sparql(await until(s, uri, (d) => sparql(d).length > 0))
    // Templates are filled with full IRIs, as reference validators fill them.
    assert.equal(d.message, "Signal http://example.com/plant#s1 measures http://example.com/plant#Temperature but has no unit")
    assert.equal(d.severity, 1)
    assert.equal(d.range.start.line, 1)
    assert.ok(d.relatedInformation?.[0]?.location.uri.endsWith("/shapes.ttl"), JSON.stringify(d))

    s.change(uri, GOOD)
    fs.writeFileSync(path.join(dir, "data.ttl"), GOOD)
    s.notify("textDocument/didSave", { textDocument: { uri } })
    await until(s, uri, (d) => sparql(d).length === 0)
  } finally {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
