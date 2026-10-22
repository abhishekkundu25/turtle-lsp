// Workspace diagnostics (`diagnostics.workspace`): files that are not open get
// the diagnostics `turtle-lsp check` reports for them, pushed in the
// background; opening a file hands it to live diagnostics and closing it hands
// it back. Each test runs on a temporary workspace written by the test.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, startServer, tempDir, uriOf } from "./lsp-client.mjs"

const PREFIXES = `@prefix ex: <http://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
const SHAPES = `ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] .

ex:Person a rdfs:Class .

ex:name a rdf:Property .
`
const CLEAN = `ex:bob a ex:Person ;
    ex:name "Bob" .
`
const FILES = {
  "prefixes.ttl": PREFIXES,
  "shapes.ttl": SHAPES,
  "data/clean.ttl": CLEAN,
  // A SHACL violation: no ex:name.
  "data/people.ttl": `${CLEAN}\nex:alice a ex:Person .\n`,
  "data/broken.ttl": "ex:a ex:b ex:c .\nex:d ex:e ex:f\n",
  "data/undeclared.ttl": '<http://x/a> foaf:name "A" .\n',
  "data/typo.ttl": 'ex:bob rdfs:lable "Bob" .\n',
  "queries/q.rq": "SELECT * WHERE { ?s nope:p ?o }\n",
}
const ON = { diagnostics: { workspace: true } }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// The servers started in each workspace. Hooks run in the order they are
// registered, and Windows cannot remove a folder a running server has open, so
// one hook stops them and then removes the folder.
const servers = new Map()

function workspace(t, files = FILES) {
  const dir = tempDir("turtle-wsdiag-")
  servers.set(dir, [])
  t.after(async () => {
    for (const s of servers.get(dir)) await s.stop()
    servers.delete(dir)
    fs.rmSync(dir, { recursive: true, force: true })
  })
  for (const [name, text] of Object.entries(files)) write(dir, name, text)
  return dir
}

function write(dir, name, text) {
  fs.mkdirSync(path.dirname(path.join(dir, name)), { recursive: true })
  fs.writeFileSync(path.join(dir, name), text)
}

async function session(t, dir, initializationOptions) {
  const s = await startServer({ cwd: dir, initializationOptions })
  servers.get(dir).push(s)
  return s
}

const uri = (dir, name) => uriOf(path.join(dir, name))

// The diagnostics last published for `u` (undefined if none were).
function latest(s, u) {
  for (let i = s.publications.length - 1; i >= 0; i--) if (s.publications[i].uri === u) return s.publications[i]
}

async function until(s, u, pred, timeoutMs = 10000) {
  const end = Date.now() + timeoutMs
  for (;;) {
    const p = latest(s, u)
    if (p && pred(p.diagnostics, p)) return p.diagnostics
    if (Date.now() > end) {
      throw new Error(`diagnostics for ${u} never matched; last: ${JSON.stringify(p?.diagnostics)}`)
    }
    await sleep(20)
  }
}

const SEVERITY = { 1: "error", 2: "warning", 3: "info", 4: "hint" }

// Comparable lines: the server's 0-based UTF-16 positions (the texts are
// ASCII) as check's 1-based columns.
function fromServer(diags) {
  return diags
    .map((d) => {
      const { start: a, end: b } = d.range
      return `${a.line + 1}:${a.character + 1}-${b.line + 1}:${b.character + 1} ${SEVERITY[d.severity]}[${d.code ?? ""}] ${d.message}`
    })
    .sort()
}

function fromCheck(diags) {
  return diags
    .map((d) => {
      const { start: a, end: b } = d.range
      return `${a.line}:${a.column}-${b.line}:${b.column} ${d.severity}[${d.code ?? ""}] ${d.message}`
    })
    .sort()
}

// `turtle-lsp check --format json` on `dir`: {relative path: lines}.
function check(dir) {
  const r = spawnSync(serverPath, ["check", "--format", "json"], {
    cwd: dir,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(dir, ".cache"), NO_COLOR: "1" },
  })
  const report = JSON.parse(r.stdout)
  return Object.fromEntries(report.files.map((f) => [f.path, fromCheck(f.diagnostics)]))
}

test("files that are not open get the diagnostics check reports", async (t) => {
  const dir = workspace(t)
  const expected = check(dir)
  // Every kind of finding is there: syntax, prefix, unknown term, SHACL, SPARQL.
  assert.deepEqual(Object.keys(expected).sort(), [
    "data/broken.ttl",
    "data/people.ttl",
    "data/typo.ttl",
    "data/undeclared.ttl",
    "queries/q.rq",
  ])
  assert.ok(expected["data/people.ttl"].some((l) => l.includes("[shacl-")), expected["data/people.ttl"])
  const s = await session(t, dir, ON)
  for (const [name, lines] of Object.entries(expected)) {
    const u = uri(dir, name)
    // SHACL results arrive after the first pass.
    await until(s, u, (d) => fromServer(d).length === lines.length).catch(() => {})
    assert.deepEqual(fromServer(latest(s, u)?.diagnostics ?? []), lines, name)
  }
  // Clean files are not published at all.
  for (const name of ["prefixes.ttl", "shapes.ttl", "data/clean.ttl"]) {
    assert.equal(latest(s, uri(dir, name)), undefined, name)
  }
  assert.ok(s.publications.every((p) => p.version == null))
})

test("an open file shows what check reports for it (one composer for editor and check)", async (t) => {
  const dir = workspace(t)
  const expected = check(dir)
  const s = await session(t, dir, ON)
  for (const name of ["data/broken.ttl", "data/undeclared.ttl", "data/typo.ttl", "queries/q.rq"]) {
    const u = uri(dir, name)
    const text = fs.readFileSync(path.join(dir, name), "utf8")
    s.open(u, text, name.endsWith(".rq") ? "sparql" : "turtle")
    await until(s, u, (_, p) => p.version === 1)
    assert.ok(expected[name].length > 0, name)
    assert.deepEqual(fromServer(latest(s, u).diagnostics), expected[name], name)
  }
})

test("opening a file switches it to live diagnostics; closing hands it back", async (t) => {
  const dir = workspace(t)
  const s = await session(t, dir, ON)
  const broken = uri(dir, "data/broken.ttl")
  const syntax = (d) => d.some((x) => x.code === "syntax")
  await until(s, broken, syntax)

  // Open with the error fixed (unsaved): the live diagnostics replace them.
  s.open(broken, "ex:a ex:b ex:c .\nex:d ex:e ex:f .\n")
  await until(s, broken, (d, p) => p.version === 1 && !syntax(d))
  s.change(broken, "ex:a ex:b ex:c .\nex:d ex:e ex:f . ex:g\n")
  await until(s, broken, (d, p) => p.version === 2 && syntax(d))
  s.change(broken, "ex:a ex:b ex:c .\n")
  await until(s, broken, (d, p) => p.version === 3 && d.length === 0)
  // A later pass leaves the open file alone.
  await sleep(800)
  assert.equal(latest(s, broken).version, 3)

  // Closed without saving: the file on disk is diagnosed again, not cleared.
  s.close(broken)
  const back = await until(s, broken, (d, p) => p.version == null)
  assert.ok(syntax(back), JSON.stringify(back))
  assert.deepEqual(fromServer(back), check(dir)["data/broken.ttl"])
})

test("a file changed on disk is diagnosed again; a deleted one is cleared", async (t) => {
  const dir = workspace(t)
  const s = await session(t, dir, ON)
  const typo = uri(dir, "data/typo.ttl")
  const clean = uri(dir, "data/clean.ttl")
  const undeclared = uri(dir, "data/undeclared.ttl")
  await until(s, typo, (d) => d.length > 0)
  await until(s, undeclared, (d) => d.length > 0)

  write(dir, "data/clean.ttl", `${CLEAN}ex:carol ex:name "Carol"\n`)
  write(dir, "data/typo.ttl", 'ex:bob rdfs:label "Bob" .\n')
  fs.rmSync(path.join(dir, "data/undeclared.ttl"))
  s.notify("workspace/didChangeWatchedFiles", {
    changes: [
      { uri: clean, type: 2 },
      { uri: typo, type: 2 },
      { uri: undeclared, type: 3 },
    ],
  })
  const d = await until(s, clean, (d) => d.some((x) => x.code === "syntax"))
  assert.deepEqual(fromServer(d), check(dir)["data/clean.ttl"])
  await until(s, typo, (d) => d.length === 0)
  await until(s, undeclared, (d) => d.length === 0)
})

test("turning the setting off clears every closed file it reported", async (t) => {
  const dir = workspace(t)
  const s = await session(t, dir, ON)
  const names = ["data/broken.ttl", "data/typo.ttl", "data/undeclared.ttl", "queries/q.rq"]
  for (const n of names) await until(s, uri(dir, n), (d) => d.length > 0)
  s.notify("workspace/didChangeConfiguration", { settings: { turtle: { diagnostics: { workspace: false } } } })
  for (const n of names) await until(s, uri(dir, n), (d) => d.length === 0)
  // And nothing comes back.
  const count = s.publications.length
  await sleep(800)
  assert.equal(s.publications.length, count)

  // On again: back they come.
  s.notify("workspace/didChangeConfiguration", { settings: { turtle: { diagnostics: { workspace: true } } } })
  for (const n of names) await until(s, uri(dir, n), (d) => d.length > 0)
})

test("with the setting off (the default) nothing is published for closed files", async (t) => {
  const dir = workspace(t)
  const s = await session(t, dir)
  // Answered once the workspace is indexed.
  await s.request("workspace/symbol", { query: "bob" })
  await sleep(1000)
  assert.deepEqual(s.publications, [])
  // Closing a file still clears it.
  const broken = uri(dir, "data/broken.ttl")
  s.open(broken, FILES["data/broken.ttl"])
  await until(s, broken, (d, p) => p.version === 1 && d.length > 0)
  s.close(broken)
  await until(s, broken, (d, p) => p.version == null && d.length === 0)
})

test("a keystroke is answered while the background pass runs", async (t) => {
  // A few hundred files over a described vocabulary, each with an unknown
  // term to report.
  let vocab = "ex:next a rdf:Property .\n"
  for (let j = 0; j < 7; j++) vocab += `ex:Thing${j} a rdfs:Class .\n`
  const files = { "prefixes.ttl": PREFIXES, "vocab.ttl": vocab }
  for (let i = 0; i < 300; i++) {
    let text = ""
    for (let j = 0; j < 400; j++) text += `ex:s${i}_${j} a ex:Thing${j % 7} ; rdfs:label "s ${j}" ; ex:next ex:s${i}_${j + 1} .\n`
    files[`gen/f${i}.ttl`] = `${text}ex:s${i}_0 rdfs:lable "typo" .\n`
  }
  const dir = workspace(t, files)
  const s = await session(t, dir, ON)
  const edited = uri(dir, "gen/f0.ttl")
  const text = files["gen/f0.ttl"]
  s.open(edited, text)
  await until(s, edited, (_, p) => p.version === 1)
  const closed = (p) => p.version == null && p.uri !== edited
  // The pass is under way.
  const end = Date.now() + 20000
  while (!s.publications.some(closed)) {
    if (Date.now() > end) throw new Error("no background diagnostics")
    await sleep(5)
  }
  const before = s.publications.filter(closed).length
  const answered = []
  for (let i = 0; i < 5; i++) {
    s.change(edited, `${text}ex:extra${i} a ex:Thing0 .\n`)
    const t0 = performance.now()
    await s.request("textDocument/hover", { textDocument: { uri: edited }, position: { line: 0, character: 3 } })
    answered.push(performance.now() - t0)
  }
  await until(s, edited, (_, p) => p.version === 6, 2000)
  assert.ok(Math.max(...answered) < 200, `hover took ${answered.map((x) => x.toFixed(0))} ms`)
  assert.ok(before < 299, `the pass was over before the keystrokes (${before} files)`)
  // The pass resumes and reaches every closed file.
  const all = Date.now() + 30000
  while (new Set(s.publications.filter(closed).map((p) => p.uri)).size < 299) {
    if (Date.now() > all) throw new Error("the pass never finished")
    await sleep(20)
  }
})
