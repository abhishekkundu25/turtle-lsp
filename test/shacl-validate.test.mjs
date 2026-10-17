// SHACL validation on save. Each test runs in a temporary copy of
// test/fixtures/project (whose data conforms to its shapes) with one more
// data file, data/pump.ttl, that is valid until a test makes it violate
// sdsh:AssetShape's designation pattern.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures } from "./lsp-client.mjs"

const GOOD = `# A pump (fragment)
obj:pump-07 a sd:Asset ;
    rdfs:label "Pump 07" ;
    sd:designation "=P07" .
`
const BAD = GOOD.replace('"=P07"', '"P07"')
const MESSAGE = "Every asset needs exactly one designation like =MC2.M05"
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

function workspace(extra = {}) {
  const dir = tempDir("turtle-shacl-")
  fs.cpSync(path.join(fixtures, "project"), dir, { recursive: true })
  fs.writeFileSync(path.join(dir, "data", "pump.ttl"), GOOD)
  for (const [f, text] of Object.entries(extra)) fs.writeFileSync(path.join(dir, f), text)
  return dir
}

async function session(opts = {}, extra) {
  const dir = workspace(extra)
  const s = await startServer({ cwd: dir, ...opts })
  // Requests wait for the initial index, so validation has been scheduled.
  await s.request("workspace/symbol", { query: "" })
  const pump = uriOf(path.join(dir, "data", "pump.ttl"))
  return {
    s,
    dir,
    pump,
    async end() {
      await s.stop()
      fs.rmSync(dir, { recursive: true, force: true })
    },
  }
}

const shacl = (diags) => diags.filter((d) => String(d.code).startsWith("shacl-violation") || d.code === "shacl-skipped")

// Wait until the published diagnostics of `uri` satisfy `ok`.
async function until(s, uri, ok, ms = 5000) {
  const end = Date.now() + ms
  for (;;) {
    const d = await s.diagnosticsFor(uri, ms)
    if (ok(d)) return d
    if (Date.now() > end) throw new Error(`timed out; last diagnostics: ${JSON.stringify(d, null, 1)}`)
    await sleep(25)
  }
}

function save(s, uri, file, text) {
  fs.writeFileSync(file, text)
  s.notify("textDocument/didSave", { textDocument: { uri } })
}

function rangeOf(text, needle) {
  const at = text.indexOf(needle)
  const pos = (off) => {
    const lines = text.slice(0, off).split("\n")
    return { line: lines.length - 1, character: lines.at(-1).length }
  }
  return { start: pos(at), end: pos(at + needle.length) }
}

test("saving a violating file shows the violation on its triple; fixing it clears it", async () => {
  const w = await session()
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, GOOD)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
    w.s.change(w.pump, BAD)
    // Typing alone does not validate.
    await sleep(300)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])

    save(w.s, w.pump, file, BAD)
    const diags = shacl(await until(w.s, w.pump, (d) => shacl(d).length > 0))
    assert.equal(diags.length, 1, JSON.stringify(diags, null, 1))
    const [d] = diags
    assert.equal(d.code, "shacl-violation")
    assert.equal(d.severity, 1)
    assert.equal(d.message, MESSAGE)
    assert.deepEqual(d.range, rangeOf(BAD, '"P07"'))
    const rel = d.relatedInformation?.[0]
    assert.ok(rel?.location.uri.endsWith("/shapes/asset-shapes.ttl"), JSON.stringify(d))
    assert.match(rel.message, /shape/)

    w.s.change(w.pump, GOOD)
    save(w.s, w.pump, file, GOOD)
    await until(w.s, w.pump, (d) => shacl(d).length === 0)
  } finally {
    await w.end()
  }
})

test("a component message names the path and value when there is no sh:message", async () => {
  const w = await session()
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    const text = GOOD.replace(" .\n", " ;\n    sd:fillsPosition obj:mill-0517 .\n")
    w.s.open(w.pump, text)
    save(w.s, w.pump, file, text)
    const [d] = shacl(await until(w.s, w.pump, (d) => shacl(d).length > 0))
    assert.equal(d.message, "obj:mill-0517 (sd:fillsPosition of obj:pump-07) is not an instance of sd:Position")
    assert.deepEqual(d.range, rangeOf(text, "obj:mill-0517"))
  } finally {
    await w.end()
  }
})

test("pull clients get the results after a refresh", async () => {
  const w = await session({
    capabilities: {
      textDocument: { diagnostic: { dynamicRegistration: false } },
      workspace: { diagnostics: { refreshSupport: true } },
    },
  })
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, BAD)
    const pull = () => w.s.request("textDocument/diagnostic", { textDocument: { uri: w.pump } })
    assert.deepEqual(shacl((await pull()).items), [])
    const refreshes = () => w.s.serverRequests.filter((r) => r.method === "workspace/diagnostic/refresh").length
    const before = refreshes()
    save(w.s, w.pump, file, BAD)
    const end = Date.now() + 5000
    while (refreshes() === before && Date.now() < end) await sleep(25)
    const items = shacl((await pull()).items)
    assert.equal(items.length, 1)
    assert.equal(items[0].message, MESSAGE)
  } finally {
    await w.end()
  }
})

test("requests are answered while a validation runs", async () => {
  // Every node of a 8,000-node chain is a focus node with an ex:next* path,
  // so validation takes a while (quadratic in the chain).
  let chain = "@prefix ex: <http://example.com/chain/> .\n"
  for (let i = 0; i < 8000; i++) chain += `ex:n${i} ex:next ex:n${i + 1} .\n`
  const shapes = `@prefix ex: <http://example.com/chain/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
ex:ChainShape sh:targetSubjectsOf ex:next ;
  sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:minCount 1 ] .
`
  const w = await session({}, { "chain.ttl": chain, "chain-shapes.ttl": shapes })
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, BAD)
    await w.s.diagnosticsFor(w.pump)
    // The run after the initial index may still be going; start a new one.
    const t0 = performance.now()
    save(w.s, w.pump, file, BAD)
    const answered = []
    for (let i = 0; i < 5; i++) {
      const t = performance.now()
      await w.s.request("textDocument/hover", { textDocument: { uri: w.pump }, position: { line: 1, character: 3 } })
      answered.push(performance.now() - t)
    }
    const lastAnswer = performance.now() - t0
    await until(w.s, w.pump, (d) => shacl(d).length > 0, 20000)
    const validated = performance.now() - t0
    assert.ok(Math.max(...answered) < 200, `hover took ${answered.map((x) => x.toFixed(0))} ms`)
    assert.ok(validated > lastAnswer, `validation (${validated.toFixed(0)} ms) ended before the requests (${lastAnswer.toFixed(0)} ms)`)
  } finally {
    await w.end()
  }
})

test("over shacl.maxTriples, each file with shapes explains the skip", async () => {
  const w = await session({ initializationOptions: { shacl: { maxTriples: 1 } } })
  try {
    const shapes = uriOf(path.join(w.dir, "shapes", "asset-shapes.ttl"))
    w.s.open(shapes, fs.readFileSync(path.join(w.dir, "shapes", "asset-shapes.ttl"), "utf8"))
    const d = shacl(await until(w.s, shapes, (d) => shacl(d).length > 0))
    assert.equal(d.length, 1)
    assert.equal(d[0].code, "shacl-skipped")
    assert.equal(d[0].severity, 3)
    assert.match(d[0].message, /skipped.*maxTriples/)
    w.s.open(w.pump, BAD)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
  } finally {
    await w.end()
  }
})

test("shacl.enabled false: no validation", async () => {
  const w = await session({ initializationOptions: { shacl: { enabled: false } } })
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, BAD)
    save(w.s, w.pump, file, BAD)
    await sleep(500)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
    // Turning it on validates; off again clears.
    w.s.notify("workspace/didChangeConfiguration", { settings: { turtle: { shacl: { enabled: true } } } })
    await until(w.s, w.pump, (d) => shacl(d).length === 1)
    w.s.notify("workspace/didChangeConfiguration", { settings: { turtle: { shacl: { enabled: false } } } })
    await until(w.s, w.pump, (d) => shacl(d).length === 0)
  } finally {
    await w.end()
  }
})

test("TriG datasets are validated with the shared graph but never merged with each other", async () => {
  // Two simulation runs reuse the same IRIs; a third gives one asset two designations.
  const run = (designations) => `{ obj:pump-08 a sd:Asset ;\n    sd:designation ${designations} . }\n`
  const { s, dir, end } = await session({}, {
    "run1.trig": run('"=P08A"'),
    "run2.trig": run('"=P08B"'),
    "run3.trig": run('"=P08A" , "=P08B"'),
  })
  try {
    const open = (f) => {
      const uri = uriOf(path.join(dir, f))
      s.conn.sendNotification("textDocument/didOpen", {
        textDocument: { uri, languageId: "trig", version: 1, text: fs.readFileSync(path.join(dir, f), "utf8") },
      })
      s.notify("textDocument/didSave", { textDocument: { uri } })
      return uri
    }
    const [r1, r2, r3] = ["run1.trig", "run2.trig", "run3.trig"].map(open)
    const d3 = await until(s, r3, (d) => shacl(d).length > 0)
    assert.equal(shacl(d3).length, 1, JSON.stringify(shacl(d3)))
    for (const uri of [r1, r2]) assert.deepEqual(shacl(await s.diagnosticsFor(uri)), [])
  } finally {
    await end()
  }
})

test("a violation that only exists with a dataset merged in is reported in that dataset", async () => {
  // data/pump.ttl (shared) gives obj:pump-07 one designation; this run adds a second.
  const { s, dir, end } = await session({}, { "run4.trig": '{ obj:pump-07 sd:designation "=P07B" . }\n' })
  try {
    const uri = uriOf(path.join(dir, "run4.trig"))
    s.conn.sendNotification("textDocument/didOpen", {
      textDocument: { uri, languageId: "trig", version: 1, text: fs.readFileSync(path.join(dir, "run4.trig"), "utf8") },
    })
    s.notify("textDocument/didSave", { textDocument: { uri } })
    const d = shacl(await until(s, uri, (d) => shacl(d).length > 0))
    assert.equal(d.length, 1, JSON.stringify(d))
    assert.equal(d[0].range.start.line, 0)
    // The shared file on its own conforms.
    s.open(uriOf(path.join(dir, "data", "pump.ttl")), GOOD)
    assert.deepEqual(shacl(await s.diagnosticsFor(uriOf(path.join(dir, "data", "pump.ttl")))), [])
  } finally {
    await end()
  }
})

