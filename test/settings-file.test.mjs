// The workspace settings file `turtle-lsp.json`: a layer under the client's
// settings (defaults < file < initializationOptions < configuration section).
//
// Workspace: a copy of test/fixtures/project plus core/__more.ttl (as in
// lint.test.mjs), so a misspelled sd: term gets an `undescribed-term` warning
// unless `lint.undescribedTerms` is off.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
process.env.NO_PROXY = "127.0.0.1,localhost"
const lint = (diags) => diags.filter((d) => d.code === "undescribed-term")
const typo = "obj:m1 sd:fillsPositon obj:pos-mc2 .\n"

function workspace(settings) {
  const ws = tempDir("turtle-settings-")
  fs.cpSync(path.join(fixtures, "project"), ws, { recursive: true })
  fs.writeFileSync(path.join(ws, "core", "__more.ttl"), "sd:threshold a owl:DatatypeProperty .\nsd:active a owl:DatatypeProperty .\n")
  if (settings !== undefined) fs.writeFileSync(path.join(ws, "turtle-lsp.json"), settings)
  return ws
}

async function until(get, ok, what) {
  let last
  for (let i = 0; i < 200; i++) {
    last = await get()
    if (ok(last)) return last
    await sleep(25)
  }
  assert.fail(`${what}: ${JSON.stringify(last)}`)
}

// Starts a server on `ws`, opens a file with a misspelled term, and returns
// the undescribed-term diagnostics with the session.
async function session(ws, initializationOptions) {
  const s = await startServer({ cwd: ws, initializationOptions })
  await s.request("workspace/symbol", { query: "" })
  const uri = uriOf(path.join(ws, "data", "__typo.ttl"))
  s.open(uri, typo)
  return { s, uri, diags: lint(await s.diagnosticsFor(uri)) }
}

async function done(s, ws) {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
}

test("a setting in turtle-lsp.json changes diagnostics", async () => {
  const ws = workspace(JSON.stringify({ "lint.undescribedTerms": false }))
  const { s, diags } = await session(ws)
  assert.deepEqual(diags, [])
  assert.deepEqual(s.messages, [])
  await done(s, ws)
})

test("the client's settings override turtle-lsp.json", async () => {
  const ws = workspace(JSON.stringify({ turtle: { lint: { undescribedTerms: false } } }))
  const { s, uri, diags } = await session(ws, { lint: { undescribedTerms: true } })
  assert.equal(diags.length, 1, JSON.stringify(diags))
  // The configuration section is the top layer.
  s.notify("workspace/didChangeConfiguration", { settings: { turtle: { lint: { undescribedTerms: false } } } })
  await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 0, "still flagged with the lint off")
  await done(s, ws)
})

test("an invalid turtle-lsp.json warns once and falls back to the defaults", async () => {
  const ws = workspace("{ lint: off")
  const { s, diags } = await session(ws)
  assert.equal(diags.length, 1, JSON.stringify(diags))
  assert.equal(s.messages.length, 1, JSON.stringify(s.messages))
  assert.equal(s.messages[0].type, 2)
  assert.match(s.messages[0].message, /turtle-lsp\.json/)
  await done(s, ws)
})

test("editing turtle-lsp.json on disk applies the change", async () => {
  const ws = workspace()
  const { s, uri, diags } = await session(ws)
  assert.equal(diags.length, 1, JSON.stringify(diags))
  const file = path.join(ws, "turtle-lsp.json")
  fs.writeFileSync(file, JSON.stringify({ lint: { undescribedTerms: false } }))
  s.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: uriOf(file), type: 1 }] })
  await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 0, "still flagged after the file turned the lint off")
  fs.rmSync(file)
  s.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: uriOf(file), type: 3 }] })
  await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 1, "not flagged after the file was deleted")
  assert.deepEqual(s.messages, [])
  await done(s, ws)
})

// parse.incremental: with TURTLE_LSP_CHECK_INCREMENTAL=1 the server logs how
// it parsed each analysis (`incremental: <uri> Spliced`, `... Off`).
test("parse.incremental: false in turtle-lsp.json parses every edit in full; the editor can turn it back on", async () => {
  const ws = tempDir("turtle-settings-")
  fs.writeFileSync(path.join(ws, "turtle-lsp.json"), JSON.stringify({ parse: { incremental: false } }))
  process.env.TURTLE_LSP_CHECK_INCREMENTAL = "1"
  const s = await startServer({ cwd: ws })
  delete process.env.TURTLE_LSP_CHECK_INCREMENTAL
  const uri = uriOf(path.join(ws, "doc.ttl"))
  const text = '@prefix ex: <http://example.org/> .\nex:a ex:p 1 .\nex:b ex:p 2 .\nex:c ex:p "x" .\nex:d ex:p 3 .\n'
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  // Typing inside "x" on line 3, one character a change.
  let col = 12
  const type = async () => {
    const at = { line: 3, character: col++ }
    s.changeRange(uri, [{ range: { start: at, end: at }, text: "y" }])
    await s.diagnosticsFor(uri)
    // Answered after the log of the analysis.
    await s.request("workspace/symbol", { query: "" })
    return s.logs.filter((m) => m.startsWith(`incremental: ${uri}`)).at(-1)
  }
  assert.equal(await type(), `incremental: ${uri} Off`)
  s.notify("workspace/didChangeConfiguration", { settings: { turtle: { parse: { incremental: true } } } })
  await until(type, (m) => m === `incremental: ${uri} Spliced`, "not spliced with the editor's setting on")
  assert.deepEqual(s.messages, [])
  await done(s, ws)
})

// A loopback listener that counts the requests it gets (404 to all).
async function listener() {
  const hits = []
  const web = http.createServer((req, res) => {
    hits.push(req.url)
    res.writeHead(404)
    res.end()
  })
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  return { hits, base: `http://127.0.0.1:${web.address().port}`, close: () => web.close() }
}

// A folder with `turtle-lsp.json` (a string) and a file that imports `iri`.
function importer(fileText, iri) {
  const ws = tempDir("turtle-trust-")
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://example.com/o> a owl:Ontology ; owl:imports <${iri}> .\n`
  fs.writeFileSync(path.join(ws, "app.ttl"), text)
  if (fileText !== undefined) fs.writeFileSync(path.join(ws, "turtle-lsp.json"), fileText)
  return { ws, uri: uriOf(path.join(ws, "app.ttl")), text }
}

test("turtle-lsp.json cannot turn on network fetching; the client can, and a warning names the key", async () => {
  const web = await listener()
  try {
    const w = importer(JSON.stringify({ imports: { network: true } }), `${web.base}/x`)
    const s = await startServer({ cwd: w.ws })
    await s.request("workspace/symbol", { query: "" })
    s.open(w.uri, w.text)
    await until(() => s.diagnosticsFor(w.uri), (d) => d.some((x) => x.code === "unresolved-import"), "no unresolved-import")
    await sleep(500)
    assert.deepEqual(web.hits, [])
    assert.equal(s.messages.length, 1, JSON.stringify(s.messages))
    assert.equal(s.messages[0].type, 2)
    assert.match(s.messages[0].message, /turtle-lsp\.json: ignored imports\.network/)
    await done(s, w.ws)

    // The same file with the editor's setting: the import is fetched.
    const c = importer(JSON.stringify({ imports: { network: true } }), `${web.base}/x`)
    const s2 = await startServer({ cwd: c.ws, initializationOptions: { imports: { network: true } } })
    await s2.request("workspace/symbol", { query: "" })
    s2.open(c.uri, c.text)
    await until(() => web.hits, (h) => h.length > 0, "the client's setting did not fetch")
    // The file's own request is still reported.
    assert.equal(s2.messages.length, 1, JSON.stringify(s2.messages))
    await done(s2, c.ws)
  } finally {
    web.close()
  }
})

test("a turtle-lsp.json that asks for unlimited queries or a 1e300-second timeout starts the server and warns", async () => {
  const ws = workspace(JSON.stringify({ query: { timeoutSeconds: 1e300, maxRows: 0 }, shacl: { maxTriples: 99999999999 } }))
  const { s, diags } = await session(ws)
  assert.equal(diags.length, 1, JSON.stringify(diags))
  assert.equal(s.messages.length, 1, JSON.stringify(s.messages))
  assert.match(s.messages[0].message, /query\.timeoutSeconds needs a number of seconds, not 1e\+300/)
  assert.match(s.messages[0].message, /ignored .*query\.maxRows.*shacl\.maxTriples|ignored .*shacl\.maxTriples.*query\.maxRows/)
  await done(s, ws)
})

test("a turtle-lsp.json over 1 MB warns and is ignored", async () => {
  const ws = workspace(JSON.stringify({ "lint.undescribedTerms": false, pad: "x".repeat(1 << 20) }))
  const { s, diags } = await session(ws)
  assert.equal(diags.length, 1, "the oversized file's settings are ignored")
  assert.equal(s.messages.length, 1, JSON.stringify(s.messages))
  assert.match(s.messages[0].message, /larger than 1 MB/)
  await done(s, ws)
})

// ---- lint.ignore and lint.enable ----
//
// A workspace of its own: vocab.ttl describes four ex: terms (so ex: is an own
// namespace), and data.ttl and other.ttl each get an unused-prefix hint
// (foaf:), an undescribed-term warning (ex:Persn) and an unknown-term warning
// (rdfs:labl).

const LINT_DATA = `@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
ex:bob a ex:Persn ; rdfs:labl "Bob" .
`
const VOCAB = `@prefix ex: <http://example.org/> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
ex:Person a rdfs:Class .
ex:Place a rdfs:Class .
ex:name a rdf:Property .
ex:city a rdf:Property .
`
const ALL_CODES = ["undescribed-term", "unknown-term", "unused-prefix"]
const codes = (diags) => diags.map((d) => d.code).sort()

// The workspace with `settings` as turtle-lsp.json; a server started in it
// with `initializationOptions`, stopped (and the folder removed) after the test.
async function lintSession(t, settings, initializationOptions) {
  const ws = tempDir("turtle-lintlists-")
  fs.writeFileSync(path.join(ws, "vocab.ttl"), VOCAB)
  fs.writeFileSync(path.join(ws, "data.ttl"), LINT_DATA)
  fs.writeFileSync(path.join(ws, "other.ttl"), LINT_DATA)
  if (settings !== undefined) fs.writeFileSync(path.join(ws, "turtle-lsp.json"), JSON.stringify(settings))
  const s = await startServer({ cwd: ws, initializationOptions })
  t.after(() => done(s, ws))
  return { s, data: uriOf(path.join(ws, "data.ttl")), other: uriOf(path.join(ws, "other.ttl")) }
}

// Opens data.ttl once the index has landed; its codes once `ok` holds.
async function openCodes(s, uri, ok) {
  await s.request("workspace/symbol", { query: "" })
  s.open(uri, LINT_DATA)
  return codes(await until(() => s.diagnosticsFor(uri), (d) => ok(codes(d)), "open document"))
}

const section = (turtle) => ({ settings: { turtle: { diagnostics: { workspace: true }, ...turtle } } })
const exactly = (want) => (d) => codes(d).join() === want.join()

test("lint lists: with no settings every code is reported (what the list tests hide)", async (t) => {
  const { s, data } = await lintSession(t)
  assert.deepEqual(await openCodes(s, data, (c) => c.includes("undescribed-term")), ALL_CODES)
})

test("lint.ignore in turtle-lsp.json and in the editor settings add up, for open files and workspace diagnostics", async (t) => {
  const { s, data, other } = await lintSession(t, { lint: { ignore: ["unused-prefix"] } }, {
    diagnostics: { workspace: true },
    lint: { ignore: ["unknown-term"] },
  })
  // The file's list and initializationOptions' list: only the undescribed term is left.
  assert.deepEqual(await openCodes(s, data, (c) => c.includes("undescribed-term")), ["undescribed-term"])
  // A file that is not open gets the same, from workspace diagnostics.
  await until(() => s.diagnosticsFor(other), exactly(["undescribed-term"]), "workspace diagnostics")

  // The configuration section replaces initializationOptions, and the file's
  // list still applies; lint.undescribedTerms: false still works.
  s.notify("workspace/didChangeConfiguration", section({ lint: { undescribedTerms: false } }))
  await until(() => s.diagnosticsFor(data), exactly(["unknown-term"]), "open document with the lint off")
  await until(() => s.diagnosticsFor(other), exactly(["unknown-term"]), "workspace diagnostics with the lint off")
  // The section's lint.ignore adds to the file's.
  s.notify("workspace/didChangeConfiguration", section({ lint: { ignore: ["unknown-term"] } }))
  await until(() => s.diagnosticsFor(data), exactly(["undescribed-term"]), "open document, file + section")
  await until(() => s.diagnosticsFor(other), exactly(["undescribed-term"]), "workspace diagnostics, file + section")
  assert.deepEqual(s.messages, [])
})

test("lint.undescribedTerms: false in turtle-lsp.json still works next to lint.ignore", async (t) => {
  const { s, data } = await lintSession(t, { lint: { ignore: ["unused-prefix"], undescribedTerms: false } })
  assert.deepEqual(await openCodes(s, data, (c) => c.includes("unknown-term")), ["unknown-term"])
  // After the index has landed the undescribed term stays hidden.
  await sleep(300)
  assert.deepEqual(codes(await s.diagnosticsFor(data)), ["unknown-term"])
  assert.deepEqual(s.messages, [])
})

test("lint.enable with an off-by-default or unknown code changes nothing else and warns about nothing", async (t) => {
  const file = { lint: { enable: ["missing-ontology-license", "missing-label", "no-such-code"], ignore: ["missing-definition"] } }
  const { s, data } = await lintSession(t, file, { lint: { enable: ["unused-prefix", "also-unknown"] } })
  // A code that is already on stays on when enabled; the others are untouched.
  assert.deepEqual(await openCodes(s, data, (c) => c.includes("undescribed-term")), ALL_CODES)
  assert.deepEqual(s.messages, [])
})
