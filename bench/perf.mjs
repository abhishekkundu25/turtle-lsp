// Performance benchmark: drives the built server over stdio like an editor, and
// the `check` command like a CI job.
//
//   npm run build && node bench/perf.mjs [subjects=5000] [files=200] [options]
//
//   --json PATH     also write every row to PATH (see "Results file" below)
//   --scale N       multiply the stress scenarios' sizes by N (env BENCH_SCALE).
//                   Only for a quick local look: the sample counts stay the same
//                   but numbers at another scale are not comparable.
//   --only A,B      run only these scenarios (env BENCH_ONLY)
//   --list          print the scenario names and stop
//
//   node bench/perf.mjs --compare BASE.json CURRENT.json [--max-ratio 1.5] [--report-only]
//
// compares two results files: it prints every row present in both and exits 1
// when a gated row (a keystroke row's p50) is more than --max-ratio times the
// base (default 1.5), unless --report-only. Files taken at different scales or
// on different platforms are not compared as a gate.
//
// Statistics. Every row has n samples. p95 is reported only when n >= 20 (the
// nearest-rank p95); with fewer samples the table shows the maximum instead and
// the JSON has p95 = null, because the p95 of fewer than 20 samples is the
// maximum. Rows with n = 1 are one-off events (a cold start, a first open).
//
// Results file (--json). { schema: 1, meta: {...}, rows: [...], problems: [...] }
// where a row is { id, name, n, p50, p95, max, rssMb, gate, notes } (times in
// ms; null where the row has no such figure) and a problem is a scenario that
// threw or an expectation that failed (a stress scenario that did not exercise
// what its name says, such as a document that produced no diagnostics). Any
// problem makes the run exit 1; the file is still written.
//
// Timing is noisy on shared machines (meta.load1 is the 1-minute load average
// at the start). Do not set thresholds from one run.
//
// Memory is the server's resident set size, read from /proc on Linux, from `ps`
// on macOS and other Unix systems, and from `tasklist` on Windows. Where none of
// those works the memory figures are null.
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { execFileSync, spawnSync } from "node:child_process"
import { fileURLToPath } from "node:url"

const here = path.dirname(fileURLToPath(import.meta.url))

// ---- arguments

function parseArgs(argv) {
  const a = { positional: [], scale: Number(process.env.BENCH_SCALE ?? 1), only: process.env.BENCH_ONLY, maxRatio: 1.5 }
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i]
    const value = () => {
      const v = argv[++i]
      if (v === undefined) throw new Error(`${arg} needs a value`)
      return v
    }
    if (arg === "--json") a.json = value()
    else if (arg === "--scale") a.scale = Number(value())
    else if (arg === "--only") a.only = value()
    else if (arg === "--list") a.list = true
    else if (arg === "--compare") a.compare = [value(), value()]
    else if (arg === "--max-ratio") a.maxRatio = Number(value())
    else if (arg === "--report-only") a.reportOnly = true
    else if (arg.startsWith("--")) throw new Error(`unknown option ${arg}`)
    else a.positional.push(arg)
  }
  if (!(a.scale > 0)) throw new Error("--scale must be a positive number")
  if (!(a.maxRatio > 0)) throw new Error("--max-ratio must be a positive number")
  return a
}

// ---- statistics

const sorted = (xs) => [...xs].sort((a, b) => a - b)
// Nearest rank: the smallest sample with at least p percent of the samples at or below it.
const rank = (xs, p) => sorted(xs)[Math.max(0, Math.ceil((p / 100) * xs.length) - 1)]
const round1 = (x) => (x == null ? null : Math.round(x * 10) / 10)
const MIN_FOR_P95 = 20

// ---- results file comparison

function compare(basePath, currentPath, { maxRatio, reportOnly }) {
  const base = JSON.parse(fs.readFileSync(basePath, "utf8"))
  const cur = JSON.parse(fs.readFileSync(currentPath, "utf8"))
  const lines = []
  const out = (s) => {
    console.log(s)
    lines.push(s)
  }
  const why = []
  if (base.schema !== cur.schema) why.push(`schema ${base.schema} vs ${cur.schema}`)
  if (base.meta?.scale !== cur.meta?.scale) why.push(`scale ${base.meta?.scale} vs ${cur.meta?.scale}`)
  if (base.meta?.platform !== cur.meta?.platform) why.push(`platform ${base.meta?.platform} vs ${cur.meta?.platform}`)
  if (base.meta?.params?.subjects !== cur.meta?.params?.subjects || base.meta?.params?.files !== cur.meta?.params?.files)
    why.push("different subjects/files arguments")
  const baseRows = new Map(base.rows.map((r) => [r.id, r]))
  out(`### Benchmark against ${base.meta?.commit?.slice(0, 7) ?? "the base"} (${base.meta?.date ?? "unknown date"})`)
  out("")
  out(`Gate: the p50 of a keystroke row over ${maxRatio}x the base, where the base is at least 1 ms. Shared runners are noisy.`)
  out("")
  out("| row | base p50 ms | now p50 ms | ratio | |")
  out("|---|---:|---:|---:|---|")
  let failed = 0
  for (const r of cur.rows) {
    const b = baseRows.get(r.id)
    if (!b || b.p50 == null || r.p50 == null) continue
    const ratio = b.p50 > 0 ? r.p50 / b.p50 : NaN
    const over = r.gate && b.p50 >= 1 && ratio > maxRatio
    if (over) failed++
    out(`| ${r.name} | ${b.p50} | ${r.p50} | ${Number.isFinite(ratio) ? ratio.toFixed(2) : "-"} | ${over ? "OVER" : r.gate ? "gated" : ""} |`)
  }
  for (const [id, b] of baseRows) if (!cur.rows.some((r) => r.id === id)) out(`| ${b.name} | ${b.p50} | (missing) | | |`)
  out("")
  if (why.length) {
    out(`Not compared as a gate: ${why.join("; ")}.`)
    failed = 0
  }
  out(failed ? `${failed} gated row(s) over ${maxRatio}x${reportOnly ? " (report only)" : ""}.` : "No gated row over the ratio.")
  if (process.env.GITHUB_STEP_SUMMARY) fs.appendFileSync(process.env.GITHUB_STEP_SUMMARY, lines.join("\n") + "\n")
  return reportOnly ? 0 : failed ? 1 : 0
}

const args = parseArgs(process.argv.slice(2))
if (args.compare) process.exit(compare(args.compare[0], args.compare[1], args))

// ---- the benchmark

const { positionOf, serverPath, serverEnv, startServer, uriOf } = await import("../test/lsp-client.mjs")
const { bigJsonLd } = await import("./jsonld-gen.mjs")

const SUBJECTS = Number(args.positional[0] ?? 5000)
const FILES = Number(args.positional[1] ?? 200)
const SCALE = args.scale
const scaled = (n) => Math.max(1, Math.round(n * SCALE))

const rows = []
const problems = []
const problem = (msg) => {
  problems.push(msg)
  console.error(`PROBLEM: ${msg}`)
}
const expect = (cond, msg) => {
  if (!cond) problem(msg)
}

// record(id, name, samples, { notes, rssMb, gate }): one row of timings in ms.
function record(id, name, samples, { notes = "", rssMb = null, gate = false } = {}) {
  const n = samples.length
  rows.push({
    id,
    name,
    n,
    p50: round1(rank(samples, 50)),
    p95: n >= MIN_FOR_P95 ? round1(rank(samples, 95)) : null,
    max: round1(Math.max(...samples)),
    rssMb,
    gate,
    notes,
  })
}
// A row that is only a memory figure.
const recordRss = (id, name, rssMb, notes = "") =>
  rows.push({ id, name, n: 1, p50: null, p95: null, max: null, rssMb, gate: false, notes })

// Resident memory of a process in MB, or null where this system cannot say.
function rssMb(pid) {
  try {
    let kib
    if (process.platform === "linux") {
      kib = Number(/^VmRSS:\s+(\d+)/m.exec(fs.readFileSync(`/proc/${pid}/status`, "utf8"))?.[1])
    } else if (process.platform === "win32") {
      // "Image Name","PID","Session Name","Session#","Mem Usage" with "12,345 K".
      const csv = execFileSync("tasklist", ["/FI", `PID eq ${pid}`, "/FO", "CSV", "/NH"], { encoding: "utf8" })
      kib = Number(/"([\d,.\s]+) K"\s*$/m.exec(csv)?.[1].replace(/\D/g, ""))
    } else {
      kib = Number(execFileSync("ps", ["-o", "rss=", "-p", String(pid)], { encoding: "utf8" }).trim())
    }
    return Number.isFinite(kib) && kib > 0 ? Math.round(kib / 1024) : null
  } catch {
    return null
  }
}

async function withDir(prefix, fn) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), prefix))
  try {
    return await fn(dir)
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

// A started server whose initial index has been merged (workspace/symbol waits
// for it), stopped at the end however `fn` ends. `s.indexMs` is that wait.
async function withServer(cwd, fn) {
  const s = await startServer({ cwd })
  try {
    const t = performance.now()
    await s.request("workspace/symbol", { query: "" })
    s.indexMs = performance.now() - t
    return await fn(s)
  } finally {
    await s.stop().catch((e) => problem(`stopping the server: ${e.message}`))
  }
}

// n timings of `step(i)`, in ms.
async function time(n, step) {
  const samples = []
  for (let i = 0; i < n; i++) {
    const t = performance.now()
    await step(i)
    samples.push(performance.now() - t)
  }
  return samples
}

const WAIT = 120000
const header = `@prefix ex: <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix schema: <https://schema.org/> .
`
// Four triples per subject.
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

// A deterministic stream of numbers, so the data is the same on every run.
function prng(seed) {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = a
    t = Math.imul(t ^ (t >>> 15), t | 1)
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61)
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

const personShape = (extra = "") => `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
ex:PersonShape a sh:NodeShape ;
    sh:targetClass foaf:Person ;
    sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:pattern "^Person [0-9]+$" ]${extra} .
`

// ---- scenarios

const SCENARIOS = []
const scenario = (name, about, run) => SCENARIOS.push({ name, about, run })

// The workspace most scenarios share: FILES files of 200 subjects and a big file.
let shared
const sharedWorkspace = () =>
  (shared ??= (() => {
    const ws = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-bench-"))
    for (let f = 0; f < FILES; f++) fs.writeFileSync(path.join(ws, `f${f}.ttl`), bigDoc(200, `f${f}_`))
    const bigPath = path.join(ws, "big.ttl")
    const big = bigDoc(SUBJECTS)
    fs.writeFileSync(bigPath, big)
    return { ws, bigPath, big }
  })())
// One simple shape over every foaf:Person of the shared workspace.
function addShape(ws) {
  fs.writeFileSync(path.join(ws, "shapes.ttl"), personShape(" ;\n    sh:property [ sh:path foaf:knows ; sh:class foaf:Person ]"))
}

scenario("editing", "typing, completion, navigation, formatting and queries in a workspace of many files", async () => {
  const { ws, bigPath, big } = sharedWorkspace()
  const s = await startServer({ cwd: ws })
  try {
    record("initialize", `initialize (${FILES} files x 200 subjects in workspace)`, [s.initMs])
    const t0 = performance.now()
    await s.request("workspace/symbol", { query: "" })
    record("index-ready", "initial index merged (workspace/symbol answered)", [performance.now() - t0])

    const uri = uriOf(bigPath)
    s.open(uri, big)
    const t = performance.now()
    await s.diagnosticsFor(uri, WAIT)
    record("open", `didOpen -> diagnostics (${big.split("\n").length} lines)`, [performance.now() - t])

    // Simulated typing: one char appended to a line, full sync, wait for diagnostics.
    let text = big
    const edits = await time(25, async (i) => {
      text = text.replace(`"Person ${i}"`, `"Person ${i}x"`)
      s.change(uri, text)
      await s.diagnosticsFor(uri, WAIT)
    })
    record("keystroke", "keystroke -> diagnostics", edits, { gate: true })
    recordRss("rss-indexed", "server RSS after indexing + open + edits", rssMb(s.proc.pid))

    // Completion at the end of the file, after text of different shapes.
    const compl = async (id, name, suffix, line = `ex:new a ${suffix}`) => {
      const doc = text + `\n${line}`
      s.change(uri, doc)
      await s.diagnosticsFor(uri, WAIT)
      const pos = { line: doc.split("\n").length - 1, character: line.length }
      let bytes = 0
      let count = 0
      const samples = await time(20, async () => {
        const res = await s.request("textDocument/completion", { textDocument: { uri }, position: pos })
        count = (Array.isArray(res) ? res : res.items).length
        bytes = JSON.stringify(res).length
      })
      record(id, name, samples, { notes: `${count} items, ${(bytes / 1024).toFixed(0)} KB` })
    }
    await compl("completion-prefixed", "completion after 'foaf:'", "foaf:")
    await compl("completion-empty", "completion at empty position", "")
    await compl("completion-predicate", "completion of a predicate, ranked by type", "foaf:Person ; ")
    await compl("completion-object", "completion of an object, ranked by range", "", "ex:new foaf:knows ")

    let defCount = 0
    const defs = await time(20, async () => {
      defCount = (await s.request("textDocument/definition", { textDocument: { uri }, position: { line: 5, character: 3 } })).length
    })
    record("definition", "definition of ex:p0", defs, { notes: `${defCount} locations` })

    let symCount = 0
    const syms = await time(20, async () => {
      symCount = (await s.request("textDocument/documentSymbol", { textDocument: { uri } })).length
    })
    record("document-symbol", "documentSymbol", syms, { notes: `${symCount} symbols` })

    let semCount = 0
    const sems = await time(20, async () => {
      semCount = (await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })).data.length / 5
    })
    record("semantic-tokens", `semanticTokens/full (${text.split("\n").length} lines)`, sems, { notes: `${semCount} tokens` })

    // Labels for every term (the bench data has no opaque names), so there is work.
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: "all" } } } })
    await s.request("textDocument/inlayHint", { textDocument: { uri }, range: { start: { line: 0, character: 0 }, end: { line: 1, character: 0 } } })
    let hintCount = 0
    const hints = await time(20, async (i) => {
      const line = 1000 + i * 500
      const range = { start: { line, character: 0 }, end: { line: line + 100, character: 0 } }
      hintCount = (await s.request("textDocument/inlayHint", { textDocument: { uri }, range })).length
    })
    record("inlay-hint", "inlayHint (100-line range, labels: all)", hints, { notes: `${hintCount} hints` })

    // Formatting the whole big file (squashed onto fewer lines so there is work to do).
    s.change(uri, text.replaceAll(" ;\n    ", " ; "))
    await s.diagnosticsFor(uri, WAIT)
    let fmtEdits = 0
    const fmts = await time(20, async () => {
      fmtEdits = (await s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } })).length
    })
    record("format", `format (${text.split("\n").length} lines)`, fmts, { notes: `${fmtEdits} edit(s)` })

    // Run query: a join over the whole workspace, answered off the main loop.
    const queryUri = uriOf(path.join(ws, "people.rq"))
    s.open(
      queryUri,
      "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT (COUNT(*) AS ?n) WHERE { ?p a foaf:Person ; foaf:knows ?q . ?q foaf:name ?name }\n",
      "sparql",
    )
    let summary = ""
    const queries = await time(20, async () => {
      summary = await s.request("workspace/executeCommand", { command: "turtle.runQuery", arguments: [queryUri] })
    })
    record("run-query", "run query (join over the workspace)", queries, { notes: String(summary) })
    recordRss("rss-end", "server RSS at the end", rssMb(s.proc.pid))
  } finally {
    await s.stop().catch((e) => problem(`stopping the server: ${e.message}`))
  }
})

// Saving validates in the background; typing must stay as fast as without a shape.
// `uri` is open in `s` with `doc`; `edit(text, k)` returns `text` with one more name
// that breaks the shape, so each save changes the results.
async function shaclRounds(s, uri, doc, edit, { id, label, saves = 20, typingRounds = 5, keystrokes = 4 }) {
  const isShacl = (d) => d.code === "shacl-violation"
  const saved = []
  const typing = []
  let text = doc
  let broken = 0
  let violations = 0
  const saveAndBreak = async (typed) => {
    text = edit(text, broken)
    broken++
    s.change(uri, text)
    await s.diagnosticsFor(uri, WAIT)
    const t = performance.now()
    s.notify("textDocument/didSave", { textDocument: { uri } })
    // Keystrokes while the validation runs.
    for (let k = 0; k < typed; k++) {
      const tk = performance.now()
      s.change(uri, `${text}\n# ${broken} ${k}\n`)
      await s.diagnosticsFor(uri, WAIT)
      typing.push(performance.now() - tk)
    }
    for (;;) {
      violations = (await s.diagnosticsFor(uri, WAIT)).filter(isShacl).length
      if (violations === broken) break
      await new Promise((r) => setTimeout(r, 2))
    }
    return performance.now() - t
  }
  for (let i = 0; i < saves; i++) saved.push(await saveAndBreak(0))
  for (let i = 0; i < typingRounds; i++) await saveAndBreak(keystrokes)
  record(`${id}-save`, `didSave -> SHACL diagnostics (${label})`, saved, { notes: `${violations} violations at the end` })
  record(`${id}-typing`, `keystroke -> diagnostics while validating (${label})`, typing)
}

scenario("shacl", "didSave validation of the workspace against one shape, with typing during it", async () => {
  const { ws, big, bigPath } = sharedWorkspace()
  addShape(ws)
  await withServer(ws, async (s) => {
    const uri = uriOf(bigPath)
    s.open(uri, big)
    await s.diagnosticsFor(uri, WAIT)
    const triples = (FILES * 200 + SUBJECTS) * 4 + 6
    await shaclRounds(s, uri, big, (t, k) => t.replace(`"Person ${k}"`, `"Person ${k}x"`), {
      id: "shacl",
      label: `~${(triples / 1000).toFixed(0)}k triples, 1 shape`,
    })
  })
})

// JSON-LD next to Turtle of the same length (lines), in an empty workspace, so each
// format is measured on its own. Documents are opened unsaved; each open uses new
// IRIs and is closed after, so no open replaces another. The JSON-LD file has an
// inline @context only: no context is fetched, and after the first open,
// re-checking reuses what the server cached.
let openRatio
let jsonldKeystrokeP50
scenario("jsonld", "JSON-LD against Turtle of the same length", async () => {
  await withDir("turtle-bench-jsonld-", async (jws) => {
    await withServer(jws, async (s) => {
      const formats = {
        turtle: { ext: "ttl", lang: "turtle", make: (tag) => bigDoc(SUBJECTS, tag) },
        jsonld: { ext: "jsonld", lang: "jsonld", make: (tag) => bigJsonLd(30000, tag) },
      }
      const opens = { turtle: [], jsonld: [] }
      for (let round = 0; round < 20; round++) {
        for (const [f, spec] of Object.entries(formats)) {
          const doc = spec.make(`o${round}_`)
          const u = uriOf(path.join(jws, `open${round}.${spec.ext}`))
          s.open(u, doc, spec.lang)
          const t = performance.now()
          await s.diagnosticsFor(u, WAIT)
          opens[f].push(performance.now() - t)
          s.close(u)
        }
      }
      for (const [f, spec] of Object.entries(formats)) {
        let doc = spec.make("")
        record(`${f}-open`, `${f}: didOpen -> diagnostics (${doc.split("\n").length} lines, ${(doc.length / 1024).toFixed(0)} KB)`, opens[f])
        const u = uriOf(path.join(jws, `big.${spec.ext}`))
        s.open(u, doc, spec.lang)
        await s.diagnosticsFor(u, WAIT)
        const keys = await time(25, async (i) => {
          doc = doc.replace(`"Person ${i}"`, `"Person ${i}x"`)
          s.change(u, doc)
          await s.diagnosticsFor(u, WAIT)
        })
        record(`${f}-keystroke`, `${f}: keystroke -> diagnostics`, keys, { gate: true })
        // Hover on the name property of a resource in the middle of the file.
        const pos = positionOf(doc, f === "jsonld" ? `"name": "Person 700"` : `foaf:name "Person ${Math.min(2500, SUBJECTS >> 1)}"`, 2)
        let found = "no hover"
        const hovers = await time(20, async () => {
          if (await s.request("textDocument/hover", { textDocument: { uri: u }, position: pos })) found = "hover"
        })
        record(`${f}-hover`, `${f}: hover on the name property`, hovers, { notes: found })
        if (f === "jsonld") {
          // Completion in a key in the middle of the file, and formatting it all.
          const at = positionOf(doc, `"name": "Person 700"`, 3)
          let items = 0
          const completions = await time(20, async () => {
            const res = await s.request("textDocument/completion", { textDocument: { uri: u }, position: at })
            items = (Array.isArray(res) ? res : (res?.items ?? [])).length
          })
          record("jsonld-completion", "jsonld: completion in a key", completions, { notes: `${items} items` })
          const fmts = await time(20, () =>
            s.request("textDocument/formatting", { textDocument: { uri: u }, options: { tabSize: 2, insertSpaces: true } }),
          )
          record("jsonld-format", "jsonld: format the whole file", fmts)
        }
        s.close(u)
      }
    })
  })
  const p50 = (id) => rows.find((r) => r.id === id)?.p50
  openRatio = p50("jsonld-open") / p50("turtle-open")
  jsonldKeystrokeP50 = p50("jsonld-keystroke")
})

// A namespace of own terms that the workspace describes, and a document that
// uses a thousand misspellings of them: every one is flagged, and the first
// hundred get a "did you mean" search over all the namespace's terms.
scenario("near-miss", "1,000 near-miss IRIs against a 7,000-term namespace", async () => {
  const terms = scaled(7000)
  const uses = scaled(1000)
  const rand = prng(7)
  const letters = "abcdefghijklmnopqrstuvwxyz"
  const word = () => {
    let w = ""
    for (let i = 0; i < 9; i++) w += letters[Math.floor(rand() * 26)]
    return w
  }
  const names = new Set()
  while (names.size < terms) names.add(word())
  const list = [...names]
  // A word one substitution away from a term and not a term itself.
  const typo = (w) => {
    for (;;) {
      const i = 2 + Math.floor(rand() * (w.length - 2))
      const t = w.slice(0, i) + letters[Math.floor(rand() * 26)] + w.slice(i + 1)
      if (!names.has(t)) return t
    }
  }
  const prefixes = `@prefix v: <http://example.org/vocab#> .\n@prefix ex: <http://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n`
  const vocab = prefixes + list.map((w) => `v:${w} a rdfs:Class ; rdfs:label "${w}" .`).join("\n") + "\n"
  const document = (tag) =>
    prefixes + Array.from({ length: uses }, (_, i) => `ex:s${tag}${i} a v:${typo(list[(i * 7) % list.length])} .`).join("\n") + "\n"
  await withDir("turtle-bench-nearmiss-", async (ws) => {
    fs.writeFileSync(path.join(ws, "vocab.ttl"), vocab)
    await withServer(ws, async (s) => {
      recordRss("rss-nearmiss", `near-miss: server RSS after indexing a ${terms}-term namespace`, rssMb(s.proc.pid))
      let flagged = 0
      let fixed = 0
      let codes = {}
      // Each open is a new URI: the empty report a close sends could otherwise
      // be taken for the next open's.
      const opens = await time(20, async (i) => {
        const u = uriOf(path.join(ws, `uses${i}.ttl`))
        s.open(u, document(`o${i}_`))
        const d = await s.diagnosticsFor(u, WAIT)
        codes = Object.fromEntries(d.map((x) => x.code).map((c) => [c, d.filter((x) => x.code === c).length]))
        const own = d.filter((x) => x.code === "undescribed-term")
        flagged = own.length
        fixed = own.filter((x) => x.data?.replace).length
        s.close(u)
      })
      record("near-miss-open", `near-miss: didOpen -> diagnostics (${uses} IRIs vs ${terms} terms)`, opens, {
        notes: `${flagged} undescribed-term, ${fixed} with a suggestion`,
      })
      expect(flagged === uses, `near-miss: expected ${uses} undescribed-term diagnostics, got ${flagged} (${JSON.stringify(codes)})`)
      expect(fixed > 0, "near-miss: no diagnostic got a suggestion, so the search is not exercised")
      let doc = document("k_")
      const u = uriOf(path.join(ws, "uses.ttl"))
      s.open(u, doc)
      await s.diagnosticsFor(u, WAIT)
      const keys = await time(25, async (i) => {
        doc = doc.replace(`ex:sk_${i} a`, `ex:sk_${i}x a`)
        s.change(u, doc)
        await s.diagnosticsFor(u, WAIT)
      })
      record("near-miss-keystroke", "near-miss: keystroke -> diagnostics", keys, { gate: true })
      s.close(u)
    })
  })
})

scenario("big-file", "a file over 2 MB: open and keystroke", async () => {
  const target = scaled(2_400_000)
  let n = Math.ceil(target / 140) // about 140 bytes per subject
  let doc = bigDoc(n)
  while (doc.length < target) doc = bigDoc((n = Math.ceil(n * 1.05)))
  await withDir("turtle-bench-bigfile-", async (ws) => {
    const file = path.join(ws, "huge.ttl")
    fs.writeFileSync(file, doc)
    await withServer(ws, async (s) => {
      const u = uriOf(file)
      s.open(u, doc)
      const t = performance.now()
      await s.diagnosticsFor(u, WAIT)
      const size = `${doc.split("\n").length} lines, ${(doc.length / 1024 / 1024).toFixed(1)} MB`
      record("big-file-open", `big file: didOpen -> diagnostics (${size})`, [performance.now() - t])
      const keys = await time(20, async (i) => {
        doc = doc.replace(`"Person ${i}"`, `"Person ${i}x"`)
        s.change(u, doc)
        await s.diagnosticsFor(u, WAIT)
      })
      record("big-file-keystroke", `big file: keystroke -> diagnostics (${size})`, keys, { gate: true })
      recordRss("rss-big-file", "big file: server RSS after the edits", rssMb(s.proc.pid))
    })
  })
})

// Completion where a lot is described: the candidates for an object are the typed
// subjects of the workspace, for a predicate the properties of the subject's type.
scenario("typed-completion", "typed completion over 40,000 described subjects", async () => {
  const subjects = scaled(40000)
  const perFile = 1000
  const header2 = `@prefix ex: <http://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n`
  await withDir("turtle-bench-typed-", async (ws) => {
    fs.writeFileSync(
      path.join(ws, "schema.ttl"),
      `${header2}@prefix owl: <http://www.w3.org/2002/07/owl#> .
ex:Widget a owl:Class ; rdfs:label "Widget" .
ex:Gadget a owl:Class ; rdfs:label "Gadget" .
ex:partOf a owl:ObjectProperty ; rdfs:domain ex:Widget ; rdfs:range ex:Widget ; rdfs:label "part of" .
ex:madeBy a owl:ObjectProperty ; rdfs:domain ex:Widget ; rdfs:range ex:Gadget ; rdfs:label "made by" .
ex:weight a owl:DatatypeProperty ; rdfs:domain ex:Widget ; rdfs:label "weight" .
`,
    )
    for (let f = 0; f * perFile < subjects; f++) {
      let out = header2
      for (let i = f * perFile; i < Math.min(subjects, (f + 1) * perFile); i++)
        out += `\nex:w${i} a ex:Widget ;\n    rdfs:label "Widget ${i}" ;\n    ex:partOf ex:w${(i + 1) % subjects} .\n`
      fs.writeFileSync(path.join(ws, `widgets${f}.ttl`), out)
    }
    await withServer(ws, async (s) => {
      record("typed-index", `typed completion: initial index of ${subjects} described subjects`, [s.indexMs])
      const complete = async (id, name, line) => {
        const u = uriOf(path.join(ws, `${id}.ttl`))
        const doc = header2 + `\n${line}`
        s.open(u, doc)
        await s.diagnosticsFor(u, WAIT)
        const pos = { line: doc.split("\n").length - 1, character: line.length }
        let items = 0
        let bytes = 0
        const samples = await time(20, async () => {
          const res = await s.request("textDocument/completion", { textDocument: { uri: u }, position: pos })
          items = (Array.isArray(res) ? res : (res?.items ?? [])).length
          bytes = JSON.stringify(res).length
        })
        record(id, name, samples, { notes: `${items} items, ${(bytes / 1024).toFixed(0)} KB` })
        expect(items > 0, `${name}: no completion items`)
        s.close(u)
      }
      await complete("typed-object", "typed completion: object of ex:partOf (range ex:Widget)", "ex:fresh ex:partOf ")
      await complete("typed-predicate", "typed completion: predicate of an ex:Widget", "ex:fresh a ex:Widget ; ")
      recordRss("rss-typed", "typed completion: server RSS", rssMb(s.proc.pid))
    })
  })
})

scenario("trig-shacl", "200 TriG datasets and one shape: didSave validation and typing", async () => {
  const datasets = scaled(200)
  const per = 50
  const trig = (f) => {
    let out = `${header}\nex:g${f} {\n`
    for (let i = 0; i < per; i++)
      out += `  ex:d${f}_${i} a foaf:Person ;\n      foaf:name "Person ${i}" ;\n      foaf:knows ex:d${f}_${(i + 1) % per} .\n`
    return out + "}\n"
  }
  await withDir("turtle-bench-trig-", async (ws) => {
    for (let f = 0; f < datasets; f++) fs.writeFileSync(path.join(ws, `data${f}.trig`), trig(f))
    fs.writeFileSync(path.join(ws, "shapes.ttl"), personShape())
    await withServer(ws, async (s) => {
      record("trig-index", `TriG: initial index of ${datasets} datasets`, [s.indexMs])
      const file = path.join(ws, "data0.trig")
      const doc = fs.readFileSync(file, "utf8")
      const u = uriOf(file)
      s.open(u, doc, "trig")
      await s.diagnosticsFor(u, WAIT)
      await shaclRounds(s, u, doc, (t, k) => t.replace(`"Person ${k}"`, `"Person ${k}x"`), {
        id: "trig-shacl",
        label: `${datasets} TriG datasets, ${datasets * per * 3} triples, 1 shape`,
        saves: 20,
      })
      recordRss("rss-trig", "TriG: server RSS at the end", rssMb(s.proc.pid))
    })
  })
})

scenario("document-symbol-rss", "memory after documentSymbol on a 100,000-triple file", async () => {
  const subjects = scaled(25000)
  const doc = bigDoc(subjects)
  await withDir("turtle-bench-symbols-", async (ws) => {
    const file = path.join(ws, "triples.ttl")
    fs.writeFileSync(file, doc)
    await withServer(ws, async (s) => {
      const u = uriOf(file)
      s.open(u, doc)
      await s.diagnosticsFor(u, WAIT)
      const before = rssMb(s.proc.pid)
      let count = 0
      let bytes = 0
      const samples = await time(20, async () => {
        const res = await s.request("textDocument/documentSymbol", { textDocument: { uri: u } })
        count = res.length
        bytes = JSON.stringify(res).length
      })
      const after = rssMb(s.proc.pid)
      record("symbols-100k", `documentSymbol (${subjects * 4} triples)`, samples, {
        rssMb: after,
        notes: `${count} symbols, ${(bytes / 1024 / 1024).toFixed(1)} MB response; RSS ${before ?? "?"} -> ${after ?? "?"} MB`,
      })
      recordRss("rss-symbols-before", "documentSymbol on a 100k-triple file: RSS before the first request", before)
      recordRss("rss-symbols-after", "documentSymbol on a 100k-triple file: RSS after 20 requests", after)
    })
  })
})

// The command-line check, as a CI job runs it: wall time of a whole process.
function runCheck(cwd, extraArgs, runs) {
  let status = null
  let outBytes = 0
  const samples = []
  for (let i = 0; i < runs; i++) {
    const t = performance.now()
    const r = spawnSync(serverPath, ["check", "--root", cwd, ...extraArgs, cwd], {
      cwd,
      env: serverEnv(),
      encoding: "utf8",
      maxBuffer: 1 << 30,
      timeout: 100000,
    })
    samples.push(performance.now() - t)
    status = r.status ?? `signal ${r.signal}`
    outBytes = (r.stdout ?? "").length
  }
  return { samples, status, outBytes }
}

scenario("check-cli", "the check command over the shared workspace, SHACL included", async () => {
  const { ws } = sharedWorkspace()
  addShape(ws)
  const r = runCheck(ws, [], 10)
  record("check-workspace", `check: ${FILES + 2} files, ${(FILES * 200 + SUBJECTS) * 4} triples, 1 shape`, r.samples, {
    notes: `exit ${r.status}, ${r.outBytes} bytes of report`,
  })
  expect(r.status === 0, `check over the workspace: expected exit 0, got ${r.status}`)
})

// Every line uses two prefixes the file never declares: two diagnostics a line,
// each with the candidates for its declaration looked up.
scenario("undeclared-prefix", "a 20,000-line file full of undeclared prefixes", async () => {
  const lines = scaled(20000)
  let doc = ""
  for (let i = 0; i < lines; i++) doc += `<http://example.org/s${i}> foaf:name "n${i}" ; rdfs:label "l${i}" .\n`
  await withDir("turtle-bench-prefix-", async (ws) => {
    const file = path.join(ws, "undeclared.ttl")
    fs.writeFileSync(file, doc)
    await withServer(ws, async (s) => {
      let count = 0
      const opens = await time(10, async (i) => {
        const o = uriOf(path.join(ws, `open${i}.ttl`))
        s.open(o, doc + `# ${i}\n`)
        count = (await s.diagnosticsFor(o, WAIT)).length
        s.close(o)
      })
      record("prefix-open", `undeclared prefixes: didOpen -> diagnostics (${lines} lines)`, opens, { notes: `${count} diagnostics` })
      expect(count >= lines, `undeclared prefixes: expected at least ${lines} diagnostics, got ${count}`)
      const u = uriOf(file)
      s.open(u, doc)
      await s.diagnosticsFor(u, WAIT)
      let text = doc
      const keys = await time(20, async (i) => {
        text = text.replace(`"n${i}"`, `"n${i}x"`)
        s.change(u, text)
        await s.diagnosticsFor(u, WAIT)
      })
      record("prefix-keystroke", `undeclared prefixes: keystroke -> diagnostics (${lines} lines)`, keys, { gate: true })
      recordRss("rss-prefix", "undeclared prefixes: server RSS at the end", rssMb(s.proc.pid))
    })
    const r = runCheck(ws, ["--no-shacl"], 3)
    record("check-prefix", `check: ${lines}-line undeclared-prefix file`, r.samples, {
      notes: `exit ${r.status}, ${(r.outBytes / 1024).toFixed(0)} KB of report`,
    })
    expect(r.status === 1, `check over the undeclared-prefix file: expected exit 1, got ${r.status}`)
  })
})

// ---- run

if (args.list) {
  for (const sc of SCENARIOS) console.log(`${sc.name.padEnd(20)} ${sc.about}`)
  process.exit(0)
}
const only = args.only ? new Set(args.only.split(",")) : null
for (const o of only ?? []) {
  if (SCENARIOS.some((sc) => sc.name === o)) continue
  console.error(`unknown scenario ${o}; --list shows them`)
  process.exit(2)
}

const started = new Date()
const load1 = os.loadavg()[0]
try {
  for (const sc of SCENARIOS) {
    if (only && !only.has(sc.name)) continue
    // A scenario that throws is a problem, not the end of the run: the others still
    // run, and the temporary folders are removed on the way out.
    try {
      await sc.run()
    } catch (e) {
      problem(`scenario ${sc.name}: ${e.stack ?? e}`)
    }
  }
} finally {
  if (shared) fs.rmSync(shared.ws, { recursive: true, force: true })
}

let commit = process.env.GITHUB_SHA
if (!commit) {
  try {
    commit = execFileSync("git", ["rev-parse", "HEAD"], { cwd: here, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim()
  } catch {}
}
const meta = {
  date: started.toISOString(),
  commit: commit ?? null,
  platform: `${process.platform}-${process.arch}`,
  node: process.version,
  cpus: os.cpus().length,
  load1: Math.round(load1 * 100) / 100,
  scale: SCALE,
  params: { subjects: SUBJECTS, files: FILES },
  scenarios: only ? [...only] : SCENARIOS.map((sc) => sc.name),
}

console.log(`\nturtle-lsp perf (${SUBJECTS} subjects in big.ttl, ${FILES} workspace files; scale ${SCALE}; load ${meta.load1} on ${meta.cpus} cpus)\n`)
console.table(
  rows.map((r) => ({
    metric: r.name,
    n: r.n,
    "p50 ms": r.p50 ?? "",
    "p95 ms": r.p95 ?? (r.max == null ? "" : `max ${r.max}`),
    "RSS MB": r.rssMb ?? "",
    notes: r.notes,
  })),
)
// The JSON-LD budgets (docs/specs/jsonld.md); tests/jsonld_budget.rs asserts
// them on the engine in release builds.
if (openRatio !== undefined)
  console.log(`JSON-LD budgets: open ${openRatio.toFixed(2)}x Turtle (budget 3x); keystroke p50 ${jsonldKeystrokeP50} ms (budget 10 ms)`)
if (rows.some((r) => r.id.startsWith("rss-")) && rows.every((r) => r.rssMb == null)) console.log("RSS is not readable on this system: the memory figures are empty.")

if (args.json) {
  fs.mkdirSync(path.dirname(path.resolve(args.json)), { recursive: true })
  fs.writeFileSync(args.json, JSON.stringify({ schema: 1, meta, rows, problems }, null, 2) + "\n")
  console.log(`wrote ${args.json}`)
}
if (problems.length) {
  console.error(`\n${problems.length} problem(s):\n${problems.map((p) => `  ${p.split("\n")[0]}`).join("\n")}`)
  process.exitCode = 1
}
