// What each corpus file costs the real server: one server per file, in a
// directory holding only that file. Measures the time to the first
// diagnostics, formatting, semantic tokens and document symbols, the memory
// after them, CPU while idle, and that the server exits cleanly.
//
//     node tools/corpus/lsp-cost.mjs [DIR]
//
// Writes target/corpus/lsp-cost.md. Files over 50 MB are skipped (the server
// does not index them either).
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { startServer, uriOf, tempDir } from "../../test/lsp-client.mjs"
import { cpuSeconds } from "../../test/procs.mjs"

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..")
const dir = process.argv[2] ?? path.join(root, "test", "corpus")
const MAX = 50 << 20
const LANG = { ".ttl": "turtle", ".nt": "ntriples", ".trig": "trig" }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function rssMb(pid) {
  return Number(execFileSync("ps", ["-o", "rss=", "-p", String(pid)]).toString().trim()) / 1024
}

async function timed(f) {
  const t = performance.now()
  const r = await f()
  return [performance.now() - t, r]
}

const rows = []
const files = fs
  .readdirSync(dir)
  .filter((f) => LANG[path.extname(f)])
  .sort()
for (const f of files) {
  const src = path.join(dir, f)
  const size = fs.statSync(src).size
  if (size > MAX) {
    rows.push({ f, size, note: "skipped (over 50 MB)" })
    continue
  }
  const ws = tempDir("turtle-corpus-")
  fs.copyFileSync(src, path.join(ws, f))
  const uri = uriOf(path.join(ws, f))
  const text = fs.readFileSync(src, "utf8")
  const row = { f, size }
  const s = await startServer({ cwd: ws })
  try {
    await s.request("workspace/symbol", { query: "" }) // waits for the index
    const [diagMs, diags] = await timed(async () => {
      s.conn.sendNotification("textDocument/didOpen", {
        textDocument: { uri, languageId: LANG[path.extname(f)], version: 1, text },
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
  fs.rmSync(ws, { recursive: true, force: true })
  const fmt = (x) => (x === undefined ? "-" : x.toFixed(0))
  console.error(
    `${f.padEnd(28)} diag ${fmt(row.diagMs).padStart(6)} ms  format ${fmt(row.formatMs).padStart(6)} ms  ` +
      `rss ${fmt(row.rssMb).padStart(5)} MB  idle ${fmt(row.idleCpu)}%  ${row.exit ?? ""} ${row.note ?? ""}`,
  )
  rows.push(row)
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
fs.writeFileSync(path.join(root, "target", "corpus", "lsp-cost.md"), md)
console.log(md)
if (rows.some((r) => r.note?.startsWith("ERROR") || r.exit?.startsWith("NOT"))) process.exit(1)
