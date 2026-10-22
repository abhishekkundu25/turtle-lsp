// Formatting SPARQL documents over stdio: the golden fixtures (a query and an
// update script), no edits when already formatted, refusal on syntax errors,
// the client's indent options and the line width setting.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures } from "./lsp-client.mjs"
import { apply } from "./query-client.mjs"

const dir = path.join(fixtures, "format")
const read = (f) => fs.readFileSync(path.join(dir, f), "utf8")
const spaces = { tabSize: 4, insertSpaces: true }
const comments = (text) => text.split("\n").filter((l) => /(^|\s)#/.test(l) && !/"[^"]*#/.test(l)).map((l) => l.slice(l.indexOf("#")))

let s
before(async () => {
  s = await startServer({ cwd: dir, initializationOptions: { format: { lineWidth: 100 } } })
})
after(async () => {
  await s.stop()
})

let n = 0
async function format(text, options = spaces, ext = "rq", server = s) {
  const uri = uriOf(path.join(dir, `__sparql${n++}.${ext}`))
  server.open(uri, text, "sparql")
  await server.diagnosticsFor(uri)
  try {
    return await server.request("textDocument/formatting", { textDocument: { uri }, options })
  } finally {
    server.close(uri)
  }
}

for (const ext of ["rq", "ru"]) {
  test(`formats the messy .${ext} fixture into the golden file`, async () => {
    const text = read(`messy.${ext}`)
    const edits = await format(text, spaces, ext)
    assert.equal(edits.length, 1)
    assert.equal(apply(text, edits), read(`expected.${ext}`))
  })

  test(`the golden .${ext} file is already formatted`, async () => {
    assert.deepEqual(await format(read(`expected.${ext}`), spaces, ext), [])
  })

  test(`formatting the .${ext} fixture keeps every comment`, async () => {
    const text = read(`messy.${ext}`)
    assert.deepEqual(comments(apply(text, await format(text, spaces, ext))), comments(text))
  })
}

test("a query with a syntax error gets no edits", async () => {
  assert.deepEqual(await format("SELECT ?x WHERE { ?x ?p }\n"), [])
})

test("a query with an undeclared prefix gets no edits", async () => {
  assert.deepEqual(await format("select * where { ?s foaf:name ?n }\n"), [])
})

test("uses tabs when the client asks for them", async () => {
  const text = "PREFIX ex: <https://e.org/>\nSELECT * WHERE { ?s ex:p ?o ; ex:q ?r . OPTIONAL { ?s ex:t ?u } }\n"
  const out = apply(text, await format(text, { tabSize: 8, insertSpaces: false }))
  assert.equal(
    out,
    "PREFIX ex: <https://e.org/>\n\nSELECT *\nWHERE {\n\t?s ex:p ?o ;\n\t\tex:q ?r .\n\tOPTIONAL {\n\t\t?s ex:t ?u\n\t}\n}\n"
  )
})

test("an empty group, ASK, CONSTRUCT and DESCRIBE; no keyword is added or removed", async () => {
  const cases = [
    ["ask{}", "ASK { }\n"],
    ["select * {?s ?p ?o}", "SELECT * {\n    ?s ?p ?o\n}\n"],
    ["ask where {?s ?p ?o}", "ASK\nWHERE {\n    ?s ?p ?o\n}\n"],
    ["construct{?s ?p ?o}where{?s ?p ?o}", "CONSTRUCT {\n    ?s ?p ?o\n}\nWHERE {\n    ?s ?p ?o\n}\n"],
    ["describe <http://e.org/a>", "DESCRIBE <http://e.org/a>\n"],
    ["construct where{?s ?p ?o}", "CONSTRUCT\nWHERE {\n    ?s ?p ?o\n}\n"],
  ]
  for (const [text, want] of cases) assert.equal(apply(text, await format(text)), want, text)
})

test("a narrow line width breaks a long projection and object list, and nothing else gets longer", async () => {
  const narrow = await startServer({ cwd: dir, initializationOptions: { format: { lineWidth: 40 } } })
  try {
    const text =
      "PREFIX ex: <https://e.org/>\nSELECT ?alpha ?bravo ?charlie ?delta ?echo ?foxtrot WHERE { ?alpha ex:p ?bravo , ?charlie , ?delta , ?echo , ?foxtrot }\n"
    const out = apply(text, await format(text, spaces, "rq", narrow))
    for (const line of out.split("\n")) assert.ok(line.length <= 40, `too long: ${line}\n${out}`)
    // The same query at the default width keeps both on one line.
    const wide = apply(text, await format(text))
    assert.ok(wide.includes("SELECT ?alpha ?bravo ?charlie ?delta ?echo ?foxtrot\n"), wide)
    assert.ok(wide.includes("?alpha ex:p ?bravo , ?charlie , ?delta , ?echo , ?foxtrot\n"), wide)
    // And formatting the narrow output again changes nothing.
    assert.deepEqual(await format(out, spaces, "rq", narrow), [])
  } finally {
    await narrow.stop()
  }
})

// How long this takes: latency.test.mjs.
test("a long query is formatted: one pattern per line", async () => {
  const body = Array.from({ length: 2000 }, (_, i) => `?s${i} <https://e.org/p> ?o${i} .`).join(" ")
  const text = `SELECT * WHERE { ${body} }\n`
  const edits = await format(text)
  assert.equal(edits.length, 1)
  const out = apply(text, edits)
  assert.equal(out.split("\n").filter((l) => l.includes("<https://e.org/p>")).length, 2000)
})
