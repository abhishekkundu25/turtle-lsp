// How fast the server answers: the wall-clock checks of the end-to-end
// suite, kept in this one file. `npm run test:node` runs it after the other
// files, on its own (--test-concurrency=1), so no other test file competes
// for the machine; its tests run one after another. The other files check
// the same behaviour by counts and message order, with no clock.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures, positionOf, serverPath } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const ms = (t0) => performance.now() - t0

// A temporary workspace with `files`, removed with its server.
async function session(t, files = {}, options = {}) {
  const dir = tempDir("turtle-latency-")
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, name)), { recursive: true })
    fs.writeFileSync(path.join(dir, name), text)
  }
  const s = await startServer({ cwd: dir, ...options })
  t.after(async () => {
    try {
      await s.stop()
    } finally {
      fs.rmSync(dir, { recursive: true, force: true })
    }
  })
  return { s, dir, at: (name) => uriOf(path.join(dir, name)) }
}

test("initialize answers within 500 ms", async () => {
  const s = await startServer({ cwd: path.join(fixtures, "workspace") })
  try {
    assert.ok(s.initMs < 500, `initialize took ${s.initMs.toFixed(0)} ms`)
  } finally {
    await s.stop()
  }
})

// ---- queries (query-lsp.test.mjs) ----

const DATA = `@prefix ex: <http://example.org/> .
ex:alice ex:name "Alice" ; ex:age 30 .
`
// A cross product of the data: far too slow to finish.
const SLOW = "SELECT (COUNT(*) AS ?n) WHERE { ?a ?p ?b . ?c ?q ?d . ?e ?r ?f . ?g ?h ?i }\n"
const chain = (n) => {
  let big = "@prefix ex: <http://example.org/> .\n"
  for (let i = 0; i < n; i++) big += `ex:n${i} ex:next ex:n${i + 1} .\n`
  return big
}
const QUERY_FILES = { "people.ttl": DATA, "slow.rq": SLOW, "big.ttl": chain(400) }
const runQuery = (s, uri) => s.request("workspace/executeCommand", { command: "turtle.runQuery", arguments: [uri] })

test("a hover is answered within 200 ms while a query runs; the query stops within 1 s of its limit", async (t) => {
  const { s, at } = await session(t, QUERY_FILES, { initializationOptions: { query: { timeoutSeconds: 1 } } })
  s.open(at("people.ttl"), DATA)
  await s.diagnosticsFor(at("people.ttl"))
  const t0 = performance.now()
  const slow = runQuery(s, at("slow.rq"))
  await sleep(200)
  for (let i = 0; i < 3; i++) {
    const h0 = performance.now()
    assert.ok(await s.request("textDocument/hover", { textDocument: { uri: at("people.ttl") }, position: positionOf(DATA, "ex:alice", 4) }))
    assert.ok(ms(h0) < 200, `hover took ${ms(h0).toFixed(0)} ms while the query ran`)
  }
  assert.equal(await slow, "stopped at the time limit (1 s, query.timeoutSeconds)")
  assert.ok(ms(t0) < 2000, `the query ran ${ms(t0).toFixed(0)} ms`)
})

test("a replaced run ends within 2 s, a cancelled one within 4 s (limit 20 s)", async (t) => {
  const { s, at } = await session(t, QUERY_FILES, { initializationOptions: { query: { timeoutSeconds: 20 } } })
  const t0 = performance.now()
  const first = runQuery(s, at("slow.rq"))
  await sleep(100)
  const second = runQuery(s, at("slow.rq"))
  assert.equal(await first, "cancelled: a newer run replaced it")
  assert.ok(ms(t0) < 2000, `the replaced run ended after ${ms(t0).toFixed(0)} ms`)
  s.sendRaw([{ id: "raw-l1", method: "workspace/executeCommand", params: { command: "turtle.runQuery", arguments: [at("slow.rq")] } }])
  assert.equal(await second, "cancelled: a newer run replaced it")
  await sleep(100)
  s.sendRaw([{ method: "$/cancelRequest", params: { id: "raw-l1" } }])
  const resp = await s.rawResponse("raw-l1", 25000)
  assert.equal(resp.error?.code, -32800, JSON.stringify(resp))
  assert.ok(ms(t0) < 4000, `the cancelled run ended after ${ms(t0).toFixed(0)} ms`)
})

// ---- SHACL validation (shacl-validate.test.mjs) ----

test("hovers are answered within 200 ms while a validation runs", async (t) => {
  // Every node of an 8,000-node chain is a focus node with an ex:next* path.
  let data = "@prefix ex: <http://example.com/chain/> .\n"
  for (let i = 0; i < 8000; i++) data += `ex:n${i} ex:next ex:n${i + 1} .\n`
  const shapes = `@prefix ex: <http://example.com/chain/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
ex:ChainShape sh:targetSubjectsOf ex:next ;
  sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:minCount 1 ] .
`
  const { s, dir, at } = await session(t, { "chain.ttl": data, "chain-shapes.ttl": shapes, "a.ttl": DATA })
  await s.request("workspace/symbol", { query: "" })
  s.open(at("a.ttl"), DATA)
  await s.diagnosticsFor(at("a.ttl"))
  fs.writeFileSync(path.join(dir, "a.ttl"), DATA)
  s.notify("textDocument/didSave", { textDocument: { uri: at("a.ttl") } })
  const answered = []
  for (let i = 0; i < 5; i++) {
    const h0 = performance.now()
    await s.request("textDocument/hover", { textDocument: { uri: at("a.ttl") }, position: { line: 1, character: 3 } })
    answered.push(ms(h0))
  }
  assert.ok(Math.max(...answered) < 200, `hover took ${answered.map((x) => x.toFixed(0))} ms`)
})

// ---- workspace diagnostics (workspace-diagnostics.test.mjs) ----

test("a hover is answered within 200 ms while the background pass runs", async (t) => {
  let vocab = "ex:next a rdf:Property .\n"
  for (let j = 0; j < 7; j++) vocab += `ex:Thing${j} a rdfs:Class .\n`
  const files = {
    "prefixes.ttl": `@prefix ex: <http://example.org/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`,
    "vocab.ttl": vocab,
  }
  for (let i = 0; i < 300; i++) {
    let text = ""
    for (let j = 0; j < 400; j++) text += `ex:s${i}_${j} a ex:Thing${j % 7} ; rdfs:label "s ${j}" ; ex:next ex:s${i}_${j + 1} .\n`
    files[`gen/f${i}.ttl`] = `${text}ex:s${i}_0 rdfs:lable "typo" .\n`
  }
  const { s, at } = await session(t, files, { initializationOptions: { diagnostics: { workspace: true } } })
  const edited = at("gen/f0.ttl")
  const text = files["gen/f0.ttl"]
  s.open(edited, text)
  await s.diagnosticsFor(edited)
  const closed = (p) => p.version == null && p.uri !== edited
  const end = Date.now() + 20000
  while (!s.publications.some(closed)) {
    if (Date.now() > end) throw new Error("no background diagnostics")
    await sleep(5)
  }
  const answered = []
  for (let i = 0; i < 5; i++) {
    s.change(edited, `${text}ex:extra${i} a ex:Thing0 .\n`)
    const h0 = performance.now()
    await s.request("textDocument/hover", { textDocument: { uri: edited }, position: { line: 0, character: 3 } })
    answered.push(ms(h0))
  }
  assert.ok(Math.max(...answered) < 200, `hover took ${answered.map((x) => x.toFixed(0))} ms`)
})

// ---- big documents and queries ----

test("the code lenses of a 30,000-line document come within 1 s", { timeout: 60_000 }, async (t) => {
  // The document of bench/perf.mjs: 5000 subjects of 6 lines (lens.test.mjs).
  let text = `@prefix ex: <http://example.org/> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
  for (let i = 0; i < 5000; i++) {
    text += `
ex:p${i} a foaf:Person ;
    foaf:name "Person ${i}" ;
    rdfs:comment """A longer
multi-line description ${i}""" ;
    foaf:knows ex:p${(i + 1) % 5000} .
`
  }
  const { s, at } = await session(t)
  s.open(at("big.ttl"), text)
  await s.diagnosticsFor(at("big.ttl"), 30_000)
  const t0 = performance.now()
  const ls = await s.request("textDocument/codeLens", { textDocument: { uri: at("big.ttl") } })
  const took = ms(t0)
  assert.equal(ls.length, 5000)
  t.diagnostic(`codeLens (${ls.length} lenses): ${took.toFixed(1)} ms`)
  assert.ok(took < 1000, `codeLens took ${took.toFixed(0)} ms`)
})

test("10,000 uses of 2,000 unknown terms are diagnosed within 1 s", async (t) => {
  // As unknown-term.test.mjs: locals shaped like real foaf names.
  const letters = "abcdefghijklmnopqrstuvwxyz"
  const local = (i) => {
    let s = ""
    for (let n = i, k = 0; k < 6 + (i % 7); k++, n = Math.floor(n / 3) + k * 7) s += letters[(n * 31 + k * 17) % 26]
    return `${s}${i}`
  }
  const lines = []
  for (let i = 0; i < 10_000; i++) lines.push(`ex:s${i % 50} foaf:${local(i % 2000)} "v" .`)
  const text = `@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n@prefix ex: <http://example.org/> .\n${lines.join("\n")}\n`
  const { s, at } = await session(t)
  const t0 = performance.now()
  s.open(at("big.ttl"), text)
  const diags = await s.diagnosticsFor(at("big.ttl"), 30_000)
  const took = ms(t0)
  assert.equal(diags.filter((d) => d.code === "unknown-term").length, 10_000)
  t.diagnostic(`unknown-term: 10,000 uses / 2,000 locals diagnosed in ${took.toFixed(0)} ms`)
  assert.ok(took < 1000, `took ${took.toFixed(0)} ms`)
})

test("a 2,000-pattern query is formatted within 2 s", async (t) => {
  const body = Array.from({ length: 2000 }, (_, i) => `?s${i} <https://e.org/p> ?o${i} .`).join(" ")
  const text = `SELECT * WHERE { ${body} }\n`
  const { s, at } = await session(t)
  s.open(at("long.rq"), text, "sparql")
  await s.diagnosticsFor(at("long.rq"))
  const t0 = performance.now()
  const edits = await s.request("textDocument/formatting", { textDocument: { uri: at("long.rq") }, options: { tabSize: 4, insertSpaces: true } })
  assert.equal(edits.length, 1)
  assert.ok(ms(t0) < 2000, `${ms(t0).toFixed(0)} ms`)
})

test("completion in a 300-pattern query answers within 2 s", async (t) => {
  const body = Array.from({ length: 300 }, (_, i) => `  ?s${i} ex:name ?n${i} .`).join("\n")
  const text = `PREFIX ex: <https://query.example.org/def/>\nSELECT * WHERE {\n${body}\n  ?x ex:\n}\n`
  const { s, at } = await session(t, { "def.ttl": "@prefix ex: <https://query.example.org/def/> .\nex:name a <http://www.w3.org/1999/02/22-rdf-syntax-ns#Property> .\n" })
  await s.request("workspace/symbol", { query: "" })
  s.open(at("long.rq"), text, "sparql")
  await s.diagnosticsFor(at("long.rq"))
  const t0 = performance.now()
  const res = await s.request("textDocument/completion", { textDocument: { uri: at("long.rq") }, position: positionOf(text, "?x ex:") })
  assert.ok(ms(t0) < 2000, `${ms(t0).toFixed(0)} ms`)
  const items = Array.isArray(res) ? res : (res?.items ?? [])
  assert.ok(items.some((i) => i.label === "ex:name"), items.slice(0, 10).map((i) => i.label).join(", "))
})

// ---- workspace lint (lint.test.mjs) ----

test("1,000 undescribed terms against a 30,000-term namespace are diagnosed in under 50 ms", async (t) => {
  const s0 = performance.now()
  const NS = "https://ontology.example.com/big#"
  // As lint.test.mjs: letters-only names of 9-16 characters, all distinct.
  const letters = "abcdefghijklmnopqrstuvwxyz"
  const name = (i) => {
    let s = ""
    let n = i
    for (let k = 0; k < 4; k++, n = Math.floor(n / 26)) s += letters[n % 26]
    for (let k = 0, h = i * 2654435761; k < 5 + (i % 8); k++, h = Math.floor(h / 7) + k * 13) s += letters[(h >>> 0) % 26]
    return s
  }
  const defs = []
  for (let i = 0; i < 30_000; i++) defs.push(`ex:${name(i)} a owl:Class .`)
  const { s, at } = await session(t, {
    "big.ttl": `@prefix ex: <${NS}> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n${defs.join("\n")}\n`,
  })
  await s.request("workspace/symbol", { query: "" })
  // 1,000 distinct names that nothing describes, each one letter off a described name.
  const uses = []
  for (let i = 0; i < 1000; i++) {
    const n = name(i * 29)
    uses.push(`inst:i${i} a ex:${n.slice(0, 6)}${n[6] === "q" ? "z" : "q"}${n.slice(7)} .`)
  }
  const text = `@prefix ex: <${NS}> .\n@prefix inst: <https://ontology.example.com/inst#> .\n${uses.join("\n")}\n`
  const uri = at("use.ttl")
  // Warm up: the first diagnosis of a file also loads the index.
  s.open(uri, text + "\n")
  await s.diagnosticsFor(uri, 30_000)
  let best = Infinity
  let diags
  for (let round = 0; round < 3; round++) {
    const t0 = performance.now()
    s.change(uri, text + `# ${round}\n`)
    diags = await s.diagnosticsFor(uri, 30_000)
    best = Math.min(best, ms(t0))
  }
  assert.equal(diags.filter((d) => d.code === "undescribed-term").length, 1000)
  t.diagnostic(`lint: 1,000 undescribed terms / 30,000 described diagnosed in ${best.toFixed(0)} ms (best of 3)`)
  assert.ok(best < 50, `took ${best.toFixed(0)} ms`)
  assert.ok(ms(s0) < 60_000)
})

// ---- the command line (cli.test.mjs) ----

test("check --statistics counts 20,000 undeclared uses in under a second", (t) => {
  let text = ""
  for (let i = 0; i < 20000; i++) text += `<http://x/s${i}> foaf:name "n${i}" .\n`
  const dir = tempDir("turtle-latency-cli-")
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }))
  fs.mkdirSync(path.join(dir, ".cache"))
  fs.writeFileSync(path.join(dir, "big.ttl"), text)
  const t0 = performance.now()
  const r = spawnSync(serverPath, ["check", "--statistics"], {
    cwd: dir,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(dir, ".cache"), NO_COLOR: "1" },
  })
  const took = ms(t0)
  assert.equal(r.status, 1, r.stderr)
  assert.match(r.stdout, /^ {2}20000 {2}total$/m)
  t.diagnostic(`check --statistics: 20,000 undeclared uses in ${took.toFixed(0)} ms`)
  assert.ok(took < 1000, `took ${took.toFixed(0)} ms`)
})
