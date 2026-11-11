// Reasoning (`turtle.reasoning.rules`) as the editor sees it: TurbOWL's
// findings as diagnostics, after open, save and a pause in typing, with
// progress, the size limit's note, and nothing at all with reasoning off.
// Each test runs in a temporary folder of its own.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf } from "./lsp-client.mjs"

const HEAD = "@prefix ex: <http://example.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
// ex:tom is a member of two disjoint classes.
const CLASH = `${HEAD}ex:Cat owl:disjointWith ex:Dog .\nex:tom a ex:Cat , ex:Dog .\n`
const CALM = `${HEAD}ex:Cat owl:disjointWith ex:Dog .\nex:tom a ex:Cat .\n`
const ON = { reasoning: { rules: "default", diagnostics: true } }
// The test client answers window/workDoneProgress/create with null (success).
const PROGRESS = { window: { workDoneProgress: true } }
const TITLE = "TurbOWL: reasoning…"
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

const turbowl = (diags) => diags.filter((d) => String(d.code).startsWith("turbowl/") || d.code === "reasoning-skipped")

async function session(files, opts = {}) {
  const dir = tempDir("turtle-reasoning-")
  for (const [f, text] of Object.entries(files)) fs.writeFileSync(path.join(dir, f), text)
  const s = await startServer({ cwd: dir, ...opts })
  // Requests wait for the initial index, so a run has been scheduled.
  await s.request("workspace/symbol", { query: "" })
  return {
    s,
    dir,
    uri: (f) => uriOf(path.join(dir, f)),
    async end() {
      await s.stop()
      fs.rmSync(dir, { recursive: true, force: true })
    },
  }
}

// Wait until the published diagnostics of `uri` satisfy `ok`.
async function until(s, uri, ok, ms = 10000) {
  const end = Date.now() + ms
  for (;;) {
    const d = await s.diagnosticsFor(uri, ms)
    if (ok(d)) return d
    if (Date.now() > end) throw new Error(`timed out; last diagnostics: ${JSON.stringify(d, null, 1)}`)
    await sleep(25)
  }
}

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

test("a disjointness contradiction is a diagnostic with its proof, on the member's line only", async () => {
  // On disk the file is consistent: opening it reasons over the unsaved text.
  const w = await session({ "a.ttl": CALM }, { initializationOptions: ON })
  const a = w.uri("a.ttl")
  try {
    w.s.open(a, CLASH)
    const found = turbowl(await until(w.s, a, (d) => turbowl(d).length > 0))
    assert.deepEqual(found.map((d) => d.code), ["turbowl/cax-dw"], JSON.stringify(found))
    // On the member, not on the class axiom (line 3): that is in the proof.
    assert.deepEqual(found.map((d) => d.range.start.line), [3])
    for (const d of found) {
      assert.equal(d.severity, 1)
      // The client shows no related information: the proof's triples are
      // lines of the message, each with its file and line.
      assert.equal(
        d.message,
        "ex:tom is both a ex:Cat and a ex:Dog, which are disjoint.\n" +
          "a.ttl:3  ex:Cat owl:disjointWith ex:Dog\n" +
          "a.ttl:4  ex:tom a ex:Cat\n" +
          "a.ttl:4  ex:tom a ex:Dog",
      )
      assert.equal(d.relatedInformation, undefined)
    }
  } finally {
    await w.end()
  }
})

test("with related information, each triple of the proof is a location; a list is one", async () => {
  const text =
    `${HEAD}[] a owl:AllDisjointClasses ;\n  owl:members ( ex:Cat ex:Dog ex:Fish ) .\n` +
    "ex:tom a ex:Cat ,\n  ex:Dog .\n"
  const w = await session(
    { "a.ttl": text },
    {
      initializationOptions: ON,
      capabilities: { textDocument: { publishDiagnostics: { relatedInformation: true } } },
    },
  )
  const a = w.uri("a.ttl")
  try {
    w.s.open(a, text)
    const found = turbowl(await until(w.s, a, (d) => turbowl(d).length > 0))
    const d = found.find((d) => d.range.start.line === 4)
    assert.ok(d, JSON.stringify(found))
    assert.equal(d.code, "turbowl/cax-adc")
    assert.equal(d.message, "ex:tom is both a ex:Cat and a ex:Dog, which are disjoint.")
    assert.deepEqual(
      d.relatedInformation.map((r) => [r.location.uri, r.location.range.start.line, r.message]),
      [
        [a, 4, "ex:tom a ex:Cat"],
        // A triple's place starts at its predicate.
        [a, 4, "ex:tom a ex:Dog"],
        [a, 2, "[] a owl:AllDisjointClasses ; owl:members ( ex:Cat ex:Dog ex:Fish )"],
      ],
    )
  } finally {
    await w.end()
  }
})

test("an edit updates the findings once typing pauses; editing back clears them", async () => {
  const w = await session({ "a.ttl": CALM }, { initializationOptions: ON })
  const a = w.uri("a.ttl")
  try {
    w.s.open(a, CALM)
    assert.deepEqual(turbowl(await w.s.diagnosticsFor(a)), [])
    // Let the run after open finish.
    await sleep(300)
    w.s.change(a, CLASH)
    const typed = performance.now()
    // The diagnostics of the edit itself come at once, without findings.
    assert.deepEqual(turbowl(await w.s.diagnosticsFor(a)), [])
    await until(w.s, a, (d) => turbowl(d).length > 0)
    const shown = w.s.publications.find((p) => p.uri === a && p.at > typed && turbowl(p.diagnostics).length > 0)
    assert.ok(shown.at - typed > 800, `reasoned ${(shown.at - typed).toFixed(0)} ms after the edit`)
    w.s.change(a, CALM)
    await until(w.s, a, (d) => turbowl(d).length === 0)
  } finally {
    await w.end()
  }
})

test("findings in a file that is not open show in workspace diagnostics", async () => {
  const w = await session(
    { "a.ttl": CALM, "b.ttl": `${HEAD}ex:rex a ex:Cat , ex:Dog .\n` },
    { initializationOptions: { ...ON, diagnostics: { workspace: true } } },
  )
  const b = w.uri("b.ttl")
  try {
    const found = turbowl(await until(w.s, b, (d) => turbowl(d).length > 0))
    assert.ok(found.every((d) => d.code === "turbowl/cax-dw"), JSON.stringify(found))
    assert.deepEqual(found.map((d) => d.range.start.line), [2])
  } finally {
    await w.end()
  }
})

test("in an N-Triples file the findings are named with the project's prefixes.ttl", async () => {
  const type = "<http://www.w3.org/1999/02/22-rdf-syntax-ns#type>"
  const nt =
    `<http://example.org/tom> ${type} <http://example.org/Cat> .\n` +
    `<http://example.org/tom> ${type} <http://example.org/Dog> .\n`
  const w = await session(
    {
      "onto.ttl": `${HEAD}ex:Cat owl:disjointWith ex:Dog .\n`,
      "prefixes.ttl": "@prefix ex: <http://example.org/> .\n",
      "data.nt": nt,
    },
    { initializationOptions: ON },
  )
  const a = w.uri("data.nt")
  try {
    w.s.open(a, nt)
    const found = turbowl(await until(w.s, a, (d) => turbowl(d).length > 0))
    assert.ok(found.length > 0, JSON.stringify(found))
    for (const d of found) {
      assert.equal(d.code, "turbowl/cax-dw")
      assert.ok(d.message.startsWith("ex:tom is both a ex:Cat and a ex:Dog, which are disjoint.\n"), d.message)
      assert.ok(d.message.includes("onto.ttl:3  ex:Cat owl:disjointWith ex:Dog"), d.message)
      assert.ok(d.message.includes("data.nt:1  ex:tom a ex:Cat"), d.message)
      assert.ok(!d.message.includes("<http"), d.message)
    }
  } finally {
    await w.end()
  }
})

test("over turtle.reasoning.maxTriples, the run is skipped with a note", async () => {
  const w = await session({ "a.ttl": CLASH }, { initializationOptions: { reasoning: { rules: "default", maxTriples: 1 } } })
  const a = w.uri("a.ttl")
  try {
    w.s.open(a, CLASH)
    const found = turbowl(await until(w.s, a, (d) => turbowl(d).length > 0))
    assert.equal(found.length, 1, JSON.stringify(found))
    assert.equal(found[0].code, "reasoning-skipped")
    assert.equal(found[0].severity, 3)
    assert.match(found[0].message, /turtle\.reasoning\.maxTriples/)
  } finally {
    await w.end()
  }
})

// A run shows progress only once it has lasted 200 ms; with
// TURTLE_LSP_REASONING_PROGRESS_NOW=1 the server shows it at once, so the
// test does not depend on how long a run takes.
test("progress: a run shows TurbOWL's progress, which ends with a count", async () => {
  process.env.TURTLE_LSP_REASONING_PROGRESS_NOW = "1"
  let w
  try {
    w = await session({ "a.ttl": CLASH }, { initializationOptions: ON, capabilities: PROGRESS })
  } finally {
    delete process.env.TURTLE_LSP_REASONING_PROGRESS_NOW
  }
  const a = w.uri("a.ttl")
  try {
    w.s.open(a, CLASH)
    await until(w.s, a, (d) => turbowl(d).length > 0)
    const ended = () => [...progressByToken(w.s).values()].filter((p) => p.title === TITLE && p.end)
    for (let i = 0; i < 100 && ended().length === 0; i++) await sleep(20)
    const runs = ended()
    assert.ok(runs.length > 0, JSON.stringify(w.s.progress))
    assert.ok(runs.some((p) => /^\d+ errors? in \d+ files?$/.test(p.end)), JSON.stringify(runs))
  } finally {
    await w.end()
  }
})

test("a SPARQL file's open, edits and close start no run; an RDF file's edit does", async () => {
  process.env.TURTLE_LSP_REASONING_PROGRESS_NOW = "1"
  let w
  try {
    w = await session({ "a.ttl": CLASH }, { initializationOptions: ON, capabilities: PROGRESS })
  } finally {
    delete process.env.TURTLE_LSP_REASONING_PROGRESS_NOW
  }
  const a = w.uri("a.ttl")
  const q = w.uri("q.rq")
  const runs = () => [...progressByToken(w.s).values()].filter((p) => p.title === TITLE)
  const settled = async () => {
    for (let i = 0; i < 250 && runs().some((p) => !p.end); i++) await sleep(20)
    assert.ok(runs().every((p) => p.end), JSON.stringify(runs()))
  }
  try {
    w.s.open(a, CLASH)
    await until(w.s, a, (d) => turbowl(d).length > 0)
    // Opening a.ttl can start a second run after the first one's findings
    // land (a slow runner sees it late): wait past the pause in typing until
    // no new run starts, so the count below is only the SPARQL file's.
    let before = -1
    while (runs().length !== before) {
      before = runs().length
      await settled()
      await sleep(1500)
    }
    w.s.open(q, "SELECT * WHERE { ?s ?p ?o }", "sparql")
    w.s.change(q, "ASK { ?s ?p ?o }")
    w.s.close(q)
    // Past the pause in typing that would start a run.
    await sleep(1500)
    assert.equal(runs().length, before, JSON.stringify(runs()))
    w.s.change(a, CALM)
    await until(w.s, a, (d) => turbowl(d).length === 0)
    assert.ok(runs().length > before, JSON.stringify(runs()))
    // No progress is left on.
    await settled()
  } finally {
    await w.end()
  }
})

test("by default (reasoning.diagnostics off) a run publishes no findings; turning it on shows them", async () => {
  const w = await session({ "a.ttl": CLASH }, { initializationOptions: { reasoning: { rules: "default" } } })
  const a = w.uri("a.ttl")
  const diagnostics = (on) =>
    w.s.notify("workspace/didChangeConfiguration", { settings: { turtle: { reasoning: { rules: "default", diagnostics: on } } } })
  try {
    w.s.open(a, CLASH)
    await sleep(1500)
    assert.deepEqual(turbowl(await w.s.diagnosticsFor(a)), [])
    diagnostics(true)
    await until(w.s, a, (d) => turbowl(d).length > 0)
    diagnostics(false)
    await until(w.s, a, (d) => turbowl(d).length === 0)
  } finally {
    await w.end()
  }
})

test("turning reasoning on and off in the settings shows and clears the findings", async () => {
  const w = await session({ "a.ttl": CLASH })
  const a = w.uri("a.ttl")
  const rules = (r) => w.s.notify("workspace/didChangeConfiguration", { settings: { turtle: { reasoning: { rules: r, diagnostics: true } } } })
  try {
    w.s.open(a, CLASH)
    assert.deepEqual(turbowl(await w.s.diagnosticsFor(a)), [])
    rules("complete")
    await until(w.s, a, (d) => turbowl(d).length > 0)
    rules("off")
    await until(w.s, a, (d) => turbowl(d).length === 0)
  } finally {
    await w.end()
  }
})

test("off: the diagnostics are exactly those without reasoning, and nothing runs", async () => {
  // A lint finding besides the clash: ex:p is used and described nowhere.
  const text = `${CLASH}ex:tom ex:p 1 .\n`
  const off = await session({ "a.ttl": text }, { capabilities: PROGRESS })
  const on = await session({ "a.ttl": text }, { capabilities: PROGRESS, initializationOptions: ON })
  try {
    off.s.open(off.uri("a.ttl"), text)
    on.s.open(on.uri("a.ttl"), text)
    const withOn = await until(on.s, on.uri("a.ttl"), (d) => turbowl(d).length > 0)
    // Long enough for a run to have started and ended.
    await sleep(1500)
    const withOff = await off.s.diagnosticsFor(off.uri("a.ttl"))
    assert.deepEqual(turbowl(withOff), [])
    assert.deepEqual(
      withOff,
      withOn.filter((d) => !turbowl([d]).length),
    )
    assert.ok(![...progressByToken(off.s).values()].some((p) => p.title === TITLE), JSON.stringify(off.s.progress))
  } finally {
    await off.end()
    await on.end()
  }
})
