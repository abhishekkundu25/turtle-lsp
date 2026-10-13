// SHACL shape lint: one snippet per rule, each with exactly the expected
// `shacl-*` diagnostic at the expected range. The workspace is
// test/fixtures/shacl (vocab.ttl describes the classes, datatype and shapes
// the snippets refer to, and is itself clean).
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures } from "./lsp-client.mjs"

const dir = path.join(fixtures, "shacl")
const H = `@prefix ex: <http://example.com/shop/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
`

let s
before(async () => {
  s = await startServer({ cwd: dir })
  // Requests wait for the initial index; after this, diagnostics see it.
  await s.request("workspace/symbol", { query: "" })
})
after(async () => {
  await s?.stop()
})

let n = 0
async function shaclDiagnostics(body, server = s, at = dir) {
  const uri = uriOf(path.join(at, `__snippet_${n++}.ttl`))
  server.open(uri, H + body)
  const diags = (await server.diagnosticsFor(uri)).filter((d) => String(d.code).startsWith("shacl-"))
  server.close(uri)
  return diags
}

// The range of `needle` (the `nth` occurrence, 0-based) in H + body; with
// `within`, only that substring of the needle.
function rangeOf(body, needle, nth = 0, within = needle) {
  const text = H + body
  let at = -1
  for (let i = 0; i <= nth; i++) at = text.indexOf(needle, at + 1)
  assert.ok(at >= 0, `no ${needle} in snippet`)
  at += needle.indexOf(within)
  const pos = (off) => {
    const lines = text.slice(0, off).split("\n")
    return { line: lines.length - 1, character: lines.at(-1).length }
  }
  return { start: pos(at), end: pos(at + within.length) }
}

async function expectOne(body, code, severity, needle, nth = 0, within = needle) {
  const diags = await shaclDiagnostics(body)
  assert.deepEqual(
    diags.map((d) => ({ code: d.code, severity: d.severity, range: d.range })),
    [{ code, severity, range: rangeOf(body, needle, nth, within) }],
    JSON.stringify(diags, null, 1)
  )
  return diags[0]
}

const ERROR = 1
const WARNING = 2

test("the fixture vocabulary and its shapes are clean", async () => {
  const diags = await shaclDiagnostics(fs.readFileSync(path.join(dir, "vocab.ttl"), "utf8").replace(/^@prefix.*\n/gm, ""))
  assert.deepEqual(diags, [])
})

test("shacl-missing-path: inline property shape without sh:path", async () => {
  const body = "ex:S1 a sh:NodeShape ;\n    sh:targetClass ex:Product ;\n    sh:property [ sh:minCount 1 ] .\n"
  const d = await expectOne(body, "shacl-missing-path", ERROR, "[")
  assert.match(d.message, /sh:path/)
})

test("shacl-missing-path: typed sh:PropertyShape without sh:path", async () => {
  await expectOne('ex:P2 a sh:PropertyShape ;\n    sh:name "p2" .\n', "shacl-missing-path", ERROR, "ex:P2")
})

test("shacl-unknown-class: sh:targetClass naming no known class", async () => {
  const body = "ex:S3 a sh:NodeShape ;\n    sh:targetClass ex:Prodcut .\n"
  const d = await expectOne(body, "shacl-unknown-class", WARNING, "ex:Prodcut")
  assert.match(d.message, /ex:Prodcut/)
})

test("shacl-unknown-class: sh:class on a property shape", async () => {
  await expectOne(
    "ex:S4 sh:targetClass ex:Book ;\n    sh:property [ sh:path ex:author ; sh:class ex:Author ] .\n",
    "shacl-unknown-class",
    WARNING,
    "ex:Author"
  )
})

test("shacl-bad-datatype: a class used as sh:datatype", async () => {
  await expectOne(
    "ex:S5 sh:targetClass ex:Book ;\n    sh:property [ sh:path ex:title ; sh:datatype ex:Book ] .\n",
    "shacl-bad-datatype",
    WARNING,
    "ex:Book",
    1
  )
})

test("shacl-bad-datatype: rdfs:Literal is not a datatype", async () => {
  await expectOne(
    "ex:S6 sh:targetClass ex:Book ;\n    sh:property [ sh:path ex:title ; sh:datatype rdfs:Literal ] .\n",
    "shacl-bad-datatype",
    WARNING,
    "rdfs:Literal"
  )
})

test("shacl-misplaced: sh:minCount on a node shape", async () => {
  const d = await expectOne(
    "ex:S7 a sh:NodeShape ;\n    sh:targetClass ex:Book ;\n    sh:minCount 1 .\n",
    "shacl-misplaced",
    WARNING,
    "sh:minCount"
  )
  assert.match(d.message, /property shapes/)
})

test("shacl-misplaced: sh:path on a sh:NodeShape", async () => {
  await expectOne("ex:S8 a sh:NodeShape ;\n    sh:path ex:title .\n", "shacl-misplaced", WARNING, "sh:path")
})

test("shacl-dangling-shape: sh:node naming an undescribed shape", async () => {
  await expectOne(
    "ex:S9 sh:targetClass ex:Book ;\n    sh:node ex:MissingShape .\n",
    "shacl-dangling-shape",
    WARNING,
    "ex:MissingShape"
  )
})

test("shacl-dangling-shape: an sh:or member naming an undescribed shape", async () => {
  await expectOne(
    "ex:S10 sh:targetClass ex:Book ;\n    sh:or ( ex:NameShape ex:GoneShape [ sh:datatype xsd:string ] ) .\n",
    "shacl-dangling-shape",
    WARNING,
    "ex:GoneShape"
  )
})

test("shacl-count-conflict: sh:minCount above sh:maxCount", async () => {
  const body = "ex:S11 sh:targetClass ex:Book ;\n    sh:property [ sh:path ex:title ; sh:minCount 2 ; sh:maxCount 1 ] .\n"
  const d = await expectOne(body, "shacl-count-conflict", ERROR, "sh:maxCount 1", 0, "1")
  assert.match(d.message, /sh:maxCount 1 is less than sh:minCount 2/)
})

test("shacl-count-conflict: a negative count", async () => {
  await expectOne(
    "ex:S12 sh:targetClass ex:Book ;\n    sh:property [ sh:path ex:title ; sh:minCount -1 ] .\n",
    "shacl-count-conflict",
    ERROR,
    "-1"
  )
})

test("classes, datatypes and shapes of a namespace the workspace only references stay quiet", async () => {
  const body =
    "@prefix other: <http://example.org/elsewhere/> .\nex:S14 sh:targetClass other:Machine ;\n    sh:node other:MachineShape ;\n    sh:property [ sh:path ex:size ; sh:datatype other:Size ] .\n"
  assert.deepEqual(await shaclDiagnostics(body), [])
})

test("targets and sh:closed on a property shape are allowed", async () => {
  const body = "ex:P13 a sh:PropertyShape ;\n    sh:targetNode ex:book1 ;\n    sh:path ex:title ;\n    sh:closed false .\n"
  assert.deepEqual(await shaclDiagnostics(body), [])
})

test("the project shapes produce no shacl diagnostics", async () => {
  const project = path.join(fixtures, "project")
  const p = await startServer({ cwd: project })
  try {
    await p.request("workspace/symbol", { query: "" })
    const file = path.join(project, "shapes", "asset-shapes.ttl")
    const uri = uriOf(file)
    p.open(uri, fs.readFileSync(file, "utf8"))
    const diags = (await p.diagnosticsFor(uri)).filter((d) => String(d.code).startsWith("shacl-"))
    assert.deepEqual(diags, [])
  } finally {
    await p.stop()
  }
})
