// SHACL validation on save and after typing pauses. Each test runs in a temporary copy of
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
    // Typing validates only once it pauses for a second.
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

test("an unresolved import turns a class violation into a warning naming it, until it resolves", async () => {
  const ONTO = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/site> a owl:Ontology ;
    owl:imports <http://example.invalid/units> .
`
  const w = await session({}, { "site.ttl": ONTO })
  const file = path.join(w.dir, "data", "pump.ttl")
  const site = path.join(w.dir, "site.ttl")
  try {
    const text = GOOD.replace(" .\n", " ;\n    sd:fillsPosition obj:mill-0517 .\n")
    w.s.open(w.pump, text)
    save(w.s, w.pump, file, text)
    const [d] = shacl(await until(w.s, w.pump, (d) => shacl(d).some((x) => x.severity === 2)))
    assert.equal(
      d.message,
      "obj:mill-0517 (sd:fillsPosition of obj:pump-07) is not an instance of sd:Position (may change once these imports resolve: http://example.invalid/units)",
    )
    // The import goes away: the next validation is strict again.
    const resolved = ONTO.replace(" ;\n    owl:imports <http://example.invalid/units>", "")
    const siteUri = uriOf(site)
    w.s.open(siteUri, resolved)
    save(w.s, siteUri, site, resolved)
    const [e] = shacl(await until(w.s, w.pump, (d) => shacl(d).some((x) => x.severity === 1)))
    assert.equal(e.message, "obj:mill-0517 (sd:fillsPosition of obj:pump-07) is not an instance of sd:Position")
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


test("a dataset that mentions a violating shared node does not repeat its violation", async () => {
  // Without a designation: shown on the node, which the dataset also describes.
  const missing = GOOD.replace(' ;\n    sd:designation "=P07" .', " .")
  const run5 = '{ obj:pump-07 rdfs:label "Pump 07, run 5" . }\n'
  const { s, dir, pump, end } = await session({}, { "run5.trig": run5 })
  const file = path.join(dir, "data", "pump.ttl")
  try {
    const uri = uriOf(path.join(dir, "run5.trig"))
    s.conn.sendNotification("textDocument/didOpen", {
      textDocument: { uri, languageId: "trig", version: 1, text: run5 },
    })
    assert.notEqual(missing, GOOD)
    s.open(pump, missing)
    save(s, pump, file, missing)
    const d = shacl(await until(s, pump, (d) => shacl(d).length > 0))
    assert.equal(d.length, 1, JSON.stringify(d))
    assert.equal(d[0].message, MESSAGE)
    assert.deepEqual(shacl(await s.diagnosticsFor(uri)), [])
  } finally {
    await end()
  }
})

test("typing validates the unsaved text once typing pauses; editing back clears it", async () => {
  const w = await session()
  try {
    w.s.open(w.pump, GOOD)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
    w.s.change(w.pump, BAD)
    const diags = shacl(await until(w.s, w.pump, (d) => shacl(d).length > 0, 5000))
    assert.equal(diags.length, 1)
    assert.equal(diags[0].message, MESSAGE)
    w.s.change(w.pump, GOOD)
    await until(w.s, w.pump, (d) => shacl(d).length === 0, 5000)
  } finally {
    await w.end()
  }
})

test("a burst of edits is validated once, after it ends", async () => {
  const w = await session()
  try {
    w.s.open(w.pump, GOOD)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
    const from = w.s.publications.length
    // Alternate between violating and conforming text, ending violating: a
    // validation during the burst would leave a violation on every later
    // publication.
    for (let i = 0; i < 20; i++) {
      w.s.change(w.pump, i % 2 === 0 ? GOOD : BAD)
      await sleep(50)
    }
    const burstEnd = performance.now()
    await until(w.s, w.pump, (d) => shacl(d).length > 0, 5000)
    await sleep(1500)
    const withViolation = w.s.publications
      .slice(from)
      .filter((p) => p.uri === w.pump && shacl(p.diagnostics).length > 0)
    assert.equal(withViolation.length, 1, `${withViolation.length} publications with the violation`)
    assert.ok(withViolation[0].at - burstEnd > 800, `validated ${(withViolation[0].at - burstEnd).toFixed(0)} ms after the burst`)
  } finally {
    await w.end()
  }
})

test("shacl.onType false: typing does not validate, saving does", async () => {
  const w = await session({ initializationOptions: { shacl: { onType: false } } })
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, GOOD)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
    w.s.change(w.pump, BAD)
    await sleep(1500)
    assert.deepEqual(shacl(await w.s.diagnosticsFor(w.pump)), [])
    save(w.s, w.pump, file, BAD)
    await until(w.s, w.pump, (d) => shacl(d).length > 0)
  } finally {
    await w.end()
  }
})

// The test client answers window/workDoneProgress/create with null (success).
const PROGRESS = { window: { workDoneProgress: true } }

// The progress of each token: { title, end } (end is the end message, or
// true without one), in the order begun.
function progressByToken(s) {
  const by = new Map()
  for (const { token, value } of s.progress) {
    if (value.kind === "begin") by.set(token, { title: value.title, end: undefined })
    else if (value.kind === "end" && by.has(token)) by.get(token).end = value.message ?? true
  }
  return by
}

test("progress: indexing is created, begun and ended", async () => {
  const w = await session({ capabilities: PROGRESS })
  try {
    const created = w.s.serverRequests
      .filter((r) => r.method === "window/workDoneProgress/create")
      .map((r) => r.params.token)
    // begin goes out once the client has answered create: wait for the end.
    const indexed = () => [...progressByToken(w.s)].find(([, p]) => /index/i.test(p.title) && p.end)
    for (let i = 0; i < 100 && !indexed(); i++) await sleep(20)
    const indexing = indexed()
    assert.ok(indexing, JSON.stringify(w.s.progress))
    assert.ok(created.includes(indexing[0]), "token created before use")
    assert.match(String(indexing[1].end), /\d+ files?/)
  } finally {
    await w.end()
  }
})

const CHAIN_SHAPES = `@prefix ex: <http://example.com/chain/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
ex:ChainShape sh:targetSubjectsOf ex:next ;
  sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:minCount 1 ] .
`
function chain() {
  let text = "@prefix ex: <http://example.com/chain/> .\n"
  for (let i = 0; i < 8000; i++) text += `ex:n${i} ex:next ex:n${i + 1} .\n`
  return text
}

test("progress: a slow SHACL run reports begin and ends with a count", async () => {
  const w = await session({ capabilities: PROGRESS }, { "chain.ttl": chain(), "chain-shapes.ttl": CHAIN_SHAPES })
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, BAD)
    save(w.s, w.pump, file, BAD)
    await until(w.s, w.pump, (d) => shacl(d).length > 0, 20000)
    await sleep(100)
    const runs = [...progressByToken(w.s).values()].filter((p) => /SHACL/.test(p.title))
    assert.ok(runs.length > 0, JSON.stringify(w.s.progress))
    assert.ok(
      runs.some((p) => /^\d+ errors? in \d+ files?$/.test(p.end)),
      JSON.stringify(runs)
    )
    // No pop-ups.
    assert.ok(!w.s.serverRequests.some((r) => r.method === "window/showMessageRequest"))
  } finally {
    await w.end()
  }
})

test("progress: none without the client capability", async () => {
  const w = await session({}, { "chain.ttl": chain(), "chain-shapes.ttl": CHAIN_SHAPES })
  const file = path.join(w.dir, "data", "pump.ttl")
  try {
    w.s.open(w.pump, BAD)
    save(w.s, w.pump, file, BAD)
    await until(w.s, w.pump, (d) => shacl(d).length > 0, 20000)
    assert.deepEqual(w.s.progress, [])
    assert.ok(!w.s.serverRequests.some((r) => r.method === "window/workDoneProgress/create"))
  } finally {
    await w.end()
  }
})

test("progress: nothing is sent for a token the client refuses", async () => {
  const w = await session({
    capabilities: PROGRESS,
    onRequest: (method) => {
      if (method === "window/workDoneProgress/create") throw new Error("no progress here")
      return null
    },
  })
  try {
    assert.ok(w.s.serverRequests.some((r) => r.method === "window/workDoneProgress/create"))
    await sleep(200)
    assert.deepEqual(w.s.progress, [])
  } finally {
    await w.end()
  }
})

// A shape's path nested 20,000 steps deep (flat triples, so the file itself is
// not nested) cannot be read: the shape is not validated, and says so.
const nestedPath = (steps) => {
  let text = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.com/> .
ex:PathShape${steps} sh:targetNode ex:a${steps} ;
    sh:property [ sh:path _:b0 ; sh:minCount 1 ] .
ex:a${steps} ex:other 1 .
`
  for (let i = 0; i < steps; i++) text += `_:b${i} sh:inversePath _:b${i + 1} .\n`
  return text + `_:b${steps} sh:inversePath ex:p .\n`
}
const invalidPath = (diags) => diags.filter((d) => d.code === "shacl-invalid-path")

test("a path too deep to read is an error on its shape; a short one validates", async () => {
  const deep = nestedPath(20000)
  const short = nestedPath(10)
  const w = await session({}, { "deep-shape.ttl": deep, "short-shape.ttl": short })
  try {
    const deepUri = uriOf(path.join(w.dir, "deep-shape.ttl"))
    w.s.open(deepUri, deep)
    const [d] = invalidPath(await until(w.s, deepUri, (d) => invalidPath(d).length > 0, 10000))
    assert.equal(d.severity, 1)
    assert.equal(deep.split("\n")[d.range.start.line].trim().startsWith("sh:property"), true, JSON.stringify(d.range))
    // 11 steps are read: the missing value is a violation, not a path error.
    const shortUri = uriOf(path.join(w.dir, "short-shape.ttl"))
    w.s.open(shortUri, short)
    const diags = await until(w.s, shortUri, (d) => shacl(d).length > 0, 10000)
    assert.deepEqual(invalidPath(diags), [])
  } finally {
    await w.end()
  }
})
