// Performance benchmark: drives the built server over stdio like an editor.
// Usage: npm run build && node bench/perf.mjs [subjects=5000] [files=200]
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { execSync } from "node:child_process"
import { positionOf, startServer, uriOf } from "../test/lsp-client.mjs"
import { bigJsonLd } from "./jsonld-gen.mjs"

const SUBJECTS = Number(process.argv[2] ?? 5000)
const FILES = Number(process.argv[3] ?? 200)

const header = `@prefix ex: <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix schema: <https://schema.org/> .
`
function bigDoc(n, tag = "") {
  let out = header
  for (let i = 0; i < n; i++) {
    out += `
ex:p${tag}${i} a foaf:Person ;
    foaf:name "Person ${i}" ;
    rdfs:comment """A longer
multi-line description ${i}""" ;
    foaf:knows ex:p${tag}${(i + 1) % n} .
`
  }
  return out
}

const ws = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-bench-"))
for (let f = 0; f < FILES; f++) fs.writeFileSync(path.join(ws, `f${f}.ttl`), bigDoc(200, `f${f}_`))
const bigPath = path.join(ws, "big.ttl")
const big = bigDoc(SUBJECTS)
fs.writeFileSync(bigPath, big)

// Resident memory of the server (ps reports KiB).
const rss = (pid) => (Number(execSync(`ps -o rss= -p ${pid}`).toString().trim()) / 1024).toFixed(0)

const pct = (xs, p) => [...xs].sort((a, b) => a - b)[Math.min(xs.length - 1, Math.floor((p / 100) * xs.length))]
const rows = []
const record = (name, samples, extra = "") =>
  rows.push({ name, p50: pct(samples, 50).toFixed(1), p95: pct(samples, 95).toFixed(1), n: samples.length, extra })

const s = await startServer({ cwd: ws })
record(`initialize (${FILES} files x 200 subjects in workspace)`, [s.initMs])

const uri = uriOf(bigPath)
let t = performance.now()
s.open(uri, big)
await s.diagnosticsFor(uri, 120000)
record(`didOpen -> diagnostics (${big.split("\n").length} lines)`, [performance.now() - t])

// Simulated typing: one char appended to a line, full sync, wait for diagnostics.
const edits = []
let text = big
for (let i = 0; i < 15; i++) {
  text = text.replace(`"Person ${i}"`, `"Person ${i}x"`)
  t = performance.now()
  s.change(uri, text)
  await s.diagnosticsFor(uri, 120000)
  edits.push(performance.now() - t)
}
record("keystroke -> diagnostics", edits)
const rssIndexed = rss(s.proc.pid)

// Completion at end of file after "foaf:" and at an empty position.
const lines = text.split("\n")
const compl = async (suffix, line = `ex:new a ${suffix}`) => {
  const doc = text + `\n${line}`
  s.change(uri, doc)
  await s.diagnosticsFor(uri, 120000)
  const pos = { line: doc.split("\n").length - 1, character: line.length }
  const samples = []
  let bytes = 0
  let count = 0
  for (let i = 0; i < 10; i++) {
    const t0 = performance.now()
    const res = await s.request("textDocument/completion", { textDocument: { uri }, position: pos })
    samples.push(performance.now() - t0)
    const items = Array.isArray(res) ? res : res.items
    count = items.length
    bytes = JSON.stringify(res).length
  }
  return { samples, extra: `${count} items, ${(bytes / 1024).toFixed(0)} KB` }
}
let c = await compl("foaf:")
record("completion after 'foaf:'", c.samples, c.extra)
c = await compl("")
record("completion at empty position", c.samples, c.extra)
c = await compl("foaf:Person ; ")
record("completion of a predicate, ranked by type", c.samples, c.extra)
c = await compl("", "ex:new foaf:knows ")
record("completion of an object, ranked by range", c.samples, c.extra)

const defs = []
let defCount = 0
for (let i = 0; i < 10; i++) {
  const t0 = performance.now()
  const res = await s.request("textDocument/definition", { textDocument: { uri }, position: { line: 5, character: 3 } })
  defs.push(performance.now() - t0)
  defCount = res.length
}
record("definition of ex:p0", defs, `${defCount} locations`)

const t0 = performance.now()
const syms = await s.request("textDocument/documentSymbol", { textDocument: { uri } })
record("documentSymbol", [performance.now() - t0], `${syms.length} symbols`)

const semTimes = []
let semCount = 0
for (let i = 0; i < 10; i++) {
  const t1 = performance.now()
  const res = await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
  semTimes.push(performance.now() - t1)
  semCount = res.data.length / 5
}
record(`semanticTokens/full (${text.split("\n").length} lines)`, semTimes, `${semCount} tokens`)

// Labels for every term (the bench data has no opaque names), so there is work.
s.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: "all" } } } })
await s.request("textDocument/inlayHint", { textDocument: { uri }, range: { start: { line: 0, character: 0 }, end: { line: 1, character: 0 } } })
const hintTimes = []
let hintCount = 0
for (let i = 0; i < 10; i++) {
  const line = 1000 + i * 500
  const range = { start: { line, character: 0 }, end: { line: line + 100, character: 0 } }
  const t1 = performance.now()
  const res = await s.request("textDocument/inlayHint", { textDocument: { uri }, range })
  hintTimes.push(performance.now() - t1)
  hintCount = res.length
}
record("inlayHint (100-line range, labels: all)", hintTimes, `${hintCount} hints`)

// Formatting the whole big file (squashed onto fewer lines so there is work to do).
const messy = text.replaceAll(" ;\n    ", " ; ")
s.change(uri, messy)
await s.diagnosticsFor(uri, 120000)
const fmts = []
let fmtEdits = 0
for (let i = 0; i < 10; i++) {
  const t1 = performance.now()
  const res = await s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } })
  fmts.push(performance.now() - t1)
  fmtEdits = res.length
}
record(`format (${text.split("\n").length} lines)`, fmts, `${fmtEdits} edit(s)`)

// Run query: a join over the whole workspace, answered off the main loop.
const queryUri = uriOf(path.join(ws, "people.rq"))
s.open(
  queryUri,
  "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT (COUNT(*) AS ?n) WHERE { ?p a foaf:Person ; foaf:knows ?q . ?q foaf:name ?name }\n",
  "sparql",
)
const queries = []
let querySummary = ""
for (let i = 0; i < 5; i++) {
  const t1 = performance.now()
  querySummary = await s.request("workspace/executeCommand", { command: "turtle.runQuery", arguments: [queryUri] })
  queries.push(performance.now() - t1)
}
record("run query (join over the workspace)", queries, querySummary)

const rssEnd = rss(s.proc.pid)
await s.stop()

// Shapes present: a simple shape over every foaf:Person in the workspace.
// Saving validates in the background; typing must stay as fast as above.
fs.writeFileSync(
  path.join(ws, "shapes.ttl"),
  `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass foaf:Person ;
    sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:pattern "^Person [0-9]+$" ] ;
    sh:property [ sh:path foaf:knows ; sh:class foaf:Person ] .
`
)
const s2 = await startServer({ cwd: ws })
await s2.request("workspace/symbol", { query: "" })
s2.open(uri, big)
await s2.diagnosticsFor(uri, 120000)
const isShacl = (d) => d.code === "shacl-violation"
const saves = []
const typing = []
text = big
let broken = 0
// One more name that breaks the pattern per save, so each run changes the results.
const saveAndBreak = async (keystrokes) => {
  text = text.replace(`"Person ${broken}"`, `"Person ${broken}x"`)
  broken++
  s2.change(uri, text)
  await s2.diagnosticsFor(uri, 120000)
  const t1 = performance.now()
  s2.notify("textDocument/didSave", { textDocument: { uri } })
  // Keystrokes while the validation runs.
  for (let k = 0; k < keystrokes; k++) {
    const tk = performance.now()
    s2.change(uri, `${text}\n# ${broken} ${k}\n`)
    await s2.diagnosticsFor(uri, 120000)
    typing.push(performance.now() - tk)
  }
  for (;;) {
    const d = await s2.diagnosticsFor(uri, 120000)
    if (d.filter(isShacl).length === broken) break
    await new Promise((r) => setTimeout(r, 2))
  }
  return performance.now() - t1
}
for (let i = 0; i < 5; i++) saves.push(await saveAndBreak(0))
for (let i = 0; i < 5; i++) await saveAndBreak(4)
const triples = (FILES * 200 + SUBJECTS) * 4 + 6
record(`didSave -> SHACL diagnostics (~${(triples / 1000).toFixed(0)}k triples, 1 shape)`, saves)
record("keystroke -> diagnostics while validating", typing)
await s2.stop()
fs.rmSync(ws, { recursive: true, force: true })

// JSON-LD next to Turtle of the same length (lines), in an empty workspace,
// so each format is measured on its own. Documents are opened unsaved; each
// open uses new IRIs and is closed after, so no open replaces another.
// The JSON-LD file has an inline @context only: no context is fetched, and
// after the first open, re-checking reuses what the server cached.
const jws = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-bench-jsonld-"))
const s3 = await startServer({ cwd: jws })
const formats = {
  turtle: { ext: "ttl", lang: "turtle", make: (tag) => bigDoc(SUBJECTS, tag) },
  jsonld: { ext: "jsonld", lang: "jsonld", make: (tag) => bigJsonLd(30000, tag) },
}
const opens = { turtle: [], jsonld: [] }
for (let round = 0; round < 5; round++) {
  for (const [f, spec] of Object.entries(formats)) {
    const doc = spec.make(`o${round}_`)
    const u = uriOf(path.join(jws, `open${round}.${spec.ext}`))
    const t1 = performance.now()
    s3.open(u, doc, spec.lang)
    await s3.diagnosticsFor(u, 120000)
    opens[f].push(performance.now() - t1)
    s3.close(u)
  }
}
for (const [f, spec] of Object.entries(formats)) {
  let doc = spec.make("")
  record(`${f}: didOpen -> diagnostics (${doc.split("\n").length} lines, ${(doc.length / 1024).toFixed(0)} KB)`, opens[f])
  const u = uriOf(path.join(jws, `big.${spec.ext}`))
  s3.open(u, doc, spec.lang)
  await s3.diagnosticsFor(u, 120000)
  const keys = []
  for (let i = 0; i < 15; i++) {
    doc = doc.replace(`"Person ${i}"`, `"Person ${i}x"`)
    const t1 = performance.now()
    s3.change(u, doc)
    await s3.diagnosticsFor(u, 120000)
    keys.push(performance.now() - t1)
  }
  record(`${f}: keystroke -> diagnostics`, keys)
  // Hover on the name property of a resource in the middle of the file.
  const pos = positionOf(doc, f === "jsonld" ? `"name": "Person 700"` : `foaf:name "Person 2500"`, 2)
  const hovers = []
  let found = "no hover"
  for (let i = 0; i < 10; i++) {
    const t1 = performance.now()
    const h = await s3.request("textDocument/hover", { textDocument: { uri: u }, position: pos })
    hovers.push(performance.now() - t1)
    if (h) found = "hover"
  }
  record(`${f}: hover on the name property`, hovers, found)
  if (f === "jsonld") {
    // Completion in a key in the middle of the file, and formatting it all.
    const at = positionOf(doc, `"name": "Person 700"`, 3)
    const completions = []
    let items = 0
    for (let i = 0; i < 10; i++) {
      const t1 = performance.now()
      const res = await s3.request("textDocument/completion", { textDocument: { uri: u }, position: at })
      completions.push(performance.now() - t1)
      items = (Array.isArray(res) ? res : res?.items ?? []).length
    }
    record(`${f}: completion in a key`, completions, `${items} items`)
    const formats = []
    for (let i = 0; i < 5; i++) {
      const t1 = performance.now()
      await s3.request("textDocument/formatting", { textDocument: { uri: u }, options: { tabSize: 2, insertSpaces: true } })
      formats.push(performance.now() - t1)
    }
    record(`${f}: format the whole file`, formats)
  }
  s3.close(u)
}
await s3.stop()
fs.rmSync(jws, { recursive: true, force: true })
const openRatio = pct(opens.jsonld, 50) / pct(opens.turtle, 50)

console.log(`\nturtle-lsp perf (${SUBJECTS} subjects in big.ttl, ${FILES} workspace files)\n`)
const table = rows.map((r) => ({ metric: r.name, "p50 ms": r.p50, "p95 ms": r.p95, n: r.n, notes: r.extra }))
table.push({ metric: "server RSS after indexing + open + edits", "p50 ms": "", "p95 ms": "", n: 1, notes: `${rssIndexed} MB` })
table.push({ metric: "server RSS at the end", "p50 ms": "", "p95 ms": "", n: 1, notes: `${rssEnd} MB` })
console.table(table)
// The JSON-LD budgets (docs/specs/jsonld.md); tests/jsonld_budget.rs asserts
// them on the engine in release builds.
const jsonldKey = rows.find((r) => r.name === "jsonld: keystroke -> diagnostics")
console.log(
  `JSON-LD budgets: open ${openRatio.toFixed(2)}x Turtle (budget 3x); keystroke p50 ${jsonldKey.p50} ms (budget 10 ms)`
)
