// Performance benchmark: drives the built server over stdio like an editor.
// Usage: npm run build && node bench/perf.mjs [subjects=5000] [files=200]
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { execSync } from "node:child_process"
import { startServer, uriOf } from "../test/lsp-client.mjs"

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
const compl = async (suffix) => {
  const doc = text + `\nex:new a ${suffix}`
  s.change(uri, doc)
  await s.diagnosticsFor(uri, 120000)
  const pos = { line: doc.split("\n").length - 1, character: `ex:new a ${suffix}`.length }
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

const rssEnd = rss(s.proc.pid)
await s.stop()
fs.rmSync(ws, { recursive: true, force: true })

console.log(`\nturtle-lsp perf (${SUBJECTS} subjects in big.ttl, ${FILES} workspace files)\n`)
const table = rows.map((r) => ({ metric: r.name, "p50 ms": r.p50, "p95 ms": r.p95, n: r.n, notes: r.extra }))
table.push({ metric: "server RSS after indexing + open + edits", "p50 ms": "", "p95 ms": "", n: 1, notes: `${rssIndexed} MB` })
table.push({ metric: "server RSS at the end", "p50 ms": "", "p95 ms": "", n: 1, notes: `${rssEnd} MB` })
console.table(table)
