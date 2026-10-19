// What each corpus file costs the real server: one server per file, in a
// directory holding only that file. Measures the time to the first
// diagnostics, formatting, semantic tokens and document symbols, the memory
// after them, CPU while idle, and that the server exits cleanly.
//
//     node tools/corpus/lsp-cost.mjs [DIR] [--only TEXT]
//
// Measures every Turtle, TriG, N-Triples and N-Quads file in DIR, and the
// longest SPARQL query of each SIB sparql-examples directory in it (the
// longest `sh:select` / `sh:ask` / `sh:construct` / `sh:describe` /
// `sh:update` / `spex:describe` literal, saved as `query.rq`). `--only TEXT`
// measures only the rows whose name contains TEXT and writes
// target/corpus/lsp-cost-only.md instead of target/corpus/lsp-cost.md.
// Files over 50 MB are skipped (the server does not index them either).
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { startServer, uriOf, tempDir } from "../../test/lsp-client.mjs"
import { cpuSeconds } from "../../test/procs.mjs"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..")
const args = process.argv.slice(2)
const onlyAt = args.findIndex((a) => a === "--only")
const only = onlyAt >= 0 ? args.splice(onlyAt, 2)[1] : undefined
const dir = args[0] ?? path.join(root, "test", "corpus")
const MAX = 50 << 20
const LANG = { ".ttl": "turtle", ".nt": "ntriples", ".trig": "trig", ".nq": "nquads", ".rq": "sparql" }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function rssMb(pid) {
  return Number(execFileSync("ps", ["-o", "rss=", "-p", String(pid)]).toString().trim()) / 1024
}

async function timed(f) {
  const t = performance.now()
  const r = await f()
  return [performance.now() - t, r]
}

// The longest SPARQL query literal in a sparql-examples directory, or
// undefined. Only triple-quoted literals: the longest queries are written so.
function longestQuery(sub) {
  let best
  for (const f of fs.readdirSync(sub, { recursive: true })) {
    if (!f.endsWith(".ttl")) continue
    const text = fs.readFileSync(path.join(sub, f), "utf8")
    const re = /(sh|spex):(select|ask|construct|describe|update)\s+"""([\s\S]*?)"""/g
    for (const m of text.matchAll(re)) {
      const query = m[3].replace(/\\(u[0-9A-Fa-f]{4}|U[0-9A-Fa-f]{8}|.)/g, (_, e) =>
        e.length > 1 ? String.fromCodePoint(parseInt(e.slice(1), 16)) : ({ n: "\n", t: "\t", r: "\r", b: "\b", f: "\f" }[e] ?? e),
      )
      if (!best || query.length > best.text.length) best = { text: query, from: `${f} ${m[1]}:${m[2]}` }
    }
  }
  return best
}

// Opens `text` as `file` (languageId from its extension) in a fresh
// workspace holding only that file, and measures the server.
async function measure(name, file, text) {
  const size = Buffer.byteLength(text)
  const row = { f: name, size }
  const ws = tempDir("turtle-corpus-")
  try {
    fs.writeFileSync(path.join(ws, file), text)
    const uri = uriOf(path.join(ws, file))
    const s = await startServer({ cwd: ws })
    try {
      await s.request("workspace/symbol", { query: "" }) // waits for the index
      const [diagMs, diags] = await timed(async () => {
        s.conn.sendNotification("textDocument/didOpen", {
          textDocument: { uri, languageId: LANG[path.extname(file)], version: 1, text },
        })
        return s.diagnosticsFor(uri, 120000)
      })
      row.diagMs = diagMs
      row.errors = diags.filter((d) => d.severity === 1).length
      row.warnings = diags.filter((d) => d.severity === 2).length
      ;[row.formatMs, row.formatted] = await timed(() =>
        s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } }),
      )
      row.formatted = row.formatted?.length ? "yes" : "no edits"
      ;[row.tokensMs] = await timed(() => s.request("textDocument/semanticTokens/full", { textDocument: { uri } }))
      ;[row.symbolsMs] = await timed(() => s.request("textDocument/documentSymbol", { textDocument: { uri } }))
      row.rssMb = rssMb(s.proc.pid)
      const cpu0 = cpuSeconds(s.proc.pid)
      await sleep(2000)
      row.idleCpu = ((cpuSeconds(s.proc.pid) - cpu0) / 2) * 100
    } catch (e) {
      row.note = `ERROR: ${e.message.slice(0, 200)}`
    }
    try {
      await s.stop() // strict: fails unless the server exits with 0
      row.exit = "clean"
    } catch (e) {
      row.exit = `NOT CLEAN: ${e.message.slice(0, 100)}`
    }
  } finally {
    fs.rmSync(ws, { recursive: true, force: true })
  }
  const fmt = (x) => (x === undefined ? "-" : x.toFixed(0))
  console.error(
    `${name.padEnd(28)} diag ${fmt(row.diagMs).padStart(6)} ms  format ${fmt(row.formatMs).padStart(6)} ms  ` +
      `rss ${fmt(row.rssMb).padStart(5)} MB  idle ${fmt(row.idleCpu)}%  ${row.exit ?? ""} ${row.note ?? ""}`,
  )
  return row
}

const rows = []
const entries = fs.readdirSync(dir).sort()
const wanted = (name) => only === undefined || name.includes(only)
for (const f of entries.filter((f) => LANG[path.extname(f)] && wanted(f))) {
  const src = path.join(dir, f)
  if (!fs.statSync(src).isFile()) continue
  const size = fs.statSync(src).size
  if (size > MAX) {
    rows.push({ f, size, note: "skipped (over 50 MB)" })
    continue
  }
  rows.push(await measure(f, f, fs.readFileSync(src, "utf8")))
}
for (const d of entries.filter((d) => fs.statSync(path.join(dir, d)).isDirectory())) {
  const q = longestQuery(path.join(dir, d))
  if (!q) continue
  const name = `${d}/${q.from} (longest query)`
  if (wanted(name)) rows.push(await measure(name, "query.rq", q.text))
}

const n = (x, d = 0) => (x === undefined ? "-" : x.toFixed(d))
let md = "# Server cost per corpus file\n\n"
md += `One server per file, on ${os.platform()} ${os.arch()}. Times in ms; memory is resident size after all requests.\n\n`
md += "| File | Size | Open → diagnostics | Errors / warnings | Format | Formatted | Semantic tokens | Symbols | Memory MB | Idle CPU % | Exit |\n"
md += "|---|---:|---:|---|---:|---|---:|---:|---:|---:|---|\n"
for (const r of rows) {
  const size = r.size >= 1 << 20 ? `${(r.size / (1 << 20)).toFixed(1)} MB` : `${Math.ceil(r.size / 1024)} KB`
  md += `| ${r.f} | ${size} | ${n(r.diagMs)} | ${r.errors ?? "-"} / ${r.warnings ?? "-"} | ${n(r.formatMs)} | ${r.formatted ?? "-"} | ${n(r.tokensMs)} | ${n(r.symbolsMs)} | ${n(r.rssMb)} | ${n(r.idleCpu, 1)} | ${r.exit ?? r.note ?? "-"} |\n`
}
fs.mkdirSync(path.join(root, "target", "corpus"), { recursive: true })
fs.writeFileSync(path.join(root, "target", "corpus", only === undefined ? "lsp-cost.md" : "lsp-cost-only.md"), md)
console.log(md)
if (rows.some((r) => r.note?.startsWith("ERROR") || r.exit?.startsWith("NOT"))) process.exit(1)
