// The RDF Toolbox page: one input (a set of files) and four tools over it.
// The engine (turtle-lsp as WebAssembly) runs in a worker; every change to
// the input is analysed again, and only the answer to the latest input is shown.
import { SETS } from "./samples.js"

const TOOLS = [
  { id: "format", name: "Format", glyph: "¶", job: "Lays the file out again. The graph (or the query's algebra) is checked to be the same before anything is shown." },
  { id: "validate", name: "Validate", glyph: "✓", job: "Every problem turtle-lsp finds in the RDF files, with its fixes." },
  { id: "shacl", name: "SHACL", glyph: "◆", job: "The data checked against the shapes in the input." },
  { id: "sparql", name: "SPARQL", glyph: "?", job: "The query's problems and fixes, and the query formatted." },
]
const DEBOUNCE_MS = 200
// The files a folder contributes, by extension; anything else is skipped.
const KNOWN = /\.(ttl|turtle|trig|nt|nq|rq|sparql|ru|jsonld)$/i
// Limits for a folder, so a large tree cannot stall the tab.
const MAX_FOLDER_FILES = 500
const MAX_FILE_BYTES = 10 * 1024 * 1024
const SEVERITY = { 1: "Error", 2: "Warning", 3: "Info", 4: "Hint" }

const $ = (id) => document.getElementById(id)
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c])

// ---- state -------------------------------------------------------------------

const S = {
  files: [], // { name, text }
  current: null, // name of the file in the editor
  tool: TOOLS.some((t) => t.id === location.hash.slice(1)) ? location.hash.slice(1) : "validate",
  result: null, // the latest analysis: { files: [...], hasShapes }
  analysed: {}, // name -> the text the result is about
  currentLine: null, // { name, line } shown highlighted in the editor
  ms: null,
}
const fileOf = (name) => S.files.find((f) => f.name === name)
const resultOf = (name) => S.result?.files.find((f) => f.name === name)
const isSparql = (name) => resultOf(name)?.kind === "sparql" || /\.(rq|ru|sparql)$/i.test(name)
// Whether the result still describes `name` as it is now.
const fresh = (name) => S.analysed[name] !== undefined && S.analysed[name] === fileOf(name)?.text

// ---- the worker ------------------------------------------------------------------

const worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" })
const pending = new Map()
let nextId = 0
worker.onmessage = ({ data }) => {
  const p = pending.get(data.id)
  pending.delete(data.id)
  if (!p) return
  if ("ok" in data) p.resolve(data)
  else {
    if (data.crashed) notify("The engine crashed on this input and was restarted. The results shown are from before.")
    p.reject(new Error(data.error))
  }
}
function ask(op, files) {
  const id = ++nextId
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    worker.postMessage({ id, op, files })
  })
}

let seq = 0
let timer = null
function setBusy(busy) {
  document.body.dataset.busy = String(busy)
}
// Analyse the input after a pause in typing (now when `now`).
function schedule(now = false) {
  setBusy(true)
  clearTimeout(timer)
  timer = setTimeout(analyse, now ? 0 : DEBOUNCE_MS)
}
async function analyse() {
  const mine = ++seq
  const snapshot = S.files.map((f) => ({ name: f.name, text: f.text }))
  try {
    const { ok, ms } = await ask("analyse", snapshot)
    if (mine !== seq) return
    S.result = ok
    S.analysed = Object.fromEntries(snapshot.map((f) => [f.name, f.text]))
    S.ms = ms
    if (S.tool === "shacl" && !ok.hasShapes) S.tool = "validate"
    render()
    document.body.dataset.ready = "true"
  } catch (e) {
    if (mine === seq) notify(`Analysis failed: ${e.message}`)
  } finally {
    if (mine === seq) setBusy(false)
  }
}

let notifyTimer
function notify(message) {
  const el = $("crash")
  el.textContent = message
  el.hidden = false
  clearTimeout(notifyTimer)
  notifyTimer = setTimeout(() => (el.hidden = true), 6000)
}

// ---- input -----------------------------------------------------------------------

function uniqueName(name) {
  if (!fileOf(name)) return name
  const [, stem, ext] = name.match(/^(.*?)(\.[^.]*)?$/)
  for (let k = 2; ; k++) if (!fileOf(`${stem}-${k}${ext ?? ""}`)) return `${stem}-${k}${ext ?? ""}`
}
function addFiles(list) {
  let first
  for (const f of list) {
    const name = uniqueName(f.name)
    S.files.push({ name, text: f.text })
    first ??= name
  }
  if (first) select(first)
  schedule(true)
}
function loadSample(key) {
  S.files = structuredClone(SETS[key].files)
  S.current = null
  S.result = null
  S.analysed = {}
  S.currentLine = null
  select(S.files[0]?.name)
  schedule(true)
}
function select(name) {
  S.current = name ?? null
  const f = fileOf(name)
  const ed = $("editor")
  ed.value = f ? f.text : ""
  ed.disabled = !f
  ed.scrollTop = 0
  if (S.currentLine?.name !== name) S.currentLine = null
  renderInput()
  renderTool()
}
// Replace file texts (a fix, a formatting); the editor follows its file.
function setTexts(texts) {
  for (const [name, text] of Object.entries(texts)) {
    const f = fileOf(name)
    if (f) f.text = text
  }
  if (S.current in texts) {
    const ed = $("editor")
    const { scrollTop } = ed
    ed.value = fileOf(S.current).text
    ed.scrollTop = scrollTop
  }
  renderInput()
  schedule(true)
}
// The guess at a pasted text's syntax, for its name.
function pastedName(text) {
  const body = text.replace(/^\s*(#[^\n]*\n|\s+)*/g, "").replace(/^(\s*(PREFIX|BASE)\b[^\n]*\n)*/gi, "")
  // A JSON object opens with a key, an array with an object; TriG's graph
  // braces hold IRIs and prefixed names.
  if (/^\s*(\{\s*["}]|\[\s*[{\]])/.test(text)) return "pasted.jsonld"
  if (/^\s*(SELECT|CONSTRUCT|ASK|DESCRIBE|INSERT|DELETE|WITH|LOAD|CLEAR|DROP|CREATE|ADD|MOVE|COPY)\b/i.test(body)) return "pasted.rq"
  if (/(^|\n)\s*(GRAPH\s+)?(<[^>]*>|[\w-]*:[\w.-]*|_:[\w.-]+)?\s*\{/i.test(body) && !/^\s*@?prefix\b/i.test(body)) return "pasted.trig"
  return "pasted.ttl"
}

// ---- text helpers -----------------------------------------------------------------

function lineStarts(text) {
  const starts = [0]
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1)
  return starts
}
const offsetOf = (starts, text, p) => Math.min(text.length, (starts[p.line] ?? text.length) + p.character)
// Apply LSP text edits (UTF-16 positions, as JS strings count) to `text`.
function applyEdits(text, edits) {
  const starts = lineStarts(text)
  const spans = edits.map((e) => [offsetOf(starts, text, e.range.start), offsetOf(starts, text, e.range.end), e.newText])
  spans.sort((a, b) => b[0] - a[0])
  for (const [s, e, t] of spans) text = text.slice(0, s) + t + text.slice(e)
  return text
}

// Turtle, TriG, N-Triples, N-Quads and SPARQL tokens, enough to colour them.
const TOKEN =
  /(#[^\n]*)|("""[\s\S]*?"""|'''[\s\S]*?'''|"(?:[^"\\\n]|\\.)*"|'(?:[^'\\\n]|\\.)*')|(<[^<>"{}|^`\\\s]*>)|(@[a-zA-Z]+(?:-[a-zA-Z0-9]+)*)|([?$][A-Za-z_0-9·À-￿]+)|([+-]?(?:\d+\.\d*|\.\d+|\d+)(?:[eE][+-]?\d+)?)|((?:[A-Za-zÀ-￿][\w.·À-￿-]*)?:(?:[\wÀ-￿:%-](?:[\w.·À-￿:%-]*[\wÀ-￿:%-])?)?)|([A-Za-z_]+)/g
const KEYWORDS = new Set(
  "a true false PREFIX BASE SELECT DISTINCT REDUCED CONSTRUCT DESCRIBE ASK FROM NAMED WHERE GRAPH OPTIONAL UNION MINUS FILTER BIND VALUES SERVICE SILENT ORDER BY ASC DESC GROUP HAVING LIMIT OFFSET AS INSERT DELETE DATA WITH USING DEFAULT ALL LOAD CLEAR DROP CREATE ADD MOVE COPY INTO TO UNDEF NOT IN EXISTS".split(" ")
)
const CLASSES = ["", "t-com", "t-str", "t-iri", "t-lang", "t-var", "t-num", "t-pn", "t-kw"]
const MAX_COLOURED = 300_000

function highlight(text, marks, current) {
  const tok = new Uint8Array(text.length)
  if (text.length <= MAX_COLOURED) {
    for (const m of text.matchAll(TOKEN)) {
      let k = m.slice(1).findIndex((g) => g !== undefined) + 1
      if (k === 4 && /^@(prefix|base)$/i.test(m[0])) k = 8
      if (k === 8 && !KEYWORDS.has(m[0]) && !KEYWORDS.has(m[0].toUpperCase())) k = 0
      tok.fill(k, m.index, m.index + m[0].length)
    }
  }
  const sev = new Uint8Array(text.length)
  for (const m of marks) sev.fill(m.sev, m.start, Math.max(m.end, m.start + 1))
  const starts = lineStarts(text)
  let html = ""
  starts.forEach((s, i) => {
    const e = i + 1 < starts.length ? starts[i + 1] - 1 : text.length
    let line = ""
    for (let j = s; j < e; ) {
      let k = j + 1
      while (k < e && tok[k] === tok[j] && sev[k] === sev[j]) k++
      const cls = [CLASSES[tok[j]], sev[j] ? `mk s${sev[j]}` : ""].filter(Boolean).join(" ")
      const piece = esc(text.slice(j, k))
      line += cls ? `<span class="${cls}">${piece}</span>` : piece
      j = k
    }
    html += `<div class="line${current === i ? " current" : ""}" data-n="${i + 1}">${line || " "}</div>`
  })
  return html
}

// Line diff: the common head and tail kept, the middle compared line by line
// when small enough, else shown as replaced.
function lineDiff(a, b) {
  const A = a.split("\n"), B = b.split("\n")
  let h = 0
  while (h < A.length && h < B.length && A[h] === B[h]) h++
  let t = 0
  while (t < A.length - h && t < B.length - h && A[A.length - 1 - t] === B[B.length - 1 - t]) t++
  const a2 = A.slice(h, A.length - t), b2 = B.slice(h, B.length - t)
  const out = A.slice(0, h).map((x, i) => ({ k: " ", x, n: i + 1 }))
  if (a2.length * b2.length <= 4_000_000) {
    const n = a2.length, m = b2.length
    const L = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1))
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) L[i][j] = a2[i] === b2[j] ? L[i + 1][j + 1] + 1 : Math.max(L[i + 1][j], L[i][j + 1])
    let i = 0, j = 0
    while (i < n || j < m) {
      if (i < n && j < m && a2[i] === b2[j]) out.push({ k: " ", x: b2[j++], n: h + j }), i++
      else if (j < m && (i === n || L[i][j + 1] > L[i + 1][j])) out.push({ k: "+", x: b2[j++], n: h + j })
      else out.push({ k: "-", x: a2[i++], n: null })
    }
  } else {
    for (const x of a2) out.push({ k: "-", x, n: null })
    b2.forEach((x, j) => out.push({ k: "+", x, n: h + j + 1 }))
  }
  const base = h + b2.length
  A.slice(A.length - t).forEach((x, i) => out.push({ k: " ", x, n: base + i + 1 }))
  return out
}
function diffHtml(a, b, context = 3) {
  const d = lineDiff(a, b)
  const keep = d.map((x) => x.k !== " ")
  const show = d.map((_, i) => keep.slice(Math.max(0, i - context), i + context + 1).some(Boolean))
  let html = ""
  let gap = false
  d.forEach((x, i) => {
    if (!show[i]) {
      if (!gap) html += `<div class="gap"><span class="no"></span><span class="tx">⋯</span></div>`
      gap = true
      return
    }
    gap = false
    const cls = x.k === "+" ? "add" : x.k === "-" ? "del" : ""
    html += `<div class="${cls}"><span class="no">${x.n ?? ""}</span><span class="tx">${esc(x.k === " " ? " " : x.k)} ${esc(x.x)}</span></div>`
  })
  const added = d.filter((x) => x.k === "+").length, removed = d.filter((x) => x.k === "-").length
  return { html: `<div class="diff">${html}</div>`, added, removed }
}

// ---- rendering ---------------------------------------------------------------------

const rdfFiles = () => (S.result?.files ?? []).filter((f) => f.kind !== "sparql")
const sparqlFiles = () => (S.result?.files ?? []).filter((f) => f.kind === "sparql")
const shaclOf = (f) => f.diagnostics.filter((d) => d.code === "shacl-violation")
const problemsOf = (f) => f.diagnostics.filter((d) => d.code !== "shacl-violation")
const fixable = (files) => files.reduce((n, f) => n + problemsOf(f).filter((d) => d.fixes.some((x) => x.preferred)).length, 0)

function counts() {
  const files = S.result?.files ?? []
  return {
    format: files.filter((f) => f.format?.status === "changed").length,
    validate: rdfFiles().reduce((n, f) => n + problemsOf(f).length, 0),
    shacl: files.reduce((n, f) => n + shaclOf(f).length, 0),
    sparql: sparqlFiles().reduce((n, f) => n + f.diagnostics.length, 0),
  }
}

function render() {
  renderTabs()
  renderInput()
  renderTool()
  $("timing").textContent = S.ms == null ? "" : `analysed in ${Math.max(1, Math.round(S.ms))} ms`
}

function renderTabs() {
  const c = counts()
  const tools = TOOLS.filter((t) => t.id !== "shacl" || S.result?.hasShapes)
  $("note").hidden = Boolean(S.result?.hasShapes) || !S.result
  $("tabs").innerHTML = tools
    .map((t) => {
      const n = c[t.id]
      const cls = t.id === "format" ? "" : n ? "bad" : "ok"
      const badge = t.id !== "format" && !n ? "✓" : n
      return `<button class="tab" role="tab" id="tab-${t.id}" aria-selected="${S.tool === t.id}" aria-controls="tool" tabindex="${S.tool === t.id ? 0 : -1}" data-tool="${t.id}"><span class="g" aria-hidden="true">${t.glyph}</span><span>${t.name}</span><span class="count ${cls}">${badge}</span></button>`
    })
    .join("")
}

function renderInput() {
  $("input-stat").textContent = `${S.files.length} file${S.files.length === 1 ? "" : "s"}, shared by every tool`
  $("files").innerHTML = S.files
    .map((f) => {
      const r = fresh(f.name) ? resultOf(f.name) : null
      const n = r ? r.diagnostics.length : null
      const badge = n == null ? "" : `<span class="n${n ? "" : " ok"}" title="${n} problem${n === 1 ? "" : "s"}">${n || "✓"}</span>`
      return `<span class="fchip${f.name === S.current ? " on" : ""}"><button class="nm" data-file="${esc(f.name)}">${esc(f.name)}</button>${badge}<button class="x" data-remove="${esc(f.name)}" aria-label="Remove ${esc(f.name)}">×</button></span>`
    })
    .join("")
  renderEditor()
}

function renderEditor() {
  const f = fileOf(S.current)
  if (!f) {
    $("hl").innerHTML = ""
    return
  }
  const marks = []
  if (fresh(f.name)) {
    const starts = lineStarts(f.text)
    for (const d of resultOf(f.name).diagnostics) {
      marks.push({ start: offsetOf(starts, f.text, d.range.start), end: offsetOf(starts, f.text, d.range.end), sev: d.severity ?? 1 })
    }
    marks.sort((a, b) => b.sev - a.sev)
  }
  const line = S.currentLine?.name === f.name ? S.currentLine.line : null
  $("hl").innerHTML = highlight(f.text, marks, line)
  syncScroll()
}
function syncScroll() {
  $("hl").scrollTop = $("editor").scrollTop
  $("hl").scrollLeft = $("editor").scrollLeft
}

// A problem, with where it is, its source line and its fixes.
function problemHtml(name, d, di, { message = true } = {}) {
  const f = fileOf(name)
  const lineText = f ? f.text.split("\n")[d.range.start.line] ?? "" : ""
  const fixes = d.fixes
    .map((x, fi) => `<button class="ibtn sm${x.preferred ? " pri" : ""}" data-fix="${esc(name)}" data-d="${di}" data-f="${fi}">${esc(x.title)}</button>`)
    .join("")
  return `<div class="problem s${d.severity ?? 1}"><div class="meta"><button class="loc" data-loc="${esc(name)}" data-line="${d.range.start.line}" data-ch="${d.range.start.character}" data-eline="${d.range.end.line}" data-ech="${d.range.end.character}">${esc(name)}:${d.range.start.line + 1}</button><span>${SEVERITY[d.severity] ?? ""}</span><span class="code">${esc(d.code ?? "")}</span></div>${message ? `<p class="msg">${esc(d.message)}</p>` : ""}${lineText.trim() ? `<pre class="src">${esc(lineText.trim())}</pre>` : ""}${fixes ? `<div class="fixes">${fixes}</div>` : ""}</div>`
}
// The problems of `files` (results), each list under its file name.
function problemList(files, filter) {
  let html = ""
  for (const f of files) {
    const items = f.diagnostics.map((d, i) => [d, i]).filter(([d]) => filter(d))
    if (!items.length) continue
    html += `<h3 class="group mono">${esc(f.name)}</h3>` + items.map(([d, i]) => problemHtml(f.name, d, i)).join("")
  }
  return html
}

function formatHtml(name, { applyLabel = "Apply formatting" } = {}) {
  const r = resultOf(name)
  const f = fileOf(name)
  if (!r || !f || !r.format) return `<p class="empty">No formatting for this file.</p>`
  const guarantee = r.format.guarantee[0].toUpperCase() + r.format.guarantee.slice(1)
  if (r.format.status === "refused") return `<p class="refused">Not formatted: ${esc(r.format.reason)}</p>`
  if (r.format.status === "unchanged") return `<div class="thd"><b>Already formatted</b><span class="guar">${esc(guarantee)}</span></div>`
  const d = diffHtml(f.text, r.format.text)
  return `<div class="thd"><span class="guar">${esc(guarantee)}</span><span class="stat">+${d.added} −${d.removed} lines</span><span class="sp"></span><button class="ibtn pri" data-format="${esc(name)}">${applyLabel}</button><button class="ibtn" data-copy="${esc(name)}">Copy</button></div>${d.html}`
}

function renderTool() {
  const t = TOOLS.find((x) => x.id === S.tool)
  $("tool-title").textContent = `${t.glyph} ${t.name}`
  $("tool-job").textContent = t.job
  const body = $("tool")
  const keep = body.dataset.tool === S.tool ? body.scrollTop : 0
  body.dataset.tool = S.tool
  body.setAttribute("aria-labelledby", `tab-${S.tool}`)
  body.setAttribute("role", "tabpanel")
  if (!S.result) {
    body.innerHTML = `<p class="empty">Analysing…</p>`
    return
  }
  let stat = ""
  let html = ""
  if (S.tool === "format") {
    const name = S.current
    stat = name ?? ""
    const changed = S.result.files.filter((f) => f.format?.status === "changed")
    const all = changed.length > 1 ? `<div class="thd"><span class="stat">${changed.length} files would change</span><span class="sp"></span><button class="ibtn" data-format-all>Apply to all ${changed.length} files</button></div>` : ""
    html = name ? all + formatHtml(name) : `<p class="empty">No input.</p>`
  } else if (S.tool === "validate") {
    const files = rdfFiles()
    const n = files.reduce((k, f) => k + problemsOf(f).length, 0)
    const fixes = fixable(files)
    stat = "all RDF files"
    html =
      `<div class="thd"><b>${n} problem${n === 1 ? "" : "s"}</b><span class="stat">in ${files.filter((f) => problemsOf(f).length).length} of ${files.length} files</span><span class="sp"></span>${fixes ? `<button class="ibtn pri" data-fix-all="rdf">Apply all ${fixes} fixes</button>` : ""}</div>` +
      (problemList(files, (d) => d.code !== "shacl-violation") || `<p class="empty">No problems.</p>`)
  } else if (S.tool === "shacl") {
    stat = "data × shapes"
    const groups = new Map()
    for (const f of S.result.files) {
      f.diagnostics.forEach((d, i) => {
        if (d.code !== "shacl-violation") return
        if (!groups.has(d.message)) groups.set(d.message, [])
        groups.get(d.message).push([f.name, d, i])
      })
    }
    const n = [...groups.values()].reduce((k, g) => k + g.length, 0)
    html = `<div class="thd"><b>${n} violation${n === 1 ? "" : "s"}</b><span class="stat">of ${groups.size} constraint${groups.size === 1 ? "" : "s"}</span></div>`
    for (const [message, items] of groups) {
      html += `<h3 class="group">${esc(message)}</h3>` + items.map(([name, d, i]) => problemHtml(name, d, i, { message: false })).join("")
    }
    if (!n) html += `<p class="empty">The data conforms to the shapes.</p>`
  } else {
    const queries = sparqlFiles()
    const q = queries.find((f) => f.name === S.current) ?? queries[0]
    if (!q) {
      html = `<p class="empty">No queries in the input. Paste a query, open a .rq or .ru file, or load the “Queries only” sample.</p>`
    } else {
      stat = q.name
      const fixes = fixable(queries)
      html =
        `<div class="thd"><b>${q.diagnostics.length} problem${q.diagnostics.length === 1 ? "" : "s"}</b><span class="stat">in ${esc(q.name)}${queries.length > 1 ? ` (${queries.length} queries in the input)` : ""}</span><span class="sp"></span>${fixes ? `<button class="ibtn" data-fix-all="sparql">Apply all ${fixes} fixes</button>` : ""}</div>` +
        (problemList([q], () => true) || `<p class="empty">No problems.</p>`) +
        `<h3 class="group">Formatted</h3>` +
        formatHtml(q.name)
    }
  }
  $("tool-stat").textContent = stat
  body.innerHTML = html
  body.scrollTop = keep
}

// ---- actions -------------------------------------------------------------------------

function showLocation(name, range) {
  if (S.current !== name) select(name)
  const ed = $("editor")
  const f = fileOf(name)
  const starts = lineStarts(f.text)
  ed.focus({ preventScroll: true })
  ed.setSelectionRange(offsetOf(starts, f.text, range.start), offsetOf(starts, f.text, range.end))
  ed.scrollTop = Math.max(0, range.start.line * 19 - ed.clientHeight / 3)
  S.currentLine = { name, line: range.start.line }
  renderEditor()
  if (window.matchMedia("(max-width: 760px)").matches) $("editor-wrap").scrollIntoView({ block: "center" })
}

function applyFix(name, di, fi) {
  const fix = resultOf(name)?.diagnostics[di]?.fixes[fi]
  if (!fix) return
  // A fix is about the text it was computed for.
  if (!Object.keys(fix.edits).every(fresh)) return schedule(true)
  const texts = {}
  for (const [file, edits] of Object.entries(fix.edits)) texts[file] = applyEdits(fileOf(file).text, edits)
  setTexts(texts)
}

// Fix the SPARQL or the RDF files, with the others as context: the fixes
// are the ones analysis showed, which knew every file.
async function fixAll(which) {
  const names = new Set((which === "sparql" ? sparqlFiles() : rdfFiles()).map((f) => f.name))
  const files = S.files.map((f) => ({ name: f.name, text: f.text, context: !names.has(f.name) }))
  setBusy(true)
  try {
    const { ok } = await ask("fix_all", files)
    setTexts(Object.fromEntries(ok.files.map((f) => [f.name, f.text])))
  } catch (e) {
    notify(`Fixing failed: ${e.message}`)
    schedule(true)
  }
}

function applyFormat(names) {
  const texts = {}
  for (const name of names) {
    const r = resultOf(name)
    if (r?.format?.status === "changed" && fresh(name)) texts[name] = r.format.text
  }
  setTexts(texts)
}

function download() {
  const f = fileOf(S.current)
  if (!f) return
  const url = URL.createObjectURL(new Blob([f.text], { type: "text/plain" }))
  const a = Object.assign(document.createElement("a"), { href: url, download: f.name })
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

// Files chosen or dropped, each with its path in the folder it came from
// (`path`, "" for a single file). From a folder only RDF and SPARQL files
// are taken, named by their path below the folder.
async function readFiles(entries) {
  const take = []
  let skipped = 0
  for (const { file, path } of entries) {
    const fromFolder = path.includes("/")
    if ((fromFolder && !KNOWN.test(file.name)) || file.size > MAX_FILE_BYTES || take.length >= MAX_FOLDER_FILES) skipped++
    else take.push({ file, name: fromFolder ? path.slice(path.indexOf("/") + 1) : file.name })
  }
  take.sort((a, b) => a.name.localeCompare(b.name))
  addFiles(await Promise.all(take.map(async ({ file, name }) => ({ name, text: await file.text() }))))
  if (skipped) notify(`${skipped} file${skipped === 1 ? "" : "s"} skipped: only Turtle, TriG, N-Triples, N-Quads and SPARQL files up to 10 MB are read, at most ${MAX_FOLDER_FILES}.`)
}
const picked = (list) => [...list].map((file) => ({ file, path: file.webkitRelativePath ?? "" }))
// The files under dropped items, folders walked.
async function dropped(items) {
  const out = []
  const walk = async (entry) => {
    if (entry.isFile) {
      const file = await new Promise((res, rej) => entry.file(res, rej))
      out.push({ file, path: entry.fullPath.replace(/^\//, "") })
    } else if (entry.isDirectory) {
      const reader = entry.createReader()
      for (;;) {
        const batch = await new Promise((res, rej) => reader.readEntries(res, rej))
        if (!batch.length) break
        for (const e of batch) await walk(e)
      }
    }
  }
  for (const entry of [...items].map((i) => i.webkitGetAsEntry?.()).filter(Boolean)) await walk(entry)
  return out
}

// ---- wiring ---------------------------------------------------------------------------

function wire() {
  $("sample").innerHTML = Object.entries(SETS).map(([k, s]) => `<option value="${k}">${esc(s.label)}</option>`).join("")
  $("sample").addEventListener("change", (e) => loadSample(e.target.value))

  $("tabs").addEventListener("click", (e) => {
    const b = e.target.closest("[data-tool]")
    if (!b) return
    S.tool = b.dataset.tool
    history.replaceState(null, "", `#${S.tool}`)
    renderTabs()
    const query = sparqlFiles()[0]
    if (S.tool === "sparql" && query && !isSparql(S.current)) select(query.name)
    else renderTool()
  })
  $("tabs").addEventListener("keydown", (e) => {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return
    const tabs = [...$("tabs").querySelectorAll("[role=tab]")]
    const i = tabs.findIndex((t) => t.dataset.tool === S.tool)
    const next = tabs[(i + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length]
    next.click()
    $("tabs").querySelector(`[data-tool="${next.dataset.tool}"]`).focus()
  })

  $("files").addEventListener("click", (e) => {
    const pick = e.target.closest("[data-file]")
    if (pick) return select(pick.dataset.file)
    const rm = e.target.closest("[data-remove]")
    if (!rm) return
    S.files = S.files.filter((f) => f.name !== rm.dataset.remove)
    if (S.current === rm.dataset.remove) select(S.files[0]?.name)
    else renderInput()
    schedule(true)
  })

  const ed = $("editor")
  ed.addEventListener("input", () => {
    const f = fileOf(S.current)
    if (!f) return
    f.text = ed.value
    S.currentLine = null
    renderEditor()
    schedule()
  })
  ed.addEventListener("scroll", syncScroll)

  $("tool").addEventListener("click", (e) => {
    const b = e.target.closest("button")
    if (!b) return
    const d = b.dataset
    if (d.loc !== undefined) {
      showLocation(d.loc, { start: { line: +d.line, character: +d.ch }, end: { line: +d.eline, character: +d.ech } })
    } else if (d.fix !== undefined) applyFix(d.fix, +d.d, +d.f)
    else if (d.fixAll !== undefined) fixAll(d.fixAll)
    else if (d.format !== undefined) applyFormat([d.format])
    else if (d.formatAll !== undefined) applyFormat(S.result.files.map((f) => f.name))
    else if (d.copy !== undefined) navigator.clipboard?.writeText(resultOf(d.copy)?.format?.text ?? "").catch(() => notify("Copying is not allowed here; select the text instead."))
  })

  $("paste").addEventListener("click", () => {
    $("paste-box").hidden = !$("paste-box").hidden
    if (!$("paste-box").hidden) $("paste-text").focus()
  })
  $("paste-cancel").addEventListener("click", () => ($("paste-box").hidden = true))
  $("paste-add").addEventListener("click", () => {
    const text = $("paste-text").value
    if (!text.trim()) return
    $("paste-text").value = ""
    $("paste-box").hidden = true
    addFiles([{ name: pastedName(text), text }])
  })
  $("open").addEventListener("click", () => $("picker").click())
  $("open-folder").addEventListener("click", () => $("folder-picker").click())
  for (const id of ["picker", "folder-picker"]) {
    $(id).addEventListener("change", (e) => {
      readFiles(picked(e.target.files))
      e.target.value = ""
    })
  }
  $("download")?.addEventListener("click", download)

  const pane = document.querySelector(".pane.input")
  pane.addEventListener("dragover", (e) => {
    e.preventDefault()
    pane.classList.add("drop")
  })
  pane.addEventListener("dragleave", () => pane.classList.remove("drop"))
  pane.addEventListener("drop", (e) => {
    e.preventDefault()
    pane.classList.remove("drop")
    const items = e.dataTransfer?.items
    if (items?.length && items[0].webkitGetAsEntry) dropped(items).then(readFiles)
    else if (e.dataTransfer?.files?.length) readFiles(picked(e.dataTransfer.files))
  })
}

wire()
ask("version").then(({ ok }) => ($("version").textContent = `turtle-lsp ${ok.version}`), () => {})
loadSample("starter")
