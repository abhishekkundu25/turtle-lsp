// "▶ Run query": the lens on SPARQL queries and the turtle.runQuery command,
// which runs the query over the workspace in the background, writes the
// results to <cache dir>/query-results/<stem>.md and opens them with
// window/showDocument (or shows the summary with window/showMessage).
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { startServer, uriOf, tempDir, positionOf } from "./lsp-client.mjs"

const DATA = `@prefix ex: <http://example.org/> .
ex:alice ex:name "Alice" ; ex:age 30 .
ex:bob ex:name "Bob" ; ex:age 25 .
ex:carol ex:name "Carol" ; ex:age 41 .
`
const QUERY = `# Everyone's name
PREFIX ex: <http://example.org/>

SELECT ?name WHERE { ?s ex:name ?name } ORDER BY ?name
`
const UPDATE = "PREFIX ex: <http://example.org/>\nINSERT DATA { ex:dave ex:name \"Dave\" }\n"
// A cross product of the generated data: far too slow to finish.
const SLOW = "SELECT (COUNT(*) AS ?n) WHERE { ?a ?p ?b . ?c ?q ?d . ?e ?r ?f . ?g ?h ?i }\n"

let ws
const at = (name) => uriOf(path.join(ws, name))
const run = (s, uri) => s.request("workspace/executeCommand", { command: "turtle.runQuery", arguments: [uri] })
const showDocuments = (s) => s.serverRequests.filter((r) => r.method === "window/showDocument")
const showDocument = { window: { showDocument: { support: true } } }
const onRequest = (method) => (method === "window/showDocument" ? { success: true } : null)

before(() => {
  ws = tempDir("turtle-query-")
  fs.writeFileSync(path.join(ws, "people.ttl"), DATA)
  fs.writeFileSync(path.join(ws, "names.rq"), QUERY)
  fs.writeFileSync(path.join(ws, "update.ru"), UPDATE)
  fs.writeFileSync(path.join(ws, "ask.rq"), "PREFIX ex: <http://example.org/>\nASK { ex:alice ex:age 30 }\n")
  fs.writeFileSync(path.join(ws, "broken.rq"), "PREFIX ex: <http://example.org/>\nSELECT ?s WHERE {\n  ?s ex:name\n}\n")
  fs.writeFileSync(path.join(ws, "slow.rq"), SLOW)
  let big = "@prefix ex: <http://example.org/> .\n"
  for (let i = 0; i < 400; i++) big += `ex:n${i} ex:next ex:n${i + 1} .\n`
  fs.writeFileSync(path.join(ws, "big.ttl"), big)
})
after(() => fs.rmSync(ws, { recursive: true, force: true }))

test("a query gets one Run lens at its form; an update gets none", async () => {
  const s = await startServer({ cwd: ws })
  try {
    assert.ok(s.init.capabilities.executeCommandProvider.commands.includes("turtle.runQuery"))
    s.open(at("names.rq"), QUERY, "sparql")
    s.open(at("update.ru"), UPDATE, "sparql")
    const ls = await s.request("textDocument/codeLens", { textDocument: { uri: at("names.rq") } })
    assert.equal(ls.length, 1, JSON.stringify(ls))
    assert.deepEqual(ls[0].range, { start: { line: 3, character: 0 }, end: { line: 3, character: 6 } })
    assert.deepEqual(ls[0].command, { title: "▶ Run query", command: "turtle.runQuery", arguments: [at("names.rq")] })
    assert.deepEqual(await s.request("textDocument/codeLens", { textDocument: { uri: at("update.ru") } }), [])
  } finally {
    await s.stop()
  }
})

test("running writes the results file and opens it beside the query", async () => {
  const s = await startServer({ cwd: ws, capabilities: showDocument, onRequest })
  try {
    s.open(at("names.rq"), QUERY, "sparql")
    const summary = await run(s, at("names.rq"))
    assert.match(summary, /^3 rows in \d+ ms$/)
    const file = path.join(s.cacheDir, "query-results", "names.md")
    const md = fs.readFileSync(file, "utf8")
    assert.match(md, /^# names\.rq\n\nRan in \d+ ms over \d+ files\.\n\n3 rows\n\n\| \?name \|\n\|---\|\n/)
    assert.ok(md.includes('| `"Alice"` |\n| `"Bob"` |\n| `"Carol"` |\n'), md)
    const shown = showDocuments(s)
    assert.equal(shown.length, 1)
    assert.equal(fileURLToPath(shown[0].params.uri), fs.realpathSync(file))
    assert.equal(shown[0].params.takeFocus, false)
    assert.deepEqual(s.messages, [])

    // A query file that is not open runs from disk; ASK gives a boolean.
    assert.match(await run(s, at("ask.rq")), /^true in \d+ ms$/)
    assert.ok(fs.readFileSync(path.join(s.cacheDir, "query-results", "ask.md"), "utf8").includes("**true**"))

    // A syntax error is reported with its position, in the file and the answer.
    const err = await run(s, at("broken.rq"))
    assert.match(err, /^syntax error at 4:1: /)
    assert.match(fs.readFileSync(path.join(s.cacheDir, "query-results", "broken.md"), "utf8"), /## Error\n\nSyntax error at 4:1: /)
    // An update is not run.
    assert.match(await run(s, at("update.ru")), /UPDATE is not supported/)

    // Not a SPARQL document.
    await assert.rejects(run(s, at("people.ttl")), /not a SPARQL document/)
  } finally {
    await s.stop()
  }
})

test("query files of the same name in different folders keep their own results", async () => {
  for (const d of ["a", "b"]) fs.mkdirSync(path.join(ws, d), { recursive: true })
  fs.writeFileSync(path.join(ws, "a", "same.rq"), QUERY)
  fs.writeFileSync(path.join(ws, "b", "same.rq"), "PREFIX ex: <http://example.org/>\nASK { ex:alice ex:age 30 }\n")
  const s = await startServer({ cwd: ws, capabilities: showDocument, onRequest })
  try {
    const results = (name) => fs.readFileSync(path.join(s.cacheDir, "query-results", name), "utf8")
    assert.match(await run(s, at("a/same.rq")), /^3 rows/)
    assert.match(await run(s, at("b/same.rq")), /^true/)
    assert.match(await run(s, at("a/same.rq")), /^3 rows/)
    assert.ok(results("same.md").includes('`"Alice"`'), results("same.md"))
    assert.ok(results("same-2.md").includes("**true**"), results("same-2.md"))
    const shown = showDocuments(s).map((r) => path.basename(fileURLToPath(r.params.uri)))
    assert.deepEqual(shown, ["same.md", "same-2.md", "same.md"])
  } finally {
    await s.stop()
  }
})

test("the query sees unsaved edits to the data and to itself", async () => {
  const s = await startServer({ cwd: ws, capabilities: showDocument, onRequest })
  try {
    s.open(at("people.ttl"), DATA)
    await s.diagnosticsFor(at("people.ttl"))
    s.change(at("people.ttl"), DATA + 'ex:dave ex:name "Dave" .\n')
    s.open(at("names.rq"), QUERY, "sparql")
    assert.match(await run(s, at("names.rq")), /^4 rows/)
    const file = path.join(s.cacheDir, "query-results", "names.md")
    assert.ok(fs.readFileSync(file, "utf8").includes('`"Dave"`'))
    s.change(at("names.rq"), QUERY.replace("?s ex:name ?name", '?s ex:name ?name FILTER(?name != "Bob")'))
    assert.match(await run(s, at("names.rq")), /^3 rows/)
    assert.ok(!fs.readFileSync(file, "utf8").includes('`"Bob"`'))
  } finally {
    await s.stop()
  }
})

test("without showDocument support the summary is a message", async () => {
  const s = await startServer({ cwd: ws, initializationOptions: { query: { maxRows: 2 } } })
  try {
    const summary = await run(s, at("names.rq"))
    assert.match(summary, /^3 rows in \d+ ms$/)
    assert.equal(showDocuments(s).length, 0)
    assert.equal(s.messages.length, 1, JSON.stringify(s.messages))
    assert.equal(s.messages[0].type, 3)
    assert.equal(s.messages[0].message, `names.rq: ${summary}`)
    // query.maxRows cuts the table and keeps the total.
    const md = fs.readFileSync(path.join(s.cacheDir, "query-results", "names.md"), "utf8")
    assert.ok(md.includes("3 rows\n") && md.includes("_Showing 2 of 3 rows._"), md)
    assert.ok(!md.includes('`"Carol"`'), md)
  } finally {
    await s.stop()
  }
})

// How soon a query stops and a hover is answered while it runs is a
// latency, checked in latency.test.mjs; here, the order of the answers.
test("a slow query stops at the time limit, and typing is answered meanwhile", async () => {
  const s = await startServer({ cwd: ws, initializationOptions: { query: { timeoutSeconds: 2 } } })
  try {
    s.open(at("people.ttl"), DATA)
    await s.diagnosticsFor(at("people.ttl"))
    let ended = false
    const slow = run(s, at("slow.rq")).finally(() => (ended = true))
    // Let the query start (no assertion rides on this pause).
    await new Promise((r) => setTimeout(r, 200))
    const pos = positionOf(DATA, "ex:alice", 4)
    for (let i = 0; i < 3; i++) {
      const hover = await s.request("textDocument/hover", { textDocument: { uri: at("people.ttl") }, position: pos })
      assert.ok(hover, "no hover")
      // Answered while the query runs, not after it.
      assert.equal(ended, false, `hover ${i} was answered after the query ended`)
    }
    const summary = await slow
    assert.equal(summary, "stopped at the time limit (2 s, query.timeoutSeconds)")
    assert.equal(s.messages.at(-1).type, 2)
    const md = fs.readFileSync(path.join(s.cacheDir, "query-results", "slow.md"), "utf8")
    assert.ok(md.includes("The query was stopped at the time limit (2 s, `query.timeoutSeconds`)."), md)
  } finally {
    await s.stop()
  }
})

test("a second run replaces the first; $/cancelRequest stops a run", async () => {
  const s = await startServer({ cwd: ws, initializationOptions: { query: { timeoutSeconds: 20 } } })
  try {
    // The runs are far from their 20 s limit: each answer says why it ended.
    const first = run(s, at("slow.rq"))
    await new Promise((r) => setTimeout(r, 100))
    const second = run(s, at("slow.rq"))
    assert.equal(await first, "cancelled: a newer run replaced it")
    s.sendRaw([{ id: "raw-q3", method: "workspace/executeCommand", params: { command: "turtle.runQuery", arguments: [at("slow.rq")] } }])
    assert.equal(await second, "cancelled: a newer run replaced it")
    await new Promise((r) => setTimeout(r, 100))
    s.sendRaw([{ method: "$/cancelRequest", params: { id: "raw-q3" } }])
    // A run the cancel missed would answer at its time limit, with a result.
    const resp = await s.rawResponse("raw-q3", 25000)
    assert.equal(resp.error?.code, -32800, JSON.stringify(resp))
    assert.ok(!fs.existsSync(path.join(s.cacheDir, "query-results", "slow.md")))
  } finally {
    await s.stop()
  }
})
