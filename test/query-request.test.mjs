// The custom request `turtle/query` over stdio: a query as text, its
// results as JSON (src/extensions/query.rs).
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

  test("files: [] is InvalidParams, not an empty dataset", async () => {
    await assert.rejects(s.request("turtle/query", { query: COUNT, files: [] }), (e) => {
      assert.equal(e.code, InvalidParams)
      assert.match(e.message, /files is empty/)
      return true
    })
  })

  test("a relative IRI without base says that a base is missing", async () => {
    const query = "SELECT * WHERE { <p1> ?p ?o }"
    await assert.rejects(s.request("turtle/query", { query }), (e) => {
      assert.equal(e.code, InvalidParams)
      assert.match(e.message, /relative IRI needs a base/)
      return true
    })
    const r = await s.request("turtle/query", { query, base: EX })
    assert.equal(r.results.bindings.length, 2)
  })

  test("ASK, CONSTRUCT and maxRows", async () => {
    const a = await s.request("turtle/query", { query: "PREFIX ex: <http://example.org/> ASK { ex:p1 ex:feeds ex:v1 }" })
    assert.equal(a.kind, "ask")
    assert.equal(a.boolean, true)
    const no = await s.request("turtle/query", { query: "PREFIX ex: <http://example.org/> ASK { ex:v1 ex:feeds ex:p1 }" })
    assert.deepEqual(no.head, {})
    assert.equal(no.boolean, false)
    const c = await s.request("turtle/query", { query: "CONSTRUCT { ?s ?p ?o } WHERE { ?s ?p ?o }", maxRows: 2 })
    assert.equal(c.kind, "construct")
    assert.equal(c.triples.length, 2)
    assert.equal(c.omitted, 4)
    assert.equal(c.triples[0].subject.type, "uri")
    // Every term in results JSON form: IRIs, a blank node, a literal.
    const t = await s.request("turtle/query", {
      query: `PREFIX ex: <http://example.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>
        CONSTRUCT { _:b ex:names ?l . ?s ex:label ?l } WHERE { ?s rdfs:label ?l }`,
    })
    assert.equal(t.omitted, 0)
    const byPredicate = Object.fromEntries(t.triples.map((x) => [x.predicate.value, x]))
    assert.deepEqual(Object.keys(byPredicate).sort(), [EX + "label", EX + "names"])
    assert.deepEqual(byPredicate[EX + "label"], {
      subject: { type: "uri", value: EX + "Pump" },
      predicate: { type: "uri", value: EX + "label" },
      object: { type: "literal", value: "Pump" },
    })
    const b = byPredicate[EX + "names"]
    assert.equal(b.subject.type, "bnode")
    assert.ok(b.subject.value)
    assert.deepEqual(b.object, { type: "literal", value: "Pump" })
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

describe("turtle/query prefixes", () => {
  // Headerless files take ex: from prefixes.ttl; the N-Triples file has none.
  let s, dir, ntUri
  before(async () => {
    dir = tempDir("turtle-lsp-query-request-prefixes-")
    fs.writeFileSync(path.join(dir, "prefixes.ttl"), "@prefix ex: <http://example.org/> .\n")
    fs.writeFileSync(path.join(dir, "data.ttl"), "ex:p1 ex:feeds ex:v1 .\n")
    fs.writeFileSync(path.join(dir, "data.nt"), `<${EX}p2> <${EX}feeds> <${EX}v2> .\n`)
    ntUri = uriOf(path.join(dir, "data.nt"))
    s = await startServer({ cwd: dir })
    await indexed(s, 2)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  const FEEDS = "SELECT ?s WHERE { ?s ex:feeds ?o } ORDER BY ?s"

  test("a prefix of the root's prefixes.ttl works with files omitted", async () => {
    const r = await s.request("turtle/query", { query: FEEDS })
    assert.deepEqual(r.results.bindings.map((b) => b.s.value), [EX + "p1", EX + "p2"])
  })

  test("a prefix of prefixes.ttl works when the first file is N-Triples", async () => {
    const r = await s.request("turtle/query", { query: FEEDS, files: [ntUri] })
    assert.deepEqual(r.results.bindings.map((b) => b.s.value), [EX + "p2"])
  })
})

describe("turtle/query maxRows", () => {
  // More rows than the default query.maxRows (1000).
  let s, dir
  before(async () => {
    dir = tempDir("turtle-lsp-query-request-rows-")
    let big = "@prefix ex: <http://example.org/> .\n"
    for (let i = 0; i < 1500; i++) big += `ex:n${i} ex:next ex:n${i + 1} .\n`
    fs.writeFileSync(path.join(dir, "big.ttl"), big)
    s = await startServer({ cwd: dir })
    await indexed(s, 1500)
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("omitted, 0 and a higher value are query.maxRows; a lower value lowers it", async () => {
    for (const maxRows of [undefined, 0, 5000]) {
      const r = await s.request("turtle/query", { query: "SELECT * WHERE { ?s ?p ?o }", maxRows })
      assert.equal(r.results.bindings.length, 1000, String(maxRows))
      assert.equal(r.omitted, 500, String(maxRows))
    }
    const c = await s.request("turtle/query", { query: "CONSTRUCT WHERE { ?s ?p ?o }" })
    assert.equal(c.triples.length, 1000)
    assert.equal(c.omitted, 500)
    const r = await s.request("turtle/query", { query: "SELECT * WHERE { ?s ?p ?o }", maxRows: 10 })
    assert.equal(r.results.bindings.length, 10)
    assert.equal(r.omitted, 1490)
  })
})

describe("turtle/query and the end of the server", () => {
  // No time limit: only the end of the server stops the run.
  let dir
  before(() => {
    dir = tempDir("turtle-lsp-query-request-exit-")
    let big = "@prefix ex: <http://example.org/> .\n"
    for (let i = 0; i < 400; i++) big += `ex:n${i} ex:next ex:n${i + 1} .\n`
    fs.writeFileSync(path.join(dir, "big.ttl"), big)
  })
  after(() => fs.rmSync(dir, { recursive: true, force: true }))

  // Start a server, start a slow query, end the session with `end`, and
  // assert that the process exits within 3 s.
  async function endsWithQueryRunning(end) {
    const s = await startServer({ cwd: dir, initializationOptions: { query: { timeoutSeconds: 0 } } })
    try {
      await indexed(s, 400)
      s.sendRaw([{ id: "raw-slow", method: "turtle/query", params: { query: SLOW } }])
      await new Promise((r) => setTimeout(r, 200))
      const exited = new Promise((r) => s.proc.once("exit", () => r(true)))
      if (s.proc.exitCode !== null) return
      await end(s)
      const gone = await Promise.race([exited, new Promise((r) => setTimeout(() => r(false), 3000).unref())])
      assert.ok(gone, "turtle-lsp did not exit within 3 s")
    } finally {
      if (s.proc.exitCode === null) s.proc.kill()
      s.conn.dispose()
    }
  }

  test("shutdown and exit stop a running query", () =>
    endsWithQueryRunning(async (s) => {
      s.sendRaw([{ id: "raw-bye", method: "shutdown" }])
      const resp = await s.rawResponse("raw-bye")
      assert.equal(resp.error, undefined)
      s.sendRaw([{ method: "exit" }])
    }))

  test("exit alone stops a running query", () => endsWithQueryRunning(async (s) => s.sendRaw([{ method: "exit" }])))

  test("stdin closing stops a running query", () => endsWithQueryRunning(async (s) => s.proc.stdin.end()))
})

describe("turtle/query reasoning", () => {
  // ex:Pump is a subclass of ex:Device here, and ex:Device of ex:Thing in
  // the imported ontology (RDF/XML: an import, never a workspace document).
  const MODEL = `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://example.org/model> a owl:Ontology ; owl:imports <http://example.org/vendor> .
ex:Pump rdfs:subClassOf ex:Device .
ex:p1 a ex:Pump .
`
  const VENDOR = `<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
     xmlns:owl="http://www.w3.org/2002/07/owl#"
     xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
    <owl:Ontology rdf:about="http://example.org/vendor"/>
    <owl:Class rdf:about="http://example.org/Device">
        <rdfs:subClassOf rdf:resource="http://example.org/Thing"/>
    </owl:Class>
</rdf:RDF>
`
  const CATALOG = `<?xml version="1.0" encoding="UTF-8"?>
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
    <uri name="http://example.org/vendor" uri="vendor/vendor.rdf"/>
</catalog>
`
  const OF = (cls) => `PREFIX ex: <http://example.org/> SELECT ?s WHERE { ?s a ex:${cls} }`
  const subjects = (r) => r.results.bindings.map((b) => b.s.value)
  const without = ({ elapsedMs, ...rest }) => rest
  const dirs = []
  after(() => dirs.forEach((d) => fs.rmSync(d, { recursive: true, force: true })))

  async function workspace(initializationOptions) {
    const dir = tempDir("turtle-lsp-query-request-reasoning-")
    dirs.push(dir)
    fs.mkdirSync(path.join(dir, "vendor"))
    fs.writeFileSync(path.join(dir, "model.ttl"), MODEL)
    fs.writeFileSync(path.join(dir, "vendor", "vendor.rdf"), VENDOR)
    fs.writeFileSync(path.join(dir, "catalog-v001.xml"), CATALOG)
    const s = await startServer({ cwd: dir, initializationOptions })
    try {
      await indexed(s, 4)
    } catch (e) {
      await s.stop()
      throw e
    }
    return s
  }

  // Ask until the import has loaded (imports resolve after the index).
  async function imported(s) {
    const end = Date.now() + 10000
    for (;;) {
      const r = await s.request("turtle/query", { query: OF("Thing"), reasoning: "default" })
      if (subjects(r).length > 0) return r
      if (Date.now() > end) throw new Error("the import never inferred ex:p1 a ex:Thing")
      await new Promise((res) => setTimeout(res, 50))
    }
  }

  test("default and complete add the inferred rows and say so; off answers as before", async () => {
    const s = await workspace()
    try {
      const plain = await s.request("turtle/query", { query: OF("Device") })
      assert.deepEqual(subjects(plain), [])
      assert.equal("inferred" in plain, false)
      const off = await s.request("turtle/query", { query: OF("Device"), reasoning: "off" })
      assert.deepEqual(without(off), without(plain))
      for (const rules of ["default", "complete"]) {
        const r = await s.request("turtle/query", { query: OF("Device"), reasoning: rules })
        assert.deepEqual(subjects(r), [EX + "p1"], rules)
        assert.deepEqual(Object.keys(r.inferred).sort(), ["findings", "rules", "triples"], rules)
        assert.equal(r.inferred.rules, rules)
        assert.ok(r.inferred.triples > 0, JSON.stringify(r.inferred))
        assert.equal(r.inferred.findings, 0)
      }
    } finally {
      await s.stop()
    }
  })

  test("with reasoning an import is queried: its axioms infer and its own triples are rows; off it is not read", async () => {
    const s = await workspace()
    try {
      const r = await imported(s)
      assert.deepEqual(subjects(r), [EX + "p1"])
      const query =
        "PREFIX ex: <http://example.org/> PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#> ASK { ex:Device rdfs:subClassOf ex:Thing }"
      const on = await s.request("turtle/query", { query, reasoning: "default" })
      assert.equal(on.boolean, true)
      const off = await s.request("turtle/query", { query, reasoning: "off" })
      assert.equal(off.boolean, false)
    } finally {
      await s.stop()
    }
  })

  test("an import that is also open is read once, as the editor has it; files never names an import", async () => {
    const dir = tempDir("turtle-lsp-query-request-dup-")
    dirs.push(dir)
    // An import in a folder the workspace leaves out, so it is an import until opened.
    fs.mkdirSync(path.join(dir, "node_modules"))
    const vendor = path.join(dir, "node_modules", "vendor.ttl")
    const text = '@prefix ex: <http://example.org/> .\n_:x ex:name "one" .\n'
    fs.writeFileSync(vendor, text)
    fs.writeFileSync(path.join(dir, "model.ttl"), "<http://example.org/model> <http://www.w3.org/2002/07/owl#imports> <http://example.org/vendor> .\n")
    fs.writeFileSync(path.join(dir, "catalog-v001.xml"), '<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog"><uri name="http://example.org/vendor" uri="node_modules/vendor.ttl"/></catalog>')
    const s = await startServer({ cwd: dir })
    const names = async (reasoning) =>
      (await s.request("turtle/query", { query: "SELECT ?n WHERE { ?s <http://example.org/name> ?n }", reasoning })).results.bindings.map((b) => b.n.value).sort()
    try {
      for (const end = Date.now() + 10000; (await names("default")).length === 0; ) {
        if (Date.now() > end) throw new Error("the import never loaded")
        await new Promise((res) => setTimeout(res, 50))
      }
      for (const reasoning of ["off", "default"]) {
        await assert.rejects(
          s.request("turtle/query", { query: "ASK {}", reasoning, files: [uriOf(vendor)] }),
          (e) => e.code === InvalidParams,
          reasoning,
        )
      }
      s.open(uriOf(vendor), text.replace("one", "edited"))
      const end = Date.now() + 10000
      while ((await names("off")).join() !== "edited" && Date.now() < end) await new Promise((res) => setTimeout(res, 50))
      assert.deepEqual(await names("off"), ["edited"])
      assert.deepEqual(await names("default"), ["edited"])
    } finally {
      await s.stop()
    }
  })

  test("a bad reasoning value is InvalidParams", async () => {
    const s = await workspace()
    try {
      for (const reasoning of ["on", "Default", 1, true]) {
        await assert.rejects(s.request("turtle/query", { query: OF("Device"), reasoning }), (e) => {
          assert.equal(e.code, InvalidParams, String(reasoning))
          assert.ok(e.message, String(reasoning))
          return true
        })
      }
    } finally {
      await s.stop()
    }
  })

  test("the default is query.reasoning", async () => {
    const s = await workspace({ query: { reasoning: "default" } })
    try {
      const r = await s.request("turtle/query", { query: OF("Device") })
      assert.deepEqual(subjects(r), [EX + "p1"])
      assert.equal(r.inferred.rules, "default")
      const off = await s.request("turtle/query", { query: OF("Device"), reasoning: "off" })
      assert.deepEqual(subjects(off), [])
      assert.equal("inferred" in off, false)
    } finally {
      await s.stop()
    }
  })

  test("with files, only those files and the imports are read: other documents neither infer nor answer", async () => {
    const dir = tempDir("turtle-lsp-query-request-files-")
    dirs.push(dir)
    const P = `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
    fs.writeFileSync(path.join(dir, "onto.ttl"), P + `<http://example.org/onto> a owl:Ontology .\nex:Pump rdfs:subClassOf ex:Device .\n`)
    fs.writeFileSync(
      path.join(dir, "data.ttl"),
      P + `<http://example.org/data> a owl:Ontology ; owl:imports <http://example.org/onto> .\nex:p1 a ex:Pump .\n`,
    )
    fs.writeFileSync(path.join(dir, "other.ttl"), P + `ex:q1 ex:note "other" .\n`)
    const s = await startServer({ cwd: dir })
    try {
      await indexed(s, 6)
      const files = [uriOf(path.join(dir, "data.ttl"))]
      // onto.ttl's axiom is not read: no inference from it.
      const r = await s.request("turtle/query", { query: OF("Device"), reasoning: "default", files })
      assert.deepEqual(subjects(r), [])
      assert.ok("triples" in r.inferred, JSON.stringify(r.inferred))
      const a = await s.request("turtle/query", {
        query: "PREFIX ex: <http://example.org/> ASK { ex:q1 ?p ?o }",
        reasoning: "default",
        files,
      })
      assert.equal(a.boolean, false)
      // Named, it is read.
      const both = [...files, uriOf(path.join(dir, "onto.ttl"))]
      const named = await s.request("turtle/query", { query: OF("Device"), reasoning: "default", files: both })
      assert.deepEqual(subjects(named), [EX + "p1"])
      const off = await s.request("turtle/query", { query: OF("Device"), reasoning: "off", files: both })
      assert.deepEqual(subjects(off), [])
    } finally {
      await s.stop()
    }
  })

  test("over turtle.reasoning.maxTriples the inferred triples are skipped, and the answer says so", async () => {
    const s = await workspace({ reasoning: { maxTriples: 1 } })
    try {
      const r = await s.request("turtle/query", { query: OF("Device"), reasoning: "default" })
      assert.deepEqual(subjects(r), [])
      assert.deepEqual(Object.keys(r.inferred), ["skipped"])
      assert.equal(r.inferred.skipped.limit, 1)
      assert.ok(r.inferred.skipped.triples >= 4, JSON.stringify(r.inferred))
    } finally {
      await s.stop()
    }
  })
})
