// The custom request `turtle/graphDiff` over stdio: the workspace against a
// base the client supplies (src/features/graph_diff.rs), as a client sees it.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir } from "./lsp-client.mjs"

const PREFIXES = `@prefix ex: <http://example.org/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
`
const EX = "http://example.org/"
const InvalidParams = -32602

const model = `${PREFIXES}ex:feeds a rdf:Property .\n`
const dataBase = `${PREFIXES}ex:p1 ex:feeds ex:v1 .\nex:v1 ex:size "2" .\n`
// ex:p1 ex:feeds ex:v1 is gone; ex:p1 ex:feeds ex:v2 is new; ex:v1's size changed.
const dataNow = `${PREFIXES}ex:p1 ex:feeds ex:v2 .\nex:v1 ex:size "3" .\n`
const extraBase = `${PREFIXES}ex:gone ex:feeds ex:p1 .\n`

const graphIds = (r) => new Set(r.nodes.map((n) => n.id))

describe("turtle/graphDiff", () => {
  let s, dir, dataUri, modelUri, newUri, goneUri
  before(async () => {
    dir = tempDir("turtle-lsp-graph-diff-")
    fs.writeFileSync(path.join(dir, "model.ttl"), model)
    fs.writeFileSync(path.join(dir, "data.ttl"), dataNow)
    fs.writeFileSync(path.join(dir, "new.ttl"), `${PREFIXES}ex:fresh ex:feeds ex:p1 .\n`)
    dataUri = uriOf(path.join(dir, "data.ttl"))
    modelUri = uriOf(path.join(dir, "model.ttl"))
    newUri = uriOf(path.join(dir, "new.ttl"))
    goneUri = uriOf(path.join(dir, "gone.ttl"))
    s = await startServer({ cwd: dir })
    const end = Date.now() + 10000
    while (!(await s.request("turtle/graph", { layout: { iterations: 0 } })).nodes.some((n) => n.id === EX + "fresh")) {
      if (Date.now() > end) throw new Error("the workspace was not indexed")
      await new Promise((r) => setTimeout(r, 50))
    }
  })
  after(async () => {
    await s?.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("is listed under the experimental capabilities", () => {
    assert.deepEqual(s.init.capabilities.experimental, { "turtle/graph": true, "turtle/graphDiff": true })
  })

  test("a changed closed file, a new file and a deleted file against the base", async () => {
    const r = await s.request("turtle/graphDiff", {
      base: [
        { uri: dataUri, text: dataBase },
        { uri: newUri, text: null },
        { uri: goneUri, text: extraBase },
      ],
    })
    const rows = Object.fromEntries(r.files.map((f) => [f.uri, f.rows]))
    assert.deepEqual(Object.keys(rows).sort(), [dataUri, goneUri, newUri].sort())
    // One object swapped for another on the same subject and predicate is a changed row.
    const changed = rows[dataUri].map((x) => `${x.change} ${x.subject.id.slice(EX.length)} ${x.predicate.id.slice(EX.length)} ${x.was.id.replace(EX, "")} ${x.object.id.replace(EX, "")}`)
    assert.deepEqual(changed.length, 2)
    assert.ok(changed.includes("changed p1 feeds v1 v2"), changed.join("\n"))
    assert.ok(rows[dataUri].every((x) => x.change === "changed"))
    assert.deepEqual(rows[newUri].map((x) => x.change), ["added"])
    assert.deepEqual(rows[goneUri].map((x) => x.change), ["removed"])
    assert.equal(r.total.triplesAdded, 3)
    assert.equal(r.total.triplesRemoved, 3)
    assert.equal(r.total.cut, false)
    assert.ok(r.terms.length > 0)
  })

  test("an edit to an open file is a change against its base", async () => {
    s.open(modelUri, model + "ex:extra a rdf:Property .\n")
    const r = await s.request("turtle/graphDiff", { base: [{ uri: modelUri, text: model }] })
    assert.equal(r.files.length, 1)
    assert.equal(r.files[0].uri, modelUri)
    assert.deepEqual(r.files[0].rows.map((x) => x.change), ["added"])
    assert.equal(r.files[0].rows[0].subject.id, EX + "extra")
    assert.deepEqual(r.terms.filter((t) => t.status === "added").map((t) => t.id).sort(), [EX + "extra"])
  })

  test("row and term ids are the node ids of turtle/graph", async () => {
    const g = await s.request("turtle/graph", { literals: true, types: true, layout: { iterations: 0 } })
    const nodes = graphIds(g)
    const r = await s.request("turtle/graphDiff", { base: [{ uri: dataUri, text: dataBase }] })
    const current = r.files.flatMap((f) => f.rows).filter((x) => x.change !== "removed")
    for (const row of current) {
      assert.ok(nodes.has(row.subject.id), row.subject.id)
      assert.ok(nodes.has(row.object.id), row.object.id)
    }
    for (const t of r.terms.filter((t) => t.status === "added")) assert.ok(nodes.has(t.id), t.id)
  })

  test("a bad request is an InvalidParams error, and the server goes on", async () => {
    for (const params of [
      {},
      { base: "x" },
      { base: [{ uri: uriOf("/elsewhere/other.ttl"), text: dataBase }] },
      { base: [{ uri: dataUri, text: null }, { uri: dataUri, text: null }] },
      { base: [], limit: "many" },
      [1, 2],
    ]) {
      await assert.rejects(s.request("turtle/graphDiff", params), (e) => {
        assert.equal(e.code, InvalidParams, JSON.stringify(params))
        assert.ok(e.message)
        return true
      })
    }
    const r = await s.request("turtle/graphDiff", { base: [] })
    assert.deepEqual(r.files, [])
  })
})
