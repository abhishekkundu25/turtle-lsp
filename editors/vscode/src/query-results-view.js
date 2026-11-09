// The Query Results panel's pure half, shared by the extension
// (src/query-results.ts), the webview and the node tests: the server's
// `turtle.runQuery` answer read at the edge into a result with one answer
// kind per query form, the table model of each kind, sorting, IRIs shortened
// with the query's prefixes, the header line and the inferred note, the
// page as HTML, the exports (CSV, TSV, JSON), and the webview's messages
// read at the edge. In a webview (`acquireVsCodeApi` exists) it also mounts
// the page; "build" bundles it into out/query-results/.

const XSD = "http://www.w3.org/2001/XMLSchema#"
const REASONINGS = ["off", "default", "complete"]
const REASONING_LABELS = { off: "Off", default: "Default rules", complete: "Complete rules" }

/**
 * @typedef {"off" | "default" | "complete"} Reasoning
 * @typedef {{ type: "uri" | "bnode", value: string }
 *   | { type: "literal", value: string, datatype?: string, "xml:lang"?: string, "its:dir"?: string }
 *   | { type: "triple", value: { subject: Term, predicate: Term, object: Term } }} Term
 * @typedef {{ kind: "select", vars: string[], rows: (Term | undefined)[][], omitted: number }
 *   | { kind: "ask", boolean: boolean }
 *   | { kind: "construct", triples: { subject: Term, predicate: Term, object: Term }[], omitted: number }
 *   | { kind: "error", message: string, line?: number, column?: number }} Answer
 * @typedef {{ rules: Reasoning, triples: number, findings: number }
 *   | { skipped: { triples: number, limit: number } }
 *   | { skipped: { inferred: true, limit: number } }} Inferred
 * @typedef {{ kind: "result", uri: string, name: string, files: number, elapsedMs: number,
 *   reasoning: Reasoning, prefixes: Record<string, string>, answer: Answer, inferred?: Inferred }} Result
 * @typedef {Result | { kind: "cancelled", uri: string }} RunAnswer
 * @typedef {{ phase: "running", result?: Result, reasoning?: Reasoning }
 *   | { phase: "done", result: Result }
 *   | { phase: "cancelled", result?: Result, reasoning?: Reasoning }
 *   | { phase: "failed", message: string, result?: Result, reasoning?: Reasoning }} View
 * @typedef {{ text: string, title?: string }} Cell
 * @typedef {{ kind: "table", columns: string[], rows: Cell[][], shown: number, total: number }
 *   | { kind: "boolean", value: boolean }
 *   | { kind: "error", message: string, at: string | undefined }} Table
 * @typedef {{ column: number, descending: boolean }} Sort
 * @typedef {{ type: "ready" } | { type: "rerun", reasoning: Reasoning }
 *   | { type: "copy", order: number[] | undefined }
 *   | { type: "export", format: "csv" | "json", order: number[] | undefined }
 *   | { type: "drawn", status: string }} Message
 */

const isObject = (v) => typeof v === "object" && v !== null && !Array.isArray(v)
const isCount = (v) => Number.isInteger(v) && v >= 0
const str = (v) => (typeof v === "string" ? v : undefined)

/** @returns {Reasoning | undefined} */
export function parseReasoning(v) {
  return REASONINGS.includes(v) ? v : undefined
}

/** @returns {Term | undefined} */
function parseTerm(v) {
  if (!isObject(v)) return undefined
  if (v.type === "uri" || v.type === "bnode") return typeof v.value === "string" ? { type: v.type, value: v.value } : undefined
  if (v.type === "literal" && typeof v.value === "string") {
    const t = { type: "literal", value: v.value }
    for (const key of ["datatype", "xml:lang", "its:dir"]) if (typeof v[key] === "string") t[key] = v[key]
    return t
  }
  if (v.type === "triple" && isObject(v.value)) {
    const parts = parseTriple(v.value)
    return parts && { type: "triple", value: parts }
  }
  return undefined
}

function parseTriple(v) {
  if (!isObject(v)) return undefined
  const subject = parseTerm(v.subject)
  const predicate = parseTerm(v.predicate)
  const object = parseTerm(v.object)
  return subject && predicate && object ? { subject, predicate, object } : undefined
}

const unreadable = { kind: "error", message: "The server's answer could not be read." }

/** @returns {Answer} */
function parseQueryAnswer(a) {
  if (!isObject(a)) return unreadable
  switch (a.kind) {
    case "select": {
      const vars = a.head?.vars
      const bindings = a.results?.bindings
      if (!Array.isArray(vars) || !vars.every((v) => typeof v === "string") || !Array.isArray(bindings)) return unreadable
      const rows = bindings.map((b) => vars.map((v) => (isObject(b) ? parseTerm(b[v]) : undefined)))
      return { kind: "select", vars, rows, omitted: isCount(a.omitted) ? a.omitted : 0 }
    }
    case "ask":
      return typeof a.boolean === "boolean" ? { kind: "ask", boolean: a.boolean } : unreadable
    case "construct": {
      if (!Array.isArray(a.triples)) return unreadable
      const triples = a.triples.map(parseTriple)
      if (!triples.every(Boolean)) return unreadable
      return { kind: "construct", triples, omitted: isCount(a.omitted) ? a.omitted : 0 }
    }
    case "error": {
      const e = { kind: "error", message: str(a.message) ?? "The query failed." }
      if (isCount(a.line) && isCount(a.column)) Object.assign(e, { line: a.line, column: a.column })
      return e
    }
    default:
      return unreadable
  }
}

/** @returns {Inferred | undefined} */
function parseInferred(v) {
  if (!isObject(v)) return undefined
  if (parseReasoning(v.rules) && isCount(v.triples)) return { rules: v.rules, triples: v.triples, findings: isCount(v.findings) ? v.findings : 0 }
  const s = v.skipped
  if (isObject(s) && isCount(s.limit)) {
    if (s.inferred === true) return { skipped: { inferred: true, limit: s.limit } }
    if (isCount(s.triples)) return { skipped: { triples: s.triples, limit: s.limit } }
  }
  return undefined
}

/**
 * The server's answer to `turtle.runQuery`: a result, a cancelled run, or
 * `undefined` for anything else (a server without the result object answers
 * a summary string).
 * @returns {RunAnswer | undefined}
 */
export function parseAnswer(v) {
  if (!isObject(v) || typeof v.uri !== "string") return undefined
  if (v.cancelled === true) return { kind: "cancelled", uri: v.uri }
  if (!("answer" in v)) return undefined
  const prefixes = {}
  if (isObject(v.prefixes)) for (const [p, ns] of Object.entries(v.prefixes)) if (typeof ns === "string") prefixes[p] = ns
  /** @type {Result} */
  const r = {
    kind: "result",
    uri: v.uri,
    name: str(v.name) ?? v.uri.slice(v.uri.lastIndexOf("/") + 1),
    files: isCount(v.files) ? v.files : 0,
    elapsedMs: isCount(v.elapsedMs) ? v.elapsedMs : 0,
    reasoning: parseReasoning(v.reasoning) ?? "off",
    prefixes,
    answer: parseQueryAnswer(v.answer),
  }
  const inferred = parseInferred(v.inferred)
  if (inferred) r.inferred = inferred
  return r
}

const ORDER_MAX = 10_000_000
const parseOrder = (v) => (v === undefined ? { ok: true, order: undefined } : Array.isArray(v) && v.every((i) => isCount(i) && i < ORDER_MAX) ? { ok: true, order: v } : { ok: false })

/**
 * A message from the webview, or `undefined` for one it does not send.
 * @returns {Message | undefined}
 */
export function parseMessage(m) {
  if (!isObject(m)) return undefined
  switch (m.type) {
    case "ready":
      return { type: "ready" }
    case "rerun": {
      const reasoning = parseReasoning(m.reasoning)
      return reasoning && { type: "rerun", reasoning }
    }
    case "copy": {
      const o = parseOrder(m.order)
      return o.ok ? { type: "copy", order: o.order } : undefined
    }
    case "export": {
      const o = parseOrder(m.order)
      return o.ok && (m.format === "csv" || m.format === "json") ? { type: "export", format: m.format, order: o.order } : undefined
    }
    case "drawn":
      return typeof m.status === "string" ? { type: "drawn", status: m.status } : undefined
    default:
      return undefined
  }
}

// PN_PREFIX and PN_LOCAL without escapes (SPARQL 1.1 grammar), as the server's
// `syntax::names::shorten` takes them.
const PN_CHARS_BASE = "A-Za-z\\u00C0-\\u00D6\\u00D8-\\u00F6\\u00F8-\\u02FF\\u0370-\\u037D\\u037F-\\u1FFF\\u200C-\\u200D\\u2070-\\u218F\\u2C00-\\u2FEF\\u3001-\\uD7FF\\uF900-\\uFDCF\\uFDF0-\\uFFFD\\u{10000}-\\u{EFFFF}"
const PN_CHARS_U = `${PN_CHARS_BASE}_`
const PN_CHARS = `${PN_CHARS_U}\\-0-9\\u00B7\\u0300-\\u036F\\u203F-\\u2040`
const PLX = "%[0-9A-Fa-f]{2}"
const PN_PREFIX = new RegExp(`^(?:[${PN_CHARS_BASE}](?:[${PN_CHARS}.]*[${PN_CHARS}])?)?$`, "u")
const PN_LOCAL = new RegExp(`^(?:(?:[${PN_CHARS_U}:0-9]|${PLX})(?:(?:[${PN_CHARS}.:]|${PLX})*(?:[${PN_CHARS}:]|${PLX}))?)?$`, "u")

/**
 * `prefix:local` for `iri` with the longest namespace of `prefixes` whose
 * rest is a local name as is; ties go to the shorter prefix, then the
 * smaller. `undefined` when none applies.
 * @param {string} iri
 * @param {Record<string, string>} prefixes
 */
export function shorten(iri, prefixes) {
  let best
  for (const [name, ns] of Object.entries(prefixes)) {
    if (!ns || !iri.startsWith(ns) || !PN_PREFIX.test(name) || !PN_LOCAL.test(iri.slice(ns.length))) continue
    if (!best || ns.length > best[1].length || (ns.length === best[1].length && (name.length < best[0].length || (name.length === best[0].length && name < best[0])))) {
      best = [name, ns]
    }
  }
  return best && `${best[0]}:${iri.slice(best[1].length)}`
}

/** `s` as a double-quoted SPARQL string. */
function quote(s) {
  const escapes = { '"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t", "\b": "\\b", "\f": "\\f" }
  return `"${s.replace(/["\\\n\r\t\b\f]|[\u0000-\u001F\u007F]/g, (c) => escapes[c] ?? `\\u${c.charCodeAt(0).toString(16).toUpperCase().padStart(4, "0")}`)}"`
}

const isInteger = (s) => /^[+-]?\d+$/.test(s)
const isDecimal = (s) => /^[+-]?\d*\.\d+$/.test(s)

/** A literal as SPARQL writes it: strings quoted, integers, decimals and booleans bare, other datatypes by `datatype`. */
function literalText(t, datatype) {
  const quoted = quote(t.value)
  const lang = t["xml:lang"]
  if (lang) return t["its:dir"] ? `${quoted}@${lang}--${t["its:dir"]}` : `${quoted}@${lang}`
  const dt = t.datatype ?? `${XSD}string`
  const local = dt.startsWith(XSD) ? dt.slice(XSD.length) : ""
  if (local === "string") return quoted
  const bare = (local === "boolean" && (t.value === "true" || t.value === "false")) || (local === "integer" && isInteger(t.value)) || (local === "decimal" && isDecimal(t.value))
  return bare ? t.value : `${quoted}^^${datatype(dt)}`
}

/** `t` in SPARQL syntax, its IRIs (the datatypes' too) written by `iri`. */
function termText(t, iri) {
  switch (t.type) {
    case "uri":
      return iri(t.value)
    case "bnode":
      return `_:${t.value}`
    case "literal":
      return literalText(t, iri)
    case "triple":
      return `<<( ${termText(t.value.subject, iri)} ${termText(t.value.predicate, iri)} ${termText(t.value.object, iri)} )>>`
  }
}

/** `t` as shown: IRIs shortened where a prefix applies. */
const shortText = (t, prefixes) => termText(t, (i) => shorten(i, prefixes) ?? `<${i}>`)

/** `t` in SPARQL syntax with full IRIs (TSV). */
const fullText = (t) => termText(t, (i) => `<${i}>`)

/** `t` in SPARQL CSV: IRIs in full, literals as their lexical form, blank nodes as `_:label`. */
function csvText(t) {
  return t.type === "uri" ? t.value : t.type === "bnode" ? `_:${t.value}` : t.type === "literal" ? t.value : fullText(t)
}

/** @returns {Cell} */
function cellOf(t, prefixes) {
  if (!t) return { text: "" }
  const text = shortText(t, prefixes)
  return t.type === "uri" && text !== t.value ? { text, title: t.value } : { text }
}

const TRIPLE_COLUMNS = ["subject", "predicate", "object"]
const tripleRow = (t) => [t.subject, t.predicate, t.object]

/**
 * The answer as the panel shows it.
 * @param {Answer} answer
 * @param {Record<string, string>} prefixes
 * @returns {Table}
 */
export function tableOf(answer, prefixes) {
  switch (answer.kind) {
    case "select":
      return {
        kind: "table",
        columns: answer.vars,
        rows: answer.rows.map((r) => r.map((t) => cellOf(t, prefixes))),
        shown: answer.rows.length,
        total: answer.rows.length + answer.omitted,
      }
    case "construct":
      return {
        kind: "table",
        columns: TRIPLE_COLUMNS,
        rows: answer.triples.map((t) => tripleRow(t).map((x) => cellOf(x, prefixes))),
        shown: answer.triples.length,
        total: answer.triples.length + answer.omitted,
      }
    case "ask":
      return { kind: "boolean", value: answer.boolean }
    case "error":
      return { kind: "error", message: answer.message, at: answer.line === undefined ? undefined : `${answer.line}:${answer.column}` }
  }
}

const collator = new Intl.Collator("en", { numeric: true })
const NUMBER = /^[+-]?(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?$/

/** -1, 0, 1 for two non-empty cell texts: numbers by value and before the rest, the rest by text. */
function compareText(a, b) {
  const na = NUMBER.test(a)
  const nb = NUMBER.test(b)
  if (na && nb) return Math.sign(Number(a) - Number(b))
  if (na !== nb) return na ? -1 : 1
  return collator.compare(a, b)
}

/**
 * The rows' indices in the order `sort` shows them: unbound cells last
 * either way, ties in the answer's order.
 * @param {Table} table
 * @param {Sort | undefined} sort
 * @returns {number[]}
 */
export function order(table, sort) {
  if (table.kind !== "table") return []
  const indices = table.rows.map((_, i) => i)
  if (!sort || sort.column >= table.columns.length) return indices
  const sign = sort.descending ? -1 : 1
  return indices.sort((i, j) => {
    const a = table.rows[i][sort.column].text
    const b = table.rows[j][sort.column].text
    if (!a || !b) return a === b ? i - j : a ? -1 : 1
    return sign * compareText(a, b) || i - j
  })
}

/** The answer's rows (SELECT) or triples (CONSTRUCT) as term rows, in `ord` if given. */
function termRows(answer, ord) {
  const rows = answer.kind === "select" ? answer.rows : answer.kind === "construct" ? answer.triples.map(tripleRow) : []
  return ord ? ord.map((i) => rows[i]).filter(Boolean) : rows
}

/** `header` and a line per row of `rows`, cells by `cell` (unbound: empty) joined by `sep`, each line ending in `eol`. */
function delimited(header, rows, cell, sep, eol) {
  return [header, ...rows.map((r) => r.map((t) => (t ? cell(t) : "")))].map((l) => `${l.join(sep)}${eol}`).join("")
}

const csvField = (v) => (/[",\r\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v)

/**
 * SPARQL 1.1 CSV: a header of names, a line per row, CRLF; "" for an error.
 * @param {Answer} answer
 * @param {number[]} [ord]
 */
export function csvOf(answer, ord) {
  if (answer.kind === "ask") return `boolean\r\n${answer.boolean}\r\n`
  if (answer.kind === "error") return ""
  const columns = answer.kind === "select" ? answer.vars : TRIPLE_COLUMNS
  return delimited(columns.map(csvField), termRows(answer, ord), (t) => csvField(csvText(t)), ",", "\r\n")
}

/**
 * SPARQL 1.1 TSV for the clipboard: `?variables`, terms with full IRIs, LF.
 * @param {Answer} answer
 * @param {number[]} [ord]
 */
export function tsvOf(answer, ord) {
  if (answer.kind === "ask") return `${answer.boolean}\n`
  if (answer.kind === "error") return ""
  const header = answer.kind === "select" ? answer.vars.map((v) => `?${v}`) : TRIPLE_COLUMNS
  return delimited(header, termRows(answer, ord), fullText, "\t", "\n")
}

/**
 * SPARQL 1.1 Query Results JSON (SELECT, ASK); the triples for CONSTRUCT.
 * @param {Answer} answer
 * @param {number[]} [ord]
 */
export function jsonOf(answer, ord) {
  const rows = termRows(answer, ord)
  switch (answer.kind) {
    case "select": {
      const bindings = rows.map((r) => Object.fromEntries(answer.vars.flatMap((v, i) => (r[i] ? [[v, r[i]]] : []))))
      return JSON.stringify({ head: { vars: answer.vars }, results: { bindings } }, null, 2)
    }
    case "construct":
      return JSON.stringify({ triples: rows.map(([subject, predicate, object]) => ({ subject, predicate, object })) }, null, 2)
    case "ask":
      return JSON.stringify({ head: {}, boolean: answer.boolean }, null, 2)
    case "error":
      return ""
  }
}

const grouped = (n) => n.toLocaleString("en-US")
const counted = (n, what) => `${grouped(n)} ${what}${n === 1 ? "" : "s"}`

/** "34 ms", "2 s", "1.2 s", as the server writes a duration. */
function duration(ms) {
  if (ms < 1000) return `${ms} ms`
  return ms % 1000 === 0 ? `${ms / 1000} s` : `${(ms / 1000).toFixed(1)} s`
}

/**
 * "12 rows · 34 ms · 3 files", "1,000 of 12,345 rows · …", "true · …".
 * @param {Result} r
 */
export function headerLine(r) {
  const size = (shown, total, what) => (shown < total ? `${grouped(shown)} of ${counted(total, what)}` : counted(total, what))
  const a = r.answer
  const first =
    a.kind === "select" ? size(a.rows.length, a.rows.length + a.omitted, "row")
    : a.kind === "construct" ? size(a.triples.length, a.triples.length + a.omitted, "triple")
    : a.kind === "ask" ? String(a.boolean)
    : undefined
  return [first, duration(r.elapsedMs), counted(r.files, "file")].filter(Boolean).join(" · ")
}

/**
 * What reasoning added, in the server's words (`Inferred::sentence`).
 * @param {Inferred | undefined} inferred
 */
export function inferredNote(inferred) {
  if (!inferred) return undefined
  const count = (n, what) => `${n} ${what}${n === 1 ? "" : "s"}`
  if ("rules" in inferred) {
    const line = `Includes ${count(inferred.triples, "inferred triple")} (TurbOWL, ${inferred.rules} rules).`
    return inferred.findings > 0 ? `${line} TurbOWL reported ${count(inferred.findings, "finding")}.` : line
  }
  const s = inferred.skipped
  return "inferred" in s
    ? `Inferred triples skipped: TurbOWL inferred more than ${count(s.limit, "triple")}, over the reasoning limit.`
    : `Inferred triples skipped: the workspace has ${s.triples} triples, over the reasoning limit of ${s.limit}.`
}

const ESCAPES = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }
const esc = (s) => String(s).replace(/[&<>"']/g, (c) => ESCAPES[c])
const attr = (name, on) => (on ? ` ${name}` : "")

function tableHtml(table, sort) {
  const head = table.columns
    .map((c, i) => {
      const sorted = sort?.column === i
      const dir = sorted ? (sort.descending ? "descending" : "ascending") : undefined
      const next = sorted && !sort.descending ? "descending" : "ascending"
      return `<th scope="col"${dir ? ` aria-sort="${dir}"` : ""}><button type="button" data-sort="${i}" title="Sort by ${esc(c)}, ${next}">${esc(c)}<span class="arrow" aria-hidden="true">${dir === "ascending" ? "▲" : dir === "descending" ? "▼" : ""}</span></button></th>`
    })
    .join("")
  const body = order(table, sort)
    .map((i) => `<tr>${table.rows[i].map((c) => `<td${c.title ? ` title="${esc(c.title)}"` : ""}>${esc(c.text)}</td>`).join("")}</tr>`)
    .join("\n")
  const empty = table.rows.length === 0 ? `<p class="empty">No ${table.columns === TRIPLE_COLUMNS ? "triples" : "rows"}.</p>` : ""
  return `<table><thead><tr>${head}</tr></thead><tbody>\n${body}\n</tbody></table>${empty}`
}

function bodyHtml(table, sort) {
  switch (table.kind) {
    case "table":
      return tableHtml(table, sort)
    case "boolean":
      return `<p class="boolean" aria-label="ASK answer">${esc(table.value)}</p>`
    case "error":
      return `<p class="error" role="alert">${esc(table.at ? `Syntax error at ${table.at}: ${table.message}` : table.message)}</p>`
  }
}

/**
 * The panel's page for `view`, with the table sorted by `sort`.
 * @param {View} view
 * @param {Sort} [sort]
 */
export function render(view, sort) {
  const r = view.result
  const table = r && tableOf(r.answer, r.prefixes)
  const running = view.phase === "running"
  const exportable = !running && view.phase === "done" && table?.kind !== "error"
  // The run's reasoning until its result comes back, then the result's.
  const reasoning = (view.phase !== "done" && view.reasoning) || r?.reasoning || "off"
  const status =
    view.phase === "running" ? "Running…"
    : view.phase === "cancelled" ? "Cancelled."
    : view.phase === "failed" ? "The query did not run."
    : headerLine(view.result)
  const options = REASONINGS.map((k) => `<option value="${k}"${attr("selected", k === reasoning)}>${REASONING_LABELS[k]}</option>`).join("")
  const note = view.phase === "done" ? inferredNote(r.inferred) : undefined
  const message = view.phase === "failed" ? `<p class="error" role="alert">${esc(view.message)}</p>` : ""
  const body = table ? `<div class="results${view.phase === "done" ? "" : " stale"}">${bodyHtml(table, sort)}</div>` : ""
  return `<header class="bar">
<p class="summary${running ? " running" : ""}" role="status" aria-live="polite">${esc(status)}</p>
<div class="controls">
<label for="reasoning">Inferred</label><select id="reasoning" aria-label="Inferred triples"${attr("disabled", running)}>${options}</select>
<button type="button" data-action="rerun"${attr("disabled", running)}>Rerun</button>
<span class="sep" aria-hidden="true"></span>
<button type="button" data-action="copy"${attr("disabled", !exportable)}>Copy</button>
<button type="button" data-action="export-csv"${attr("disabled", !exportable)}>Export CSV</button>
<button type="button" data-action="export-json"${attr("disabled", !exportable)}>Export JSON</button>
</div>
</header>
${note ? `<p class="note">${esc(note)}</p>\n` : ""}${message}${body}`
}

/** The webview: draws what the extension sends, sorts locally, posts the actions back. */
function mount() {
  const api = acquireVsCodeApi()
  const root = document.getElementById("results")
  /** @type {View} */
  let view = { phase: "running" }
  /** @type {Sort | undefined} */
  let sort = api.getState()?.sort
  const currentOrder = () => (view.result && view.result.answer.kind !== "ask" ? order(tableOf(view.result.answer, view.result.prefixes), sort) : undefined)

  function draw() {
    const focused = document.activeElement
    const key = focused?.dataset?.sort !== undefined ? `[data-sort="${focused.dataset.sort}"]` : focused?.dataset?.action ? `[data-action="${focused.dataset.action}"]` : focused?.id ? `#${focused.id}` : undefined
    root.innerHTML = render(view, sort)
    if (key) root.querySelector(key)?.focus()
    api.postMessage({ type: "drawn", status: root.querySelector(".summary")?.textContent ?? "" })
  }

  root.addEventListener("click", (e) => {
    const button = e.target.closest("button")
    if (!button || button.disabled) return
    if (button.dataset.sort !== undefined) {
      const column = Number(button.dataset.sort)
      sort = sort?.column === column ? (sort.descending ? undefined : { column, descending: true }) : { column, descending: false }
      api.setState({ sort })
      draw()
      return
    }
    const action = button.dataset.action
    if (action === "rerun") api.postMessage({ type: "rerun", reasoning: root.querySelector("#reasoning").value })
    else if (action === "copy") api.postMessage({ type: "copy", order: currentOrder() })
    else if (action === "export-csv" || action === "export-json") api.postMessage({ type: "export", format: action.slice(7), order: currentOrder() })
  })
  root.addEventListener("change", (e) => {
    if (e.target.id === "reasoning") api.postMessage({ type: "rerun", reasoning: e.target.value })
  })
  window.addEventListener("message", (e) => {
    if (e.data?.type !== "show") return
    view = e.data.view
    draw()
  })
  draw()
  api.postMessage({ type: "ready" })
}

if (typeof acquireVsCodeApi === "function") mount()
