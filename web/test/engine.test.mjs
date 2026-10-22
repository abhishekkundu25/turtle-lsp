// The browser engine: turtle-lsp built for WebAssembly, loaded the way the
// page's worker loads it, and asked what the toolbox asks it. Run
// `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { SETS, WORKSPACE } from "../app/samples.js"
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

// --- query ---------------------------------------------------------------------

// The sample workspace with `text` as q.rq, asked to run q.rq.
const ask = (text, limits = {}) =>
  engine.call("query", { files: [...WORKSPACE.map((f) => ({ name: f.path, text: f.text })), { name: "q.rq", text }], query: "q.rq", ...limits })

test("query: the sample's SELECT gives its rows as display strings, with the project's prefixes", () => {
  const files = WORKSPACE.map((f) => ({ name: f.path, text: f.text }))
  const r = engine.call("query", { files, query: "queries/people.rq" })
  assert.equal(r.kind, "select")
  assert.deepEqual(r.variables, ["name"])
  assert.equal(r.total, r.rows.length)
  assert.ok(r.rows.some((row) => row[0] === '"Alice"'), JSON.stringify(r.rows))
  assert.equal(typeof r.elapsedMs, "number")
  const p = ask("SELECT ?p ?nothing WHERE { ?p a ex:Person } ORDER BY ?p", { maxRows: 2 })
  assert.equal(p.rows.length, 2)
  assert.ok(p.total > 2)
  assert.match(p.rows[0][0], /^id:/)
  assert.equal(p.rows[0][1], null)
})

test("query: ASK gives a boolean, CONSTRUCT gives Turtle", () => {
  assert.equal(ask("ASK { id:alice a ex:Person }").boolean, true)
  assert.equal(ask("ASK { id:alice a ex:Organisation }").boolean, false)
  const c = ask("CONSTRUCT { ?p ex:employer ?o } WHERE { ?p ex:worksFor ?o }", { maxRows: 1 })
  assert.equal(c.kind, "construct")
  assert.equal(c.shown, 1)
  assert.ok(c.total > 1)
  assert.match(c.turtle, /@prefix ex: <https:\/\/toolbox\.example\.org\/def\/> \./)
  assert.match(c.turtle, /ex:employer/)
})

test("query: a syntax error has its line and column; a missing query file is a request error", () => {
  const e = ask("SELECT ?s\nWHERE { ?s a }")
  assert.equal(e.kind, "error")
  assert.deepEqual([e.line, e.column], [2, 14])
  assert.match(e.message, /syntax error/)
  assert.throws(() => engine.call("query", { files: [], query: "nope.rq" }), /no file is named 'nope.rq'/)
})

test("query: the time limit stops a long query with a message, and the engine keeps answering", () => {
  const e = ask("SELECT * WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l }", { timeout: 0.05 })
  assert.equal(e.kind, "error")
  assert.equal(e.line, null)
  assert.match(e.message, /time limit/)
  assert.equal(ask("ASK {}").boolean, true)
})

test("query: the time limit stops a join that combines rows in memory, reading no data", () => {
  let chain = "@prefix ex: <http://example.org/> .\n"
  for (let i = 0; i < 400; i++) chain += `ex:n${i} ex:next ex:n${i + 1} .\n`
  const text = "PREFIX ex: <http://example.org/>\nSELECT (COUNT(*) AS ?n) WHERE { ?a ex:next* ?b . ?c ex:next* ?d }\n"
  const t0 = performance.now()
  const e = engine.call("query", { files: [{ name: "chain.ttl", text: chain }, { name: "q.rq", text }], query: "q.rq", timeout: 0.05 })
  const ms = performance.now() - t0
  assert.equal(e.kind, "error")
  assert.match(e.message, /time limit/)
  assert.ok(ms < 1000, `the query ran ${ms.toFixed(0)} ms`)
})

// A prefix only the user's settings declare (Settings → Prefixes).
const ZZ = { "turtle.prefixes": { zz: "https://toolbox.example.org/def/" } }
const ZZ_QUERY = "SELECT ?p WHERE { ?p a zz:Person } ORDER BY ?p"

test("query: a prefix only the settings declare is known, as it is to the editor", () => {
  assert.match(ask(ZZ_QUERY).message, /Undeclared prefix 'zz:'/)
  const r = ask(ZZ_QUERY, { settings: ZZ })
  assert.equal(r.kind, "select")
  assert.ok(r.total > 0)
  // The same settings as the server reads them: nested, under turtle.
  assert.equal(ask(ZZ_QUERY, { settings: { turtle: { prefixes: ZZ["turtle.prefixes"] } } }).kind, "select")
})

test("query: the limits are the settings' unless the request has its own", () => {
  const all = "SELECT ?p WHERE { ?p a ex:Person } ORDER BY ?p"
  const total = ask(all).total
  assert.ok(total > 2)
  assert.equal(ask(all).rows.length, total)
  const set = ask(all, { settings: { "query.maxRows": 2 } })
  assert.deepEqual([set.rows.length, set.total], [2, total])
  assert.equal(ask(all, { settings: { "query.maxRows": 2 }, maxRows: 1 }).rows.length, 1)
  // 0 is no limit, here as in the settings.
  assert.equal(ask(all, { settings: { "query.maxRows": 2 }, maxRows: 0 }).rows.length, total)
  const slow = "SELECT * WHERE { ?a ?b ?c . ?d ?e ?f . ?g ?h ?i . ?j ?k ?l }"
  assert.match(ask(slow, { settings: { "query.timeoutSeconds": 0.05 } }).message, /time limit/)
})

test("query: a timeout too long for a duration is an error, not a crash of the engine", () => {
  for (const timeout of [1e300, 1.8446744073709552e19]) {
    assert.throws(
      () => ask("ASK {}", { timeout }),
      (e) => !e.crashed && /timeout is too long/.test(e.message),
    )
  }
  assert.throws(() => ask("ASK {}", { timeout: -1 }), (e) => !e.crashed && /timeout must be/.test(e.message))
  assert.throws(() => ask("ASK {}", { timeout: "soon" }), (e) => !e.crashed && /timeout must be/.test(e.message))
  // The same in the settings, which the server's parser would have panicked on.
  for (const settings of [{ "query.timeoutSeconds": 1e300 }, { turtle: { query: { timeoutSeconds: 1e300 } } }]) {
    assert.throws(() => ask("ASK {}", { settings }), (e) => !e.crashed && /timeoutSeconds 1e\+?300 is too large/.test(e.message))
  }
  assert.throws(() => ask("ASK {}", { settings: 3 }), (e) => !e.crashed && /settings must be an object/.test(e.message))
  assert.equal(ask("ASK {}").boolean, true)
})

test("query: a row limit past what the engine's memory can count is no limit, not zero rows", () => {
  const all = "SELECT ?p WHERE { ?p a ex:Person } ORDER BY ?p"
  const total = ask(all).total
  const r = ask(all, { maxRows: 4294967296 })
  assert.deepEqual([r.rows.length, r.total], [total, total])
  const c = ask("CONSTRUCT { ?p ex:employer ?o } WHERE { ?p ex:worksFor ?o }", { maxRows: 2 ** 40 })
  assert.equal(c.shown, c.total)
  for (const maxRows of [-1, 1.5, "ten"]) assert.throws(() => ask(all, { maxRows }), (e) => !e.crashed && /maxRows must be/.test(e.message), String(maxRows))
})

test("analyse and fix all use the settings: a prefix they declare needs no fix", () => {
  const files = [{ name: "a.ttl", text: "zz:a zz:b zz:c .\n" }]
  const plain = engine.call("analyse", { files }).files[0]
  assert.ok(withCode(plain, "undeclared-prefix").length > 0)
  const set = engine.call("analyse", { files, settings: ZZ }).files[0]
  assert.deepEqual(withCode(set, "undeclared-prefix"), [])
  // `foaf:` is a known vocabulary, so Fix All declares it, unless the user's
  // prefixes say what it is.
  const foaf = [{ name: "b.ttl", text: "foaf:a foaf:b foaf:c .\n" }]
  const declared = engine.call("fix_all", { files: foaf })
  assert.equal(declared.applied, 1)
  assert.match(declared.files[0].text, /^@prefix foaf: </)
  const own = engine.call("fix_all", { files: foaf, settings: { "turtle.prefixes": { foaf: "https://e.org/foaf#" } } })
  assert.equal(own.applied, 0)
  assert.equal(own.files[0].text, foaf[0].text)
  // The format verdict is at the user's line width.
  const long = [{ name: "c.ttl", text: "@prefix ex: <https://e.org/> .\n\nex:a ex:b ex:c, ex:d, ex:e, ex:f, ex:g, ex:h, ex:i, ex:j .\n" }]
  const wide = engine.call("analyse", { files: long, settings: { "turtle.format.lineWidth": 200 } }).files[0].format
  const narrow = engine.call("analyse", { files: long, settings: { "turtle.format.lineWidth": 20 } }).files[0].format
  assert.notEqual(wide.text, narrow.text)
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

test("files in folders are opened at their paths, so a folder's prefixes file declares its prefixes", () => {
  const files = [
    { name: "vocab/prefixes.ttl", text: "@prefix ex: <https://e.org/> .\n" },
    { name: "vocab/a.ttl", text: "ex:a rdfs:label ex:c .\n" },
  ]
  const a = fileOf(engine.call("analyse", { files }), "vocab/a.ttl")
  assert.deepEqual(
    withCode(a, "undeclared-prefix").map((d) => covered(files[1].text, d.range)),
    ["rdfs:label"],
  )
  // The fix for rdfs: comes back under the file's name; ex: needs none.
  const fixed = fileOf(engine.call("fix_all", { files }), "vocab/a.ttl").text
  assert.match(fixed, /^@prefix rdfs: /m)
  assert.doesNotMatch(fixed, /@prefix ex:/)
})

test("fix all applies only the fixes of the files not marked context, using what the context files say", () => {
  const onto = {
    name: "onto.ttl",
    text: "@prefix ex: <https://e.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\nex:Person a owl:Class .\nex:Organisation a owl:Class .\nex:name a owl:DatatypeProperty .\n",
    context: true,
  }
  const query = { name: "q.rq", text: "PREFIX ex: <https://e.org/>\nSELECT ?p WHERE { ?p a ex:Persn }\n" }
  const res = engine.call("fix_all", { files: [onto, query] })
  assert.equal(res.applied, 1)
  // Context files are not returned: their unused skos prefix stays theirs.
  assert.deepEqual(
    res.files.map((f) => f.name),
    ["q.rq"],
  )
  assert.match(fileOf(res, "q.rq").text, /\?p a ex:Person \}/)
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
