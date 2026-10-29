// Edits sent as ranged changes give the same published diagnostics as
// reopening the document on the final text, in each syntax: Turtle, TriG and
// N-Quads take the statement splice where it applies; SPARQL has its own
// parse and JSON-LD always parses in full.
//
// Each sequence runs twice. A plain server shows what a user gets. A server
// with TURTLE_LSP_CHECK_INCREMENTAL=1 parses each edited document both ways
// and logs how each analysis was parsed (`incremental: <uri> Spliced`,
// `... Fallback(Directive)`) and any splice that differed from a full parse.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures } from "./lsp-client.mjs"

const CHECK = "TURTLE_LSP_CHECK_INCREMENTAL"

// The LSP position (UTF-16, as JavaScript strings count) of `offset` in `text`.
function position(text, offset) {
  const before = text.slice(0, offset).split(/\r\n|\r|\n/)
  return { line: before.length - 1, character: before[before.length - 1].length }
}

// Replace the first `find` in `text` (after `after`, when given) with `put`:
// the ranged change and the new text.
function replace(text, find, put, after = "") {
  const from = after ? text.indexOf(after) : 0
  const at = text.indexOf(find, from)
  assert.ok(from >= 0 && at >= 0, `${JSON.stringify(find)} not found`)
  const range = { start: position(text, at), end: position(text, at + find.length) }
  return { change: { range, text: put }, text: text.slice(0, at) + put + text.slice(at + find.length) }
}

// Each case: a fixture, its language id, extra statements appended first (so
// later edits are past the directives the splice falls back on), then
// replacements, each [find, put, after?]; `splices` when the splice applies.
const cases = [
  {
    name: "Turtle",
    file: "workspace/people.ttl",
    lang: "turtle",
    append: "\nex:carol a foaf:Person ;\n    foaf:name \"Carol\" .\n\nex:dan a foaf:Person ;\n    foaf:name \"Dan\" ;\n    foaf:knows ex:carol .\n\nex:erin foaf:name \"Erin\" .\n",
    edits: [
      ["\"Dan\"", "\"Daniel\""],
      [" ;\n    foaf:knows ex:carol", " ;\n    foaf:knows ex:carol ,", "ex:dan"],
      ["ex:carol ,", "ex:carol , ex:unknownTerm"],
      ["foaf:name \"Erin\" .", "foaf:name \"Erin\""],
      ["foaf:name \"Erin\"", "foaf:name \"Erin\" ."],
      ["\"Carol\"", "\"Car\\qol\""],
    ],
    splices: true,
  },
  {
    name: "TriG",
    file: "datasets/graphs.trig",
    lang: "trig",
    append: "\ng:more {\n    p:erin a dx:Person ;\n        dx:name \"Erin\" .\n\n    p:fay a dx:Person ;\n        dx:name \"Fay\" .\n\n    p:gus a dx:Person .\n}\n",
    edits: [
      ["\"Fay\"", "\"Fay Wray\""],
      ["p:gus a dx:Person .", "p:gus a dx:Person ; dx:name 12 ."],
      ["dx:name \"Erin\" .", "dx:name \"Erin\""],
      ["dx:name \"Erin\"", "dx:name \"Erin\" ."],
    ],
    splices: true,
  },
  {
    name: "N-Quads",
    file: "query/data.nq",
    lang: "nquads",
    append: "",
    edits: [
      ["\"Douglas Adams\"", "\"Douglas Noel Adams\""],
      ["<https://query.example.org/id/Q42>", "<https://query.example.org/id/Q 42>"],
      ["\"Alice\" <https://query.example.org/graph/people> .", "\"Alice\" <https://query.example.org/graph/people>"],
      ["\"Alice\" <https://query.example.org/graph/people>", "\"Alice\" <https://query.example.org/graph/people> ."],
    ],
    splices: true,
  },
  {
    name: "SPARQL",
    file: "query/people.rq",
    lang: "sparql",
    append: "",
    edits: [
      ["?orgLabel", "?label"],
      ["LIMIT 10", "LIMIT"],
      ["ex:name ?name .", "ex:name ?name"],
    ],
    splices: false,
  },
  {
    name: "JSON-LD",
    file: "chaos/scoped-context.jsonld",
    lang: "json",
    append: "",
    edits: [
      ["\"scoped\"", "\"scoped more\""],
      ["\"in g1\"", "\"in g1\","],
      ["\"in g1\",", "\"in g1\""],
    ],
    splices: false,
  },
]

// Opens the case's text, applies its edits one change at a time, waiting
// for the diagnostics of each, and returns the last ones with the text.
async function edited(s, uri, c) {
  let text = fs.readFileSync(path.join(fixtures, c.file), "utf8")
  s.open(uri, text, c.lang)
  await s.diagnosticsFor(uri)
  const steps = c.edits.map(([find, put, after]) => (t) => replace(t, find, put, after))
  if (c.append) steps.unshift((t) => appendChange(t, c.append))
  for (const step of steps) {
    const r = step(text)
    s.changeRange(uri, [r.change])
    text = r.text
    await s.diagnosticsFor(uri)
  }
  return { text, diagnostics: await s.diagnosticsFor(uri) }
}

// `extra` added at the end of `text`.
function appendChange(text, extra) {
  const end = position(text, text.length)
  return { change: { range: { start: end, end }, text: extra }, text: text + extra }
}

// The diagnostics of `text` opened afresh as `uri`: closed first, after
// the empty report that clears the closed document's.
async function reopened(s, uri, text, lang) {
  const before = s.publications.length
  s.close(uri)
  while (!s.publications.slice(before).some((p) => p.uri === uri && p.diagnostics.length === 0)) {
    await new Promise((r) => setTimeout(r, 10))
  }
  s.open(uri, text, lang)
  return s.diagnosticsFor(uri)
}

for (const c of cases) {
  test(`${c.name}: edits give the diagnostics of a reopen`, async () => {
    const ws = tempDir("turtle-incremental-")
    const uri = uriOf(path.join(ws, path.basename(c.file)))
    const s = await startServer({ cwd: ws })
    try {
      const { text, diagnostics } = await edited(s, uri, c)
      assert.ok(diagnostics.length > 0, "the edits leave something to report")
      assert.deepEqual(diagnostics, await reopened(s, uri, text, c.lang))
    } finally {
      await s.stop()
      fs.rmSync(ws, { recursive: true, force: true })
    }
  })

  test(`${c.name}: parsed both ways, the splice matches a full parse`, async () => {
    const ws = tempDir("turtle-incremental-")
    const uri = uriOf(path.join(ws, path.basename(c.file)))
    process.env[CHECK] = "1"
    const s = await startServer({ cwd: ws })
    delete process.env[CHECK]
    try {
      await edited(s, uri, c)
      // Answered after the log of the last analysis, which follows its
      // diagnostics.
      await s.request("workspace/symbol", { query: "" })
      const paths = s.logs.filter((m) => m.startsWith(`incremental: ${uri} `)).map((m) => m.slice(`incremental: ${uri} `.length))
      assert.deepEqual(paths.filter((p) => p.startsWith("differs")), [])
      if (c.lang === "sparql") {
        assert.deepEqual(paths, [], "a SPARQL document has no RDF analysis")
      } else {
        // One analysis on open, one per edit.
        assert.equal(paths.length, 1 + c.edits.length + (c.append ? 1 : 0), paths.join(", "))
        assert.equal(paths[0], "Whole")
      }
      if (c.splices) assert.ok(paths.includes("Spliced"), paths.join(", "))
      if (c.lang === "json") assert.ok(paths.slice(1).every((p) => p === "Fallback(Syntax)"), paths.join(", "))
    } finally {
      await s.stop()
      fs.rmSync(ws, { recursive: true, force: true })
    }
  })
}
