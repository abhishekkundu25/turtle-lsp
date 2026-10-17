// owl:imports: resolved through a local catalog (Turtle and RDF/XML, with a
// transitive cycle), unresolved imports diagnosed, and network access only
// when turtle.imports.network is on, fetched once and cached.
//
// Each test gets its own temp workspace and import cache
// (TURTLE_LSP_CACHE_DIR, inherited by the server), and a local HTTP server
// counts every request.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { startServer, tempDir as longTempDir, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
process.env.NO_PROXY = "127.0.0.1,localhost"

const ONTO = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<> a owl:Ontology .
<#Remote> a owl:Class ; rdfs:label "Remote thing" .
`

let web, base
const hits = []
before(async () => {
  web = http.createServer((req, res) => {
    hits.push({ url: req.url, accept: req.headers.accept })
    if (req.url === "/old") {
      res.writeHead(302, { location: "/onto.ttl" })
      res.end()
    } else if (req.url === "/onto.ttl") {
      res.writeHead(200, { "content-type": "text/turtle" })
      res.end(ONTO)
    } else {
      res.writeHead(404)
      res.end()
    }
  })
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  base = `http://127.0.0.1:${web.address().port}`
})
after(() => web.close())

const temps = []
after(() => temps.forEach((d) => fs.rmSync(d, { recursive: true, force: true })))
function tempDir(name) {
  const d = longTempDir(`turtle-imports-${name}-`)
  temps.push(d)
  return d
}

// A workspace with one file importing `iri`, and an empty cache.
function remoteWorkspace(name, iri) {
  const root = tempDir(name)
  const ws = path.join(root, "ws")
  fs.mkdirSync(ws)
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix r: <${base}/onto.ttl#> .
<http://example.com/ont/app> a owl:Ontology ;
    owl:imports <${iri}> .
<http://example.com/ont/app#x> a r:Remote .
`
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, text)
  return { ws, cache: path.join(root, "cache"), uri: uriOf(file), text }
}

async function start(cwd, cache, initializationOptions) {
  process.env.TURTLE_LSP_CACHE_DIR = cache
  const s = await startServer({ cwd, initializationOptions })
  // Requests wait for the initial index; imports are resolved after it.
  await s.request("workspace/symbol", { query: "" })
  return s
}

async function hoverText(s, uri, pos) {
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: pos })
  return h?.contents?.value ?? ""
}

// Imports resolve in the background: poll until `want` shows up.
async function hoverUntil(s, uri, pos, want, ms = 10000) {
  const end = Date.now() + ms
  let last = ""
  while (Date.now() < end) {
    last = await hoverText(s, uri, pos)
    if (last.includes(want)) return last
    await sleep(50)
  }
  assert.fail(`hover never showed ${JSON.stringify(want)}; last: ${last}`)
}

async function diagnosticsUntil(s, uri, pred, ms = 10000) {
  const end = Date.now() + ms
  let last
  while (Date.now() < end) {
    last = await s.diagnosticsFor(uri, ms)
    if (pred(last)) return last
    await sleep(50)
  }
  assert.fail(`diagnostics never matched; last: ${JSON.stringify(last)}`)
}

test("an import mapped by a local catalog makes its terms (and its imports') known", async () => {
  const root = tempDir("catalog")
  fs.cpSync(path.join(fixtures, "imports"), root, { recursive: true })
  const ws = path.join(root, "ws")
  const s = await start(ws, path.join(root, "cache"))
  try {
    const file = path.join(ws, "main.ttl")
    const uri = uriOf(file)
    const text = fs.readFileSync(file, "utf8")
    s.open(uri, text)
    const widget = await hoverUntil(s, uri, positionOf(text, "ext:Widget", 5), "Widget thing")
    assert.match(widget, /Something with a size/)
    assert.match(widget, /Described in ext\.ttl/)
    // ext2 is imported by ext (which ext2 imports back).
    await hoverUntil(s, uri, positionOf(text, "ext2:size", 6), "size in millimetres")
    // RDF/XML.
    const teeth = await hoverUntil(s, uri, positionOf(text, "gear:teeth", 6), "number of teeth")
    assert.match(teeth, /How many teeth the gear has/)
    // Imported terms are described: nothing to report, and imports are never diagnosed.
    const diags = await diagnosticsUntil(s, uri, (d) => d.length === 0)
    assert.deepEqual(diags, [])
    assert.equal(hits.length, 0)
  } finally {
    await s.stop()
  }
})

test("an unresolved import gets an information diagnostic, and nothing is fetched with the setting off", async () => {
  const before = hits.length
  const w = remoteWorkspace("off", `${base}/onto.ttl`)
  const s = await start(w.ws, w.cache)
  try {
    s.open(w.uri, w.text)
    const diags = await diagnosticsUntil(s, w.uri, (d) => d.some((x) => x.code === "unresolved-import"))
    const d = diags.find((x) => x.code === "unresolved-import")
    assert.equal(d.severity, 3)
    assert.deepEqual(d.range.start, positionOf(w.text, `<${base}/onto.ttl>`, 0))
    assert.match(d.message, /catalog-v001\.xml/)
    assert.match(d.message, /turtle\.imports\.network/)
    await sleep(300)
    assert.equal(hits.length, before, JSON.stringify(hits.slice(before)))
  } finally {
    await s.stop()
  }
})

test("with the setting on, an import is fetched once (following redirects) and then served from the cache", async () => {
  const before = hits.length
  const w = remoteWorkspace("on", `${base}/old`)
  const pos = positionOf(w.text, "r:Remote", 3)
  const s = await start(w.ws, w.cache, { imports: { network: true } })
  try {
    s.open(w.uri, w.text)
    await hoverUntil(s, w.uri, pos, "Remote thing")
    const diags = await s.diagnosticsFor(w.uri)
    assert.ok(!diags.some((d) => d.code === "unresolved-import"), JSON.stringify(diags))
  } finally {
    await s.stop()
  }
  const fetched = hits.slice(before)
  assert.deepEqual(fetched.map((h) => h.url), ["/old", "/onto.ttl"])
  assert.match(fetched[0].accept, /^text\/turtle, application\/n-triples;q=0\.9, application\/rdf\+xml;q=0\.8$/)
  assert.equal(fs.readdirSync(path.join(w.cache, "imports")).length, 1)

  // A new session, with or without the network: the cached copy is used.
  for (const network of [true, false]) {
    const s2 = await start(w.ws, w.cache, { imports: { network } })
    try {
      s2.open(w.uri, w.text)
      await hoverUntil(s2, w.uri, pos, "Remote thing")
    } finally {
      await s2.stop()
    }
  }
  assert.equal(hits.length, before + 2, JSON.stringify(hits.slice(before)))
})

test("turning the setting on resolves a pending import without a restart", async () => {
  const before = hits.length
  const w = remoteWorkspace("toggle", `${base}/onto.ttl`)
  const s = await start(w.ws, w.cache)
  try {
    s.open(w.uri, w.text)
    await diagnosticsUntil(s, w.uri, (d) => d.some((x) => x.code === "unresolved-import"))
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { imports: { network: true } } } })
    await diagnosticsUntil(s, w.uri, (d) => !d.some((x) => x.code === "unresolved-import"))
    await hoverUntil(s, w.uri, positionOf(w.text, "r:Remote", 3), "Remote thing")
    assert.equal(hits.length, before + 1)
  } finally {
    await s.stop()
  }
})

test("a failed fetch is reported on the import", async () => {
  const before = hits.length
  const w = remoteWorkspace("missing", `${base}/missing`)
  const s = await start(w.ws, w.cache, { imports: { network: true } })
  try {
    s.open(w.uri, w.text)
    const diags = await diagnosticsUntil(s, w.uri, (d) => d.some((x) => x.code === "unresolved-import"))
    const d = diags.find((x) => x.code === "unresolved-import")
    assert.match(d.message, /could not be fetched/)
    assert.match(d.message, /catalog-v001\.xml/)
    assert.deepEqual(hits.slice(before).map((h) => h.url), ["/missing"])
  } finally {
    await s.stop()
  }
  assert.ok(!fs.existsSync(path.join(w.cache, "imports")), "nothing cached")
})
