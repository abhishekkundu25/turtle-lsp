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

// The label checks compare terms across files, and the opt-in ones wait for
// the initial index: the open files settle on what check reports, with
// turtle-lsp.json's lint.ignore and lint.enable applied by both.
const LABELS = {
  "turtle-lsp.json": JSON.stringify({ lint: { ignore: ["unused-prefix"], enable: ["missing-ontology-license"] } }),
  "core.ttl": `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .

ex: a owl:Ontology ;
    rdfs:label "Example" .

ex:Pump a owl:Class ;
    rdfs:label "Pump"@en .

ex:Valve a owl:Class ;
    rdfs:label "Valve"@en, "Tap"@en .

ex:Pipe a owl:Class ;
    rdfs:label " Pipe"@en .

ex:Old a owl:Class ;
    rdfs:label "Old"@en ;
    owl:deprecated true .

ex:Hose a owl:Class ;
    rdfs:label "Hose"@en ;
    rdfs:subClassOf ex:Old .
`,
  "other.ttl": `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

ex:Pump2 a owl:Class ;
    rdfs:label "pump"@en .
`,
}

test("an open file shows the label checks check reports, with lint.ignore and lint.enable", async (t) => {
  const dir = workspace(t, LABELS)
  const expected = check(dir)
  const codes = new Set(
    Object.values(expected)
      .flat()
      .map((l) => l.match(/\[([^\]]*)\]/)[1]),
  )
  for (const code of [
    "duplicate-label",
    "label-whitespace",
    "multiple-labels",
    "deprecated-term",
    "missing-ontology-license",
  ]) {
    assert.ok(codes.has(code), `${code} in ${JSON.stringify(expected)}`)
  }
  // The unused foaf: prefix is ignored by the file.
  assert.ok(!codes.has("unused-prefix"), JSON.stringify(expected))
  // The duplicate is reported in both files; the own deprecation is a warning.
  assert.ok(expected["other.ttl"].some((l) => l.includes("warning[duplicate-label]")), expected["other.ttl"])
  assert.ok(expected["core.ttl"].some((l) => l.includes("warning[deprecated-term]")), expected["core.ttl"])

  const s = await session(t, dir)
  for (const name of ["core.ttl", "other.ttl"]) {
    s.open(uri(dir, name), fs.readFileSync(path.join(dir, name), "utf8"), "turtle")
  }
  for (const name of ["core.ttl", "other.ttl"]) {
    const u = uri(dir, name)
    const same = (d, p) => p.version === 1 && JSON.stringify(fromServer(d)) === JSON.stringify(expected[name])
    await until(s, u, same).catch(() => {})
    assert.deepEqual(fromServer(latest(s, u)?.diagnostics ?? []), expected[name], name)
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
  // term to report: the pass takes about a second.
  let vocab = "ex:next a rdf:Property .\n"
  for (let j = 0; j < 7; j++) vocab += `ex:Thing${j} a rdfs:Class .\n`
  const files = { "prefixes.ttl": PREFIXES, "vocab.ttl": vocab }
  for (let i = 0; i < 300; i++) {
    let text = ""
    for (let j = 0; j < 400; j++) text += `ex:s${i}_${j} a ex:Thing${j % 7} ; rdfs:label "s ${j}" ; ex:next ex:s${i}_${j + 1} .\n`
    files[`gen/f${i}.ttl`] = `${text}ex:s${i}_0 rdfs:lable "typo" .\n`
  }
  const dir = workspace(t, files)
  const edited = uri(dir, "gen/f0.ttl")
  const text = files["gen/f0.ttl"]
  const closed = (p) => p.version == null && p.uri !== edited
  const distinct = (pubs) => new Set(pubs.filter(closed).map((p) => p.uri)).size
  // Ordered, not timed (how fast a hover is answered: latency.test.mjs).
  // Whether the pass was still running when the server answered the first
  // hover is read from the order of the messages: the closed files published
  // before that answer. If the pass had already finished (a client too slow
  // to see it start), the session proves nothing and a new one is tried.
  for (let attempt = 1; ; attempt++) {
    const s = await session(t, dir, ON)
    s.open(edited, text)
    await until(s, edited, (_, p) => p.version === 1)
    // The pass is under way.
    const end = Date.now() + 20000
    while (!s.publications.some(closed)) {
      if (Date.now() > end) throw new Error("no background diagnostics")
      await sleep(5)
    }
    let beforeAnswer
    for (let i = 0; i < 5; i++) {
      s.change(edited, `${text}ex:extra${i} a ex:Thing0 .\n`)
      const hover = s.request("textDocument/hover", { textDocument: { uri: edited }, position: { line: 0, character: 3 } })
      // Every publication that came before the answer is recorded by now.
      if (i === 0) await hover.then(() => (beforeAnswer = distinct(s.publications)))
      assert.ok(await hover, `no hover ${i}`)
    }
    await until(s, edited, (_, p) => p.version === 6)
    if (beforeAnswer < 299) {
      // The pass resumes and reaches every closed file.
      const all = Date.now() + 30000
      while (distinct(s.publications) < 299) {
        if (Date.now() > all) throw new Error(`the pass never finished (${distinct(s.publications)} files)`)
        await sleep(20)
      }
      return
    }
    assert.ok(attempt < 3, "in 3 sessions the pass was over before the first keystroke was answered")
    await s.stop()
    servers.get(dir).splice(servers.get(dir).indexOf(s), 1)
  }
})
