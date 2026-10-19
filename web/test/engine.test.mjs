// The browser engine: turtle-lsp built for WebAssembly, loaded the way the
// page's worker loads it, and asked what the toolbox asks it. Run
// `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { SETS } from "../app/samples.js"
import { startServer, tempDir, uriOf } from "../../test/lsp-client.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const root = path.resolve(here, "..", "..")
const wasm = fs.readFileSync(path.join(here, "..", "app", "engine.wasm"))

let engine
before(async () => {
  engine = await createEngine(wasm)
})

const starter = () => structuredClone(SETS.starter.files)
const fileOf = (res, name) => res.files.find((f) => f.name === name)
const withCode = (f, code) => f.diagnostics.filter((d) => d.code === code)
// The text a range covers, in the UTF-16 units the engine reports.
function covered(text, range) {
  const lines = text.split("\n")
  const at = (p) => lines.slice(0, p.line).reduce((n, l) => n + l.length + 1, 0) + p.character
  return text.slice(at(range.start), at(range.end))
}
function applyEdits(text, edits) {
  const lines = text.split("\n")
  const at = (p) => lines.slice(0, p.line).reduce((n, l) => n + l.length + 1, 0) + p.character
  const sorted = [...edits].sort((a, b) => at(b.range.start) - at(a.range.start))
  for (const e of sorted) text = text.slice(0, at(e.range.start)) + e.newText + text.slice(at(e.range.end))
  return text
}

// --- the module ---------------------------------------------------------------

test("the engine reports the turtle-lsp version it was built from", () => {
  const cargo = fs.readFileSync(path.join(root, "Cargo.toml"), "utf8")
  const version = cargo.match(/^version = "([^"]+)"/m)[1]
  assert.equal(engine.call("version").version, version)
})

test("an unknown operation is an error, and the engine keeps answering", () => {
  assert.throws(() => engine.call("teleport", {}), /unknown operation/)
  assert.ok(engine.call("analyse", { files: starter() }).files.length === 4)
})

test("each file's kind comes from its name", () => {
  const files = [
    { name: "a.ttl", text: "" },
    { name: "b.trig", text: "" },
    { name: "c.nt", text: "" },
    { name: "d.nq", text: "" },
    { name: "e.rq", text: "" },
    { name: "f.ru", text: "" },
    { name: "g.sparql", text: "" },
  ]
  const kinds = engine.call("analyse", { files }).files.map((f) => f.kind)
  assert.deepEqual(kinds, ["turtle", "trig", "ntriples", "nquads", "sparql", "sparql", "sparql"])
})

// --- validate -------------------------------------------------------------------

test("a misspelled vocabulary term is reported with its fix", () => {
  const files = starter()
  const onto = fileOf(engine.call("analyse", { files }), "ontology.ttl")
  const [d] = withCode(onto, "unknown-term")
  assert.ok(d, JSON.stringify(onto.diagnostics))
  assert.equal(covered(files[0].text, d.range), "rdfs:labl")
  const fix = d.fixes.find((f) => f.preferred)
  assert.equal(fix.title, "Change to 'rdfs:label'")
  assert.match(applyEdits(files[0].text, fix.edits["ontology.ttl"]), /ex:Person a owl:Class ;\n {4}rdfs:label "Person"/)
})

test("an unused prefix is reported with a fix that removes its line", () => {
  const files = starter()
  const onto = fileOf(engine.call("analyse", { files }), "ontology.ttl")
  const d = withCode(onto, "unused-prefix").find((x) => x.message.includes("skos"))
  assert.ok(d, JSON.stringify(onto.diagnostics))
  const after = applyEdits(files[0].text, d.fixes[0].edits["ontology.ttl"])
  assert.ok(!after.includes("skos"))
  assert.equal(after.split("\n").length, files[0].text.split("\n").length - 1)
})

test("an undeclared prefix is reported with a fix that declares it", () => {
  const files = starter()
  const data = fileOf(engine.call("analyse", { files }), "data.ttl")
  const d = withCode(data, "undeclared-prefix").find((x) => x.message.includes("rdfs"))
  assert.ok(d, JSON.stringify(data.diagnostics))
  const fix = d.fixes.find((f) => f.preferred)
  const after = applyEdits(files[2].text, fix.edits["data.ttl"])
  assert.match(after, /@prefix rdfs: <http:\/\/www\.w3\.org\/2000\/01\/rdf-schema#> \./)
})

test("a syntax error is reported where it is", () => {
  const text = "@prefix ex: <https://e.org/> .\nex:a ex:b ex:c ex:d .\n"
  const [f] = engine.call("analyse", { files: [{ name: "broken.ttl", text }] }).files
  const errors = f.diagnostics.filter((d) => d.severity === 1)
  assert.ok(errors.length >= 1, JSON.stringify(f.diagnostics))
  assert.equal(errors[0].range.start.line, 1)
})

test("ranges count UTF-16 units, as JavaScript strings do", () => {
  const text = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n<https://e.org/a> rdfs:comment "😀 é 😀" ; rdfs:labl "a" .\n`
  const [f] = engine.call("analyse", { files: [{ name: "wide.ttl", text }] }).files
  const [d] = withCode(f, "unknown-term")
  assert.ok(d, JSON.stringify(f.diagnostics))
  assert.equal(covered(text, d.range), "rdfs:labl")
})

// --- SHACL ------------------------------------------------------------------------

test("the data is validated against the shapes in another file", () => {
  const res = engine.call("analyse", { files: starter() })
  assert.equal(res.hasShapes, true)
  const data = fileOf(res, "data.ttl")
  const messages = withCode(data, "shacl-violation").map((d) => d.message)
  assert.ok(messages.some((m) => m.includes("Every person needs a name")), messages.join("\n"))
  assert.ok(messages.some((m) => m.includes("A person works for an organisation")), messages.join("\n"))
  // Alice conforms.
  const lines = withCode(data, "shacl-violation").map((d) => d.range.start.line)
  assert.ok(lines.every((l) => l >= 6), JSON.stringify(lines))
})

test("without shapes there is no SHACL validation", () => {
  const files = starter().filter((f) => f.name !== "shapes.ttl")
  const res = engine.call("analyse", { files })
  assert.equal(res.hasShapes, false)
  for (const f of res.files) assert.deepEqual(withCode(f, "shacl-violation"), [], f.name)
})

test("SHACL constraints written in SPARQL are evaluated, as the language server evaluates them", async () => {
  const files = [
    {
      name: "shapes.ttl",
      text: `@prefix ex: <https://e.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:sparql [
        a sh:SPARQLConstraint ;
        sh:message "A person's name must not be empty" ;
        sh:select """SELECT $this WHERE { $this <https://e.org/name> "" . FILTER (NOW() > "2000-01-01T00:00:00Z"^^<http://www.w3.org/2001/XMLSchema#dateTime>) }"""
    ] .
`,
    },
    { name: "data.ttl", text: `@prefix ex: <https://e.org/> .\n\nex:ann a ex:Person ; ex:name "" .\nex:bo a ex:Person ; ex:name "Bo" .\n` },
  ]
  const res = engine.call("analyse", { files })
  const found = withCode(fileOf(res, "data.ttl"), "shacl-violation")
  assert.equal(found.length, 1, JSON.stringify(found))
  assert.match(found[0].message, /A person's name must not be empty/)
  assert.equal(found[0].range.start.line, 2)
  const want = await serverDiagnostics(files)
  for (const f of res.files) assert.deepEqual(f.diagnostics.map(key).sort(), want[f.name].map(key).sort(), f.name)
})

test("while an import is not loaded, SHACL errors it could change are warnings, as in the language server", async () => {
  const files = [
    {
      name: "shapes.ttl",
      text: `@prefix ex: <https://e.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .

<https://e.org/shapes> a owl:Ontology ;
    owl:imports <https://e.org/core> , <https://unknown.example.org/vocab> .

ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:worksFor ; sh:class ex:Organisation ] .
`,
    },
    // Declares one of the two imports, which therefore counts as present.
    { name: "core.ttl", text: `@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\n<https://e.org/core> a owl:Ontology .\n` },
    { name: "data.ttl", text: `@prefix ex: <https://e.org/> .\n\nex:ann a ex:Person ; ex:worksFor ex:bo .\n` },
  ]
  const res = engine.call("analyse", { files })
  const [v] = withCode(fileOf(res, "data.ttl"), "shacl-violation")
  assert.ok(v, JSON.stringify(fileOf(res, "data.ttl").diagnostics))
  assert.equal(v.severity, 2)
  assert.match(v.message, /\(may change once these imports resolve: https:\/\/unknown\.example\.org\/vocab\)$/)
  const [note] = withCode(fileOf(res, "shapes.ttl"), "unresolved-import")
  assert.equal(note.message, "Import <https://unknown.example.org/vocab> is not loaded: imports are not followed in the browser. Add the imported ontology's file to the workspace to use it.")
  assert.equal(covered(files[0].text, note.range), "<https://unknown.example.org/vocab>")
  const want = await serverDiagnostics(files)
  for (const f of res.files) assert.deepEqual(f.diagnostics.map(key).sort(), want[f.name].map(key).sort(), f.name)
})

// --- format -----------------------------------------------------------------------

test("formatting keeps the graph, and formatted text stays as it is", () => {
  const res = engine.call("analyse", { files: starter() })
  const fmt = fileOf(res, "data.ttl").format
  assert.equal(fmt.status, "changed")
  assert.equal(fmt.guarantee, "same graph")
  assert.match(fmt.text, /^id:acme a ex:Organisation ;\n {4}rdfs:label "ACME" \.$/m)
  const again = engine.call("analyse", { files: [{ name: "data.ttl", text: fmt.text }] })
  assert.equal(fileOf(again, "data.ttl").format.status, "unchanged")
})

test("a file with syntax errors is not formatted, and says why", () => {
  const text = "@prefix ex: <https://e.org/> .\nex:a ex:b .\n"
  const [f] = engine.call("analyse", { files: [{ name: "broken.ttl", text }] }).files
  assert.equal(f.format.status, "refused")
  assert.match(f.format.reason, /syntax error/i)
  assert.equal(f.format.text, undefined)
})

test("N-Quads and TriG are formatted in their own syntax", () => {
  const res = engine.call("analyse", { files: structuredClone(SETS.datasets.files) })
  const nq = fileOf(res, "hr.nq").format
  assert.equal(nq.status, "changed")
  for (const line of nq.text.trim().split("\n")) assert.match(line, /^<[^>]+> <[^>]+> \S.* <[^>]+> \.$/)
  const trig = fileOf(res, "graphs.trig").format
  assert.equal(trig.status, "changed")
  assert.match(trig.text, /^g:2026 \{$/m)
})

// --- SPARQL -----------------------------------------------------------------------

test("a query's problems come with fixes; once fixed it is formatted with its algebra kept", () => {
  const files = starter()
  const q = fileOf(engine.call("analyse", { files }), "people.rq")
  assert.equal(q.kind, "sparql")
  assert.ok(withCode(q, "unbound-projection").some((d) => covered(files[3].text, d.range) === "?org"))
  // An undeclared prefix is an error, and a query with errors is not formatted.
  assert.equal(q.format.status, "refused")
  assert.equal(q.format.reason, "the query has 1 error; fix it first")
  const undeclared = withCode(q, "undeclared-prefix")[0]
  assert.ok(undeclared, JSON.stringify(q.diagnostics))
  const fix = undeclared.fixes.find((f) => f.preferred)
  files[3].text = applyEdits(files[3].text, fix.edits["people.rq"])
  assert.match(files[3].text, /^PREFIX foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/>$/m)
  const fixed = fileOf(engine.call("analyse", { files }), "people.rq")
  assert.equal(fixed.format.guarantee, "same SPARQL algebra")
  assert.equal(fixed.format.status, "changed")
  assert.match(fixed.format.text, /^SELECT \?name \?org$/m)
})

test("an update is analysed and formatted", () => {
  const res = engine.call("analyse", { files: structuredClone(SETS.queries.files) })
  const u = fileOf(res, "add-carol.ru")
  assert.deepEqual(u.diagnostics.filter((d) => d.severity === 1), [])
  assert.equal(u.format.status, "changed")
  assert.match(u.format.text, /^INSERT DATA \{$/m)
})

// --- fix all ----------------------------------------------------------------------

test("fix all applies every preferred fix across files and leaves the rest alone", () => {
  const files = starter()
  const { files: fixed, applied } = engine.call("fix_all", { files })
  assert.ok(applied >= 4, `applied ${applied}`)
  const byName = Object.fromEntries(fixed.map((f) => [f.name, f.text]))
  assert.equal(byName["shapes.ttl"], files[1].text)
  assert.match(byName["ontology.ttl"], /rdfs:label "Person"/)
  assert.ok(!byName["ontology.ttl"].includes("skos"))
  assert.match(byName["data.ttl"], /@prefix rdfs: /)
  assert.match(byName["people.rq"], /PREFIX foaf: /)
  const res = engine.call("analyse", { files: fixed })
  for (const f of res.files) {
    const left = f.diagnostics.filter((d) => d.fixes.some((x) => x.preferred))
    assert.deepEqual(left, [], f.name)
  }
})

test("fix all on input with nothing to fix changes nothing", () => {
  const files = [{ name: "ok.ttl", text: "@prefix ex: <https://e.org/> .\n\nex:a ex:b ex:c .\n" }]
  const res = engine.call("fix_all", { files })
  assert.equal(res.applied, 0)
  assert.deepEqual(res.files, files)
})

// --- robustness ---------------------------------------------------------------------

test("deeply nested input is refused, not a crash", () => {
  const n = 5000
  const ttl = `@prefix ex: <https://e.org/> .\nex:a ex:b ${"(".repeat(n)}${")".repeat(n)} .\n`
  const rq = `SELECT * WHERE ${"{ ".repeat(n)}${"} ".repeat(n)}\n`
  const res = engine.call("analyse", { files: [{ name: "deep.ttl", text: ttl }, { name: "deep.rq", text: rq }] })
  assert.ok(fileOf(res, "deep.rq").diagnostics.some((d) => d.severity === 1))
  assert.equal(engine.call("analyse", { files: starter() }).files.length, 4)
})

test("many calls do not keep growing the engine's memory", () => {
  for (let i = 0; i < 20; i++) engine.call("analyse", { files: starter() })
  const before = engine.memoryBytes()
  for (let i = 0; i < 200; i++) engine.call("analyse", { files: starter() })
  assert.ok(engine.memoryBytes() <= before * 1.5 + 4 * 1024 * 1024, `${before} -> ${engine.memoryBytes()}`)
})

// --- parity with the language server ---------------------------------------------------

// The language server's diagnostics for `files`, in a workspace of their
// own, opened once it is indexed and read when publishing has settled: no
// publication for two seconds, longer than the server waits after a change
// before validating SHACL (one second).
async function serverDiagnostics(files) {
  const dir = tempDir("turtle-web-parity-")
  for (const f of files) fs.writeFileSync(path.join(dir, f.name), f.text)
  const s = await startServer({ cwd: dir })
  try {
    await s.request("workspace/symbol", { query: "" })
    for (const f of files) s.open(uriOf(path.join(dir, f.name)), f.text, f.name.match(/\.(rq|ru|sparql)$/) ? "sparql" : "turtle")
    for (const f of files) await s.diagnosticsFor(uriOf(path.join(dir, f.name)), 10_000)
    const end = Date.now() + 15_000
    let n = -1
    while (n !== s.publications.length && Date.now() < end) {
      n = s.publications.length
      await new Promise((r) => setTimeout(r, 2000))
    }
    const out = {}
    for (const f of files) out[f.name] = await s.diagnosticsFor(uriOf(path.join(dir, f.name)))
    return out
  } finally {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

// The note on an import that is not loaded says what to do in each place
// (a quick fix and a catalog in an editor, adding the file in the toolbox):
// the same note, in the same place, with its own words.
const key = (d) =>
  `${d.range.start.line}:${d.range.start.character}-${d.range.end.line}:${d.range.end.character} ${d.severity} ${d.code} ${d.code === "unresolved-import" ? "" : d.message}`

for (const [name, set] of Object.entries(SETS)) {
  test(`the engine reports what the language server reports: ${name}`, async () => {
    const files = structuredClone(set.files)
    const want = await serverDiagnostics(files)
    const got = engine.call("analyse", { files })
    for (const f of got.files) {
      assert.deepEqual(f.diagnostics.map(key).sort(), want[f.name].map(key).sort(), f.name)
    }
  })
}
