// The Query Results panel's pure half (src/query-results-view.js), shared by
// the webview and the extension: the server's answer read at the edge, the
// table model of each answer kind, sorting, IRIs shortened with the query's
// prefixes, the header line and the inferred note, and the exports.
import { test } from "node:test"
import assert from "node:assert/strict"
import {
  csvOf,
  headerLine,
  inferredNote,
  jsonOf,
  order,
  parseAnswer,
  parseMessage,
  render,
  shorten,
  tableOf,
  tsvOf,
} from "../src/query-results-view.js"

const EX = "http://example.org/"
const XSD = "http://www.w3.org/2001/XMLSchema#"
const prefixes = { ex: EX, xsd: XSD }
const iri = (v) => ({ type: "uri", value: v })
const lit = (v, extra = {}) => ({ type: "literal", value: v, ...extra })

const select = {
  kind: "select",
  head: { vars: ["s", "n", "label"] },
  results: {
    bindings: [
      { s: iri(`${EX}b`), n: lit("10", { datatype: `${XSD}integer` }), label: lit("Bee", { "xml:lang": "en" }) },
      { s: iri(`${EX}a`), n: lit("9", { datatype: `${XSD}integer` }) },
      { s: { type: "bnode", value: "x1" }, n: lit("2.5", { datatype: `${XSD}decimal` }), label: lit('say "hi", ok') },
    ],
  },
  omitted: 0,
}

const result = (answer, extra = {}) => ({
  uri: "file:///w/q.rq",
  name: "q.rq",
  files: 3,
  elapsedMs: 34,
  summary: "3 rows in 34 ms",
  reasoning: "off",
  prefixes,
  answer,
  ...extra,
})

test("a result object is read at the edge; a string or a stranger is not one", () => {
  const read = parseAnswer(result(select))
  assert.equal(read.kind, "result")
  assert.equal(read.answer.kind, "select")
  assert.deepEqual(read.answer.vars, ["s", "n", "label"])
  assert.equal(read.answer.rows.length, 3)
  assert.equal(parseAnswer("3 rows in 34 ms"), undefined)
  assert.equal(parseAnswer(undefined), undefined)
  assert.equal(parseAnswer({ uri: "file:///w/q.rq" }), undefined)
  assert.deepEqual(parseAnswer({ cancelled: true, uri: "file:///w/q.rq" }), { kind: "cancelled", uri: "file:///w/q.rq" })
  // A bad reasoning value or an unknown answer kind does not get inside.
  assert.equal(parseAnswer(result(select, { reasoning: "maybe" })).reasoning, "off")
  const odd = parseAnswer(result({ kind: "chart" }))
  assert.equal(odd.answer.kind, "error")
  assert.match(odd.answer.message, /could not be read/)
})

test("SELECT is a table: one column per variable, IRIs shortened, the full IRI in the title", () => {
  const t = tableOf(parseAnswer(result(select)).answer, prefixes)
  assert.equal(t.kind, "table")
  assert.deepEqual(t.columns, ["s", "n", "label"])
  assert.deepEqual(t.rows[0][0], { text: "ex:b", title: `${EX}b` })
  assert.equal(t.rows[0][1].text, "10")
  assert.equal(t.rows[0][2].text, '"Bee"@en')
  assert.deepEqual(t.rows[1][2], { text: "" })
  assert.equal(t.rows[2][0].text, "_:x1")
  assert.equal(t.rows[2][2].text, '"say \\"hi\\", ok"')
  assert.equal(t.shown, 3)
  assert.equal(t.total, 3)
})

test("a typed literal shows its datatype shortened; a string is quoted", () => {
  const answer = parseAnswer(
    result({
      kind: "select",
      head: { vars: ["d"] },
      results: { bindings: [{ d: lit("2024-01-01", { datatype: `${XSD}date` }) }, { d: lit("true", { datatype: `${XSD}boolean` }) }] },
      omitted: 0,
    }),
  ).answer
  const t = tableOf(answer, prefixes)
  assert.equal(t.rows[0][0].text, '"2024-01-01"^^xsd:date')
  assert.equal(t.rows[1][0].text, "true")
})

test("ASK is true or false; CONSTRUCT is its triples; an error keeps its line and column", () => {
  assert.deepEqual(tableOf(parseAnswer(result({ kind: "ask", head: {}, boolean: true })).answer, prefixes), { kind: "boolean", value: true })
  const triples = parseAnswer(
    result({ kind: "construct", triples: [{ subject: iri(`${EX}a`), predicate: iri(`${EX}p`), object: lit("o") }], omitted: 4 }),
  ).answer
  const t = tableOf(triples, prefixes)
  assert.deepEqual(t.columns, ["subject", "predicate", "object"])
  assert.deepEqual(
    t.rows[0].map((c) => c.text),
    ["ex:a", "ex:p", '"o"'],
  )
  assert.equal(t.shown, 1)
  assert.equal(t.total, 5)
  const error = parseAnswer(result({ kind: "error", message: "expected WHERE", line: 3, column: 7 })).answer
  assert.deepEqual(tableOf(error, prefixes), { kind: "error", message: "expected WHERE", at: "3:7" })
  assert.deepEqual(tableOf(parseAnswer(result({ kind: "error", message: "stopped" })).answer, prefixes), {
    kind: "error",
    message: "stopped",
    at: undefined,
  })
})

test("sorting by a column: numbers by value, unbound last, then descending; ties keep their order", () => {
  const t = tableOf(parseAnswer(result(select)).answer, prefixes)
  assert.deepEqual(order(t, undefined), [0, 1, 2])
  assert.deepEqual(order(t, { column: 1, descending: false }), [2, 1, 0])
  assert.deepEqual(order(t, { column: 1, descending: true }), [0, 1, 2])
  assert.deepEqual(order(t, { column: 2, descending: false }), [0, 2, 1])
  assert.deepEqual(order(t, { column: 2, descending: true }), [2, 0, 1])
  assert.deepEqual(order(t, { column: 0, descending: false }), [2, 1, 0])
})

test("IRI shortening takes the longest namespace whose rest is a local name", () => {
  const p = { ex: EX, exd: `${EX}def/`, x: "http://x/" }
  assert.equal(shorten(`${EX}def/Thing`, p), "exd:Thing")
  assert.equal(shorten(`${EX}a.b`, p), "ex:a.b")
  assert.equal(shorten(`${EX}`, p), "ex:")
  assert.equal(shorten(`${EX}a b`, p), undefined)
  assert.equal(shorten(`${EX}ends.`, p), undefined)
  assert.equal(shorten(`${EX}-start`, p), undefined)
  assert.equal(shorten("http://other/a", p), undefined)
})

test("CSV is SPARQL CSV: full IRIs, lexical forms, RFC 4180 quoting, CRLF, in the shown order", () => {
  const answer = parseAnswer(result(select)).answer
  assert.equal(
    csvOf(answer, undefined),
    `s,n,label\r\n${EX}b,10,Bee\r\n${EX}a,9,\r\n_:x1,2.5,"say ""hi"", ok"\r\n`,
  )
  const t = tableOf(answer, prefixes)
  assert.ok(csvOf(answer, order(t, { column: 1, descending: false })).startsWith(`s,n,label\r\n_:x1,`))
  assert.equal(csvOf(parseAnswer(result({ kind: "ask", head: {}, boolean: false })).answer), "boolean\r\nfalse\r\n")
})

test("TSV for the clipboard: ?variables, terms in SPARQL syntax with full IRIs", () => {
  assert.equal(
    tsvOf(parseAnswer(result(select)).answer),
    `?s\t?n\t?label\n<${EX}b>\t10\t"Bee"@en\n<${EX}a>\t9\t\n_:x1\t2.5\t"say \\"hi\\", ok"\n`,
  )
})

test("JSON export is SPARQL 1.1 Query Results JSON", () => {
  const answer = parseAnswer(result(select)).answer
  const json = JSON.parse(jsonOf(answer, [1, 0, 2]))
  assert.deepEqual(json.head, { vars: ["s", "n", "label"] })
  assert.deepEqual(json.results.bindings[0], select.results.bindings[1])
  assert.deepEqual(JSON.parse(jsonOf(parseAnswer(result({ kind: "ask", head: {}, boolean: true })).answer)), { head: {}, boolean: true })
})

test("the header: rows, time, files; N of M rows when some were left out", () => {
  assert.equal(headerLine(parseAnswer(result(select))), "3 rows · 34 ms · 3 files")
  const cut = { ...select, omitted: 12342 }
  assert.equal(headerLine(parseAnswer(result(cut, { files: 1, elapsedMs: 1234 }))), "3 of 12,345 rows · 1.2 s · 1 file")
  assert.equal(headerLine(parseAnswer(result({ kind: "ask", head: {}, boolean: true }, { elapsedMs: 2000 }))), "true · 2 s · 3 files")
  assert.equal(
    headerLine(parseAnswer(result({ kind: "construct", triples: [{ subject: iri(EX), predicate: iri(EX), object: iri(EX) }], omitted: 0 }))),
    "1 triple · 34 ms · 3 files",
  )
  assert.equal(headerLine(parseAnswer(result({ kind: "error", message: "x" }))), "34 ms · 3 files")
})

test("the inferred note says what the server says", () => {
  assert.equal(inferredNote(undefined), undefined)
  assert.equal(inferredNote({ rules: "default", triples: 12, findings: 0 }), "Includes 12 inferred triples (TurbOWL, default rules).")
  assert.equal(
    inferredNote({ rules: "complete", triples: 1, findings: 2 }),
    "Includes 1 inferred triple (TurbOWL, complete rules). TurbOWL reported 2 findings.",
  )
  assert.equal(
    inferredNote({ skipped: { triples: 600000, limit: 500000 } }),
    "Inferred triples skipped: the workspace has 600000 triples, over the reasoning limit of 500000.",
  )
  assert.equal(
    inferredNote({ skipped: { inferred: true, limit: 1 } }),
    "Inferred triples skipped: TurbOWL inferred more than 1 triple, over the reasoning limit.",
  )
  const read = parseAnswer(result(select, { reasoning: "default", inferred: { rules: "default", triples: 2, findings: 0 } }))
  assert.equal(inferredNote(read.inferred), "Includes 2 inferred triples (TurbOWL, default rules).")
})

test("the webview's messages are read at the edge", () => {
  assert.deepEqual(parseMessage({ type: "rerun", reasoning: "complete" }), { type: "rerun", reasoning: "complete" })
  assert.equal(parseMessage({ type: "rerun", reasoning: "all" }), undefined)
  assert.deepEqual(parseMessage({ type: "copy", order: [1, 0] }), { type: "copy", order: [1, 0] })
  assert.deepEqual(parseMessage({ type: "copy" }), { type: "copy", order: undefined })
  assert.equal(parseMessage({ type: "copy", order: [1, -1] }), undefined)
  assert.deepEqual(parseMessage({ type: "export", format: "csv" }), { type: "export", format: "csv", order: undefined })
  assert.equal(parseMessage({ type: "export", format: "xml" }), undefined)
  assert.deepEqual(parseMessage({ type: "ready" }), { type: "ready" })
  assert.deepEqual(parseMessage({ type: "drawn", status: "3 rows" }), { type: "drawn", status: "3 rows" })
  assert.equal(parseMessage(null), undefined)
  assert.equal(parseMessage({ type: "rm -rf" }), undefined)
})

test("the page: header, note, controls with names, an escaped sortable table", () => {
  const read = parseAnswer(
    result(
      {
        ...select,
        results: { bindings: [{ s: iri(`${EX}<b>`), n: lit("<script>") }] },
      },
      { reasoning: "default", inferred: { rules: "default", triples: 2, findings: 0 } },
    ),
  )
  const html = render({ phase: "done", result: read }, { column: 0, descending: true })
  assert.match(html, /1 row · 34 ms · 3 files/)
  assert.match(html, /Includes 2 inferred triples/)
  assert.match(html, /<select[^>]*id="reasoning"[^>]*aria-label="Inferred triples"/)
  assert.match(html, /<option value="default" selected>Default rules<\/option>/)
  assert.match(html, /<button[^>]*data-action="rerun"[^>]*>Rerun<\/button>/)
  assert.match(html, /data-action="copy"[^>]*>Copy<\/button>/)
  assert.match(html, /data-action="export-csv"[^>]*>Export CSV<\/button>/)
  assert.match(html, /data-action="export-json"[^>]*>Export JSON<\/button>/)
  assert.match(html, /<th[^>]*aria-sort="descending"[^>]*><button[^>]*data-sort="0"/)
  assert.ok(!html.includes("<script>"), "a literal is not escaped")
  assert.ok(html.includes("&lt;script&gt;"))
  assert.ok(!html.includes(`title="${EX}<b>"`))
})

test("the page while running, cancelled, for ASK, for an error and for a cut table", () => {
  const read = parseAnswer(result(select))
  assert.match(render({ phase: "running", result: read }), /Running…/)
  assert.match(render({ phase: "running", result: undefined }), /Running…/)
  assert.match(render({ phase: "cancelled", result: read }), /Cancelled/)
  assert.match(render({ phase: "done", result: parseAnswer(result({ kind: "ask", head: {}, boolean: false })) }), /class="boolean"[^>]*>false</)
  const error = render({ phase: "done", result: parseAnswer(result({ kind: "error", message: "expected WHERE", line: 3, column: 7 })) })
  assert.match(error, /role="alert"/)
  assert.match(error, /Syntax error at 3:7: expected WHERE/)
  assert.match(error, /data-action="copy"[^>]*disabled/)
  const cut = render({ phase: "done", result: parseAnswer(result({ ...select, omitted: 997 })) })
  assert.match(cut, /3 of 1,000 rows/)
})

test("the Inferred choice keeps the run's reasoning when no result came back; a result's wins once it does", () => {
  const selected = (html) => html.match(/<option value="(\w+)" selected>/)?.[1]
  const read = parseAnswer(result(select))
  assert.equal(selected(render({ phase: "running", reasoning: "complete" })), "complete")
  assert.equal(selected(render({ phase: "cancelled", reasoning: "complete" })), "complete")
  assert.equal(selected(render({ phase: "failed", message: "no", reasoning: "default" })), "default")
  assert.equal(selected(render({ phase: "cancelled" })), "off")
  assert.equal(selected(render({ phase: "done", result: { ...read, reasoning: "default" } })), "default")
})
