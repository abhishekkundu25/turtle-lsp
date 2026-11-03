// The custom request `turtle/query` over stdio: a query as text, its
// results as JSON (src/server/query_request.rs).
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir } from "./lsp-client.mjs"

const PREFIXES = `@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
const model = `${PREFIXES}ex:Pump a rdfs:Class ; rdfs:label "Pump" .
ex:Valve a rdfs:Class .
`
const data = `${PREFIXES}ex:p1 a ex:Pump ; ex:feeds ex:v1 .
ex:v1 a ex:Valve .
`
const EX = "http://example.org/"
const COUNT = "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }"
// A cross product of a chain of 400 triples: far too slow to finish.
const SLOW = "SELECT (COUNT(*) AS ?n) WHERE { ?a ?p ?b . ?c ?q ?d . ?e ?r ?f . ?g ?h ?i }"
const InvalidParams = -32602
const RequestFailed = -32803
const RequestCanceled = -32800

const count = async (s, params) => {
  const r = await s.request("turtle/query", params)
  return Number(r.results.bindings[0]?.n.value ?? 0)
}

// Ask until the indexed workspace has every triple (the initial index lands
// in the background).
async function indexed(s, triples, timeoutMs = 10000) {
  const end = Date.now() + timeoutMs
  for (;;) {
    const n = await count(s, { query: COUNT })
    if (n >= triples) return
    if (Date.now() > end) throw new Error(`the workspace has ${n} triples, not ${triples}`)
    await new Promise((res) => setTimeout(res, 50))
  }
}

describe("turtle/query", () => {
  let s, dir, dataUri
  before(async () => {
    dir = tempDir("turtle-lsp-query-request-")
    fs.writeFileSync(path.join(dir, "model.ttl"), model)
    fs.writeFileSync(path.join(dir, "data.ttl"), data)
    dataUri = uriOf(path.join(dir, "data.ttl"))
    s = await startServer({ cwd: dir })
    await indexed(s, 6)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("is listed under the experimental capabilities", () => {
    assert.equal(s.init.capabilities.experimental["turtle/query"], true)
  })

  test("answers SELECT as SPARQL results JSON with full IRIs", async () => {
    const r = await s.request("turtle/query", {
      query: "PREFIX ex: <http://example.org/> SELECT ?s ?o WHERE { ?s ex:feeds ?o }",
    })
    assert.equal(r.kind, "select")
    assert.deepEqual(r.head, { vars: ["s", "o"] })
    assert.deepEqual(r.results.bindings, [{ s: { type: "uri", value: EX + "p1" }, o: { type: "uri", value: EX + "v1" } }])
    assert.equal(r.omitted, 0)
    assert.equal(typeof r.elapsedMs, "number")
  })

  test("the default graph is the union of every file; there are no named graphs", async () => {
    assert.equal(await count(s, { query: COUNT }), 6)
    assert.equal(await count(s, { query: "SELECT (COUNT(*) AS ?n) WHERE { GRAPH ?g { ?s ?p ?o } }" }), 0)
  })

  test("files limits the dataset; an unknown file is InvalidParams", async () => {
    assert.equal(await count(s, { query: COUNT, files: [dataUri] }), 3)
    // A file given twice counts once.
    assert.equal(await count(s, { query: COUNT, files: [dataUri, dataUri] }), 3)
    await assert.rejects(s.request("turtle/query", { query: "SELECT * WHERE { ?s ?p ?o }", files: [uriOf(path.join(dir, "nope.ttl"))] }), (e) => {
      assert.equal(e.code, InvalidParams)
      assert.match(e.message, /nope\.ttl/)
      return true
    })
  })

  test("sees unsaved edits of open documents", async () => {
    s.open(dataUri, data + "ex:p2 a ex:Pump .\n")
    try {
      assert.equal(await count(s, { query: "PREFIX ex: <http://example.org/> SELECT (COUNT(*) AS ?n) WHERE { ?s a ex:Pump }" }), 2)
    } finally {
      s.close(dataUri)
    }
  })

  test("ASK, CONSTRUCT and maxRows", async () => {
    const a = await s.request("turtle/query", { query: "PREFIX ex: <http://example.org/> ASK { ex:p1 ex:feeds ex:v1 }" })
    assert.equal(a.kind, "ask")
    assert.equal(a.boolean, true)
    const c = await s.request("turtle/query", { query: "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }", maxRows: 2 })
    assert.equal(c.kind, "construct")
    assert.equal(c.triples.length, 2)
    assert.equal(c.omitted, 4)
    assert.equal(c.triples[0].subject.type, "uri")
    const r = await s.request("turtle/query", { query: "SELECT * WHERE { ?s ?p ?o }", maxRows: 1 })
    assert.equal(r.results.bindings.length, 1)
    assert.equal(r.omitted, 5)
  })

  test("a bad query is InvalidParams, a stopped one RequestFailed, and the server goes on", async () => {
    for (const params of [{ query: "SELECT WHERE" }, { query: "INSERT DATA { <a> <b> <c> }" }, { query: "SELECT * {}", nope: 1 }, { query: 3 }]) {
      await assert.rejects(s.request("turtle/query", params), (e) => {
        assert.equal(e.code, InvalidParams, JSON.stringify(params))
        assert.ok(e.message, JSON.stringify(params))
        return true
      })
    }
    await assert.rejects(s.request("turtle/query", { query: SLOW, timeoutSeconds: 0.000001 }), (e) => {
      assert.equal(e.code, RequestFailed)
      assert.match(e.message, /time limit/)
      return true
    })
    assert.equal(await count(s, { query: COUNT }), 6)
  })
})

describe("turtle/query limits", () => {
  let s, dir
  before(async () => {
    dir = tempDir("turtle-lsp-query-request-limits-")
    let big = "@prefix ex: <http://example.org/> .\n"
    for (let i = 0; i < 400; i++) big += `ex:n${i} ex:next ex:n${i + 1} .\n`
    fs.writeFileSync(path.join(dir, "big.ttl"), big)
    s = await startServer({ cwd: dir, initializationOptions: { query: { timeoutSeconds: 1 } } })
    await indexed(s, 400)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("timeoutSeconds can only lower query.timeoutSeconds: 0 and a higher value are the setting", async () => {
    for (const timeoutSeconds of [undefined, 0, 60]) {
      const start = Date.now()
      await assert.rejects(s.request("turtle/query", { query: SLOW, timeoutSeconds }), (e) => {
        assert.equal(e.code, RequestFailed, String(timeoutSeconds))
        assert.match(e.message, /time limit/)
        return true
      })
      assert.ok(Date.now() - start < 5000, `${timeoutSeconds}: stopped after ${Date.now() - start} ms`)
    }
    await assert.rejects(s.request("turtle/query", { query: SLOW, timeoutSeconds: -1 }), (e) => {
      assert.equal(e.code, InvalidParams)
      return true
    })
  })

  test("$/cancelRequest stops a run, answered RequestCanceled", async () => {
    s.sendRaw([{ id: "raw-query-1", method: "turtle/query", params: { query: SLOW } }])
    await new Promise((r) => setTimeout(r, 100))
    s.sendRaw([{ method: "$/cancelRequest", params: { id: "raw-query-1" } }])
    const resp = await s.rawResponse("raw-query-1")
    assert.equal(resp.error?.code, RequestCanceled, JSON.stringify(resp))
  })

  test("at most four queries run at a time; the fifth is refused", async () => {
    const ids = [1, 2, 3, 4, 5].map((n) => `raw-cap-${n}`)
    s.sendRaw(ids.map((id) => ({ id, method: "turtle/query", params: { query: SLOW } })))
    const fifth = await s.rawResponse(ids[4])
    assert.equal(fifth.error?.code, RequestFailed, JSON.stringify(fifth))
    assert.match(fifth.error.message, /4 queries are running/)
    // The four stop at the time limit, and the server answers again.
    for (const id of ids.slice(0, 4)) {
      const resp = await s.rawResponse(id)
      assert.equal(resp.error?.code, RequestFailed, JSON.stringify(resp))
    }
    assert.equal(await count(s, { query: "SELECT (COUNT(*) AS ?n) WHERE { ?s ?p ?o }" }), 400)
  })
})
