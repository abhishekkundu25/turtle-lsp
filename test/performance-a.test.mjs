// Work that grew with the size of the workspace rather than with the work asked
// for: SHACL over many datasets, one change notification of many files, and a
// result of thousands of locations. These tests check that the answers stay right
// as that work is made proportional (the cost itself is counted in the Rust unit
// tests, and timed in bench/perf.mjs).
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const HEAD = "@prefix ex: <http://example.org/> .\n@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n"

async function until(check, what, ms = 20000) {
  const end = Date.now() + ms
  for (;;) {
    const v = await check()
    if (v) return v
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`)
    await sleep(20)
  }
}

test("SHACL over many TriG datasets: each shows only its own results, once", async () => {
  const dir = tempDir("turtle-perf-shacl-")
  try {
    let shared = HEAD
    for (let i = 0; i < 400; i++) shared += `ex:s${i} a foaf:Person ; foaf:name "S${i}" .\n`
    // One shared person has no name: a result of the shared graph, shown there once.
    shared += "ex:nameless a foaf:Person .\n"
    fs.writeFileSync(path.join(dir, "people.ttl"), shared)
    fs.writeFileSync(
      path.join(dir, "shapes.ttl"),
      `${HEAD}ex:PersonShape a sh:NodeShape ; sh:targetClass foaf:Person ;\n  sh:property [ sh:path foaf:name ; sh:minCount 1 ; sh:message "a name is needed" ] .\n`,
    )
    const datasets = 80
    for (let d = 0; d < datasets; d++) {
      const bad = d % 20 === 7 ? `  ex:d${d}x a foaf:Person .\n` : ""
      fs.writeFileSync(
        path.join(dir, `d${String(d).padStart(3, "0")}.trig`),
        `${HEAD}ex:g${d} {\n  ex:d${d}a a foaf:Person ; foaf:name "A" .\n  ex:d${d}b a foaf:Person ; foaf:name "B" .\n${bad}}\n`,
      )
    }
    const s = await startServer({ cwd: dir })
    try {
      await s.request("workspace/symbol", { query: "" })
      const shacl = (diags) => diags.filter((d) => d.code === "shacl-violation")
      const open = (name, text, id) => {
        const uri = uriOf(path.join(dir, name))
        s.open(uri, text, id)
        return uri
      }
      const people = open("people.ttl", shared, "turtle")
      const trigs = [7, 27, 47, 67, 3].map((d) => {
        const name = `d${String(d).padStart(3, "0")}.trig`
        return [d, open(name, fs.readFileSync(path.join(dir, name), "utf8"), "trig")]
      })
      await s.diagnosticsFor(people)
      for (const [, uri] of trigs) await s.diagnosticsFor(uri)
      s.notify("textDocument/didSave", { textDocument: { uri: people } })
      const got = await until(async () => {
        const mine = shacl(await s.diagnosticsFor(people))
        return mine.length === 1 ? mine : null
      }, "the shared result")
      assert.match(got[0].message, /a name is needed/)
      for (const [d, uri] of trigs) {
        const want = d % 20 === 7 ? 1 : 0
        const list = await until(async () => {
          const l = shacl(await s.diagnosticsFor(uri))
          return l.length === want ? l : null
        }, `dataset ${d}'s results`)
        assert.equal(list.length, want, `dataset ${d}`)
      }
      // The shared result stays one, however many datasets were validated.
      assert.equal(shacl(await s.diagnosticsFor(people)).length, 1)
    } finally {
      await s.stop()
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test("one notification of many changed files updates the index for all of them", async () => {
  const dir = tempDir("turtle-perf-watch-")
  try {
    const N = 300
    const file = (i) => path.join(dir, `f${i}.ttl`)
    const text = (i, v) => `${HEAD}ex:thing${i} ex:${v} "x" .\n`
    for (let i = 0; i < N; i++) fs.writeFileSync(file(i), text(i, "before"))
    const s = await startServer({ cwd: dir })
    try {
      await s.request("workspace/symbol", { query: "" })
      const names = async (query) => (await s.request("workspace/symbol", { query })).map((x) => x.name)
      assert.equal((await names("thing")).length, N)
      // Every file rewritten, one removed, one created; one notification.
      for (let i = 0; i < N; i++) fs.writeFileSync(file(i), text(i, "after") + `ex:extra${i} a ex:Kind .\n`)
      fs.rmSync(file(5))
      const made = path.join(dir, "made.ttl")
      fs.writeFileSync(made, `${HEAD}ex:madeLater a ex:Kind .\n`)
      const changes = [
        ...Array.from({ length: N }, (_, i) => ({ uri: uriOf(file(i)), type: i === 5 ? 3 : 2 })),
        { uri: uriOf(file(9)), type: 2 },
        { uri: uriOf(made), type: 1 },
      ]
      s.notify("workspace/didChangeWatchedFiles", { changes })
      const extra = await names("extra")
      assert.equal(extra.length, N - 1, "the deleted file's term is gone, the others are there")
      assert.ok(!extra.some((n) => n.endsWith("extra5")))
      assert.deepEqual(await names("madeLater"), ["ex:madeLater"])
    } finally {
      await s.stop()
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test("references on a term with thousands of uses name every one", async () => {
  const dir = tempDir("turtle-perf-refs-")
  try {
    let text = HEAD
    const N = 3000
    for (let i = 0; i < N; i++) text += `ex:s${i} a foaf:Person .\n`
    const file = path.join(dir, "big.ttl")
    fs.writeFileSync(file, text)
    const s = await startServer({ cwd: dir })
    try {
      await s.request("workspace/symbol", { query: "" })
      const uri = uriOf(file)
      s.open(uri, text)
      await s.diagnosticsFor(uri)
      const refs = await s.request("textDocument/references", {
        textDocument: { uri },
        position: { line: 3, character: 16 },
        context: { includeDeclaration: true },
      })
      assert.equal(refs.length, N)
      assert.ok(refs.every((l) => l.uri === uri))
      assert.equal(new Set(refs.map((l) => l.range.start.line)).size, N)
    } finally {
      await s.stop()
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
