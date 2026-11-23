// owl:imports: resolved through a local catalog (Turtle, RDF/XML and
// OWL/XML, with a transitive cycle), unresolved imports diagnosed, and network access only
// when turtle.imports.network is on, fetched once and cached. With it off, a
// quick fix fetches one import on request; its cached copy opens read-only.
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
import { fileURLToPath } from "node:url"
import crypto from "node:crypto"
import { spawnSync } from "node:child_process"
import { startServer, serverPath, tempDir as longTempDir, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

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
    } else if (req.url === "/cut.rdf") {
      res.writeHead(200, { "content-type": "application/rdf+xml" })
      res.end(CUT_RDF)
    } else if (req.url === "/latin1.rdf") {
      res.writeHead(200, { "content-type": "application/rdf+xml" })
      res.end(LATIN1_RDF)
    } else if (OWL_XML_SERVED[req.url]) {
      // OWL/XML as servers send it: the type its Recommendation names, an
      // RDF/XML type a `.owl` name gets, or plain text.
      res.writeHead(200, { "content-type": OWL_XML_SERVED[req.url] })
      res.end(req.url.startsWith("/cut") ? CUT_OWL_XML : owlXmlAt(req.url))
    } else if (FUNCTIONAL_SERVED[req.url]) {
      // OWL Functional syntax as servers send it: its registered type, an
      // RDF/XML type a `.owl` name gets, or plain text (raw GitHub files).
      res.writeHead(200, { "content-type": FUNCTIONAL_SERVED[req.url] })
      res.end(req.url.startsWith("/cut") ? CUT_FUNCTIONAL : functionalAt(req.url))
    } else if (req.url === "/donn%C3%A9es.ttl") {
      res.writeHead(200, { "content-type": "text/turtle" })
      res.end(VOCAB)
    } else {
      res.writeHead(404)
      res.end()
    }
  })
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  base = `http://127.0.0.1:${web.address().port}`
})
after(() => web.close())

// OWL Functional documents served by type, and one with an error on line 4.
const FUNCTIONAL_SERVED = {
  "/onto.ofn": "text/owl-functional",
  "/rdfxml-type.owl": "application/rdf+xml",
  "/plain.owl": "text/plain; charset=utf-8",
  "/cut.ofn": "text/owl-functional",
}
const functionalAt = (url) => `Prefix(:=<http://example.org/fn${url.replace(/\W/g, "_")}#>)
Prefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)
Ontology(<http://example.org/fn${url}>
Declaration(Class(:Remote))
AnnotationAssertion(rdfs:label :Remote "Remote functional ${url}")
)
`
const CUT_FUNCTIONAL = `Prefix(:=<http://example.org/cut-fn#>)
Prefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)
Ontology(<http://example.org/cut-fn>
SubClassOf(:First nope:Thing)
AnnotationAssertion(rdfs:label :Second "Second functional")
)
`

// OWL/XML documents served by type, and one with an error on line 5.
const OWL_XML_SERVED = {
  "/onto.owx": "application/owl+xml",
  "/rdfxml-type-ox.owl": "application/rdf+xml",
  "/plain-ox.owl": "text/plain; charset=utf-8",
  "/cut.owx": "application/owl+xml",
}
const owlXmlAt = (url) => `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/ox${url}">
    <Prefix name="" IRI="http://example.org/ox${url.replace(/\W/g, "_")}#"/>
    <Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
    <Declaration><Class abbreviatedIRI=":Remote"/></Declaration>
    <AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><AbbreviatedIRI>:Remote</AbbreviatedIRI><Literal>Remote OWL/XML ${url}</Literal></AnnotationAssertion>
</Ontology>
`
const CUT_OWL_XML = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/cut-ox">
    <Prefix name="" IRI="http://example.org/cut-ox#"/>
    <Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
    <SubClassOf><Class abbreviatedIRI=":First"/><Class abbreviatedIRI="nope:Thing"/></SubClassOf>
    <AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><AbbreviatedIRI>:Second</AbbreviatedIRI><Literal>Second OWL/XML</Literal></AnnotationAssertion>
</Ontology>
`

// RDF/XML with an error on line 5: the first class is read, the second not.
const CUT_RDF = `<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
     xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
  <rdf:Description rdf:about="http://example.org/cut#First"><rdfs:label>First cut</rdfs:label></rdf:Description>
  <rdf:Description rdf:about="http://example.org/cut#Second"><rdfs:label>Second cut</rdfs:label></rdf:Descr>
</rdf:RDF>
`

// RDF/XML with a byte that is not UTF-8 (0xFF) in a label: "a\xFFb".
const LATIN1_RDF = Buffer.concat([
  Buffer.from(`<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
     xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#">
  <rdf:Description rdf:about="http://example.org/latin1#A"><rdfs:label>a`),
  Buffer.from([0xff]),
  Buffer.from(`b</rdfs:label></rdf:Description>
</rdf:RDF>
`),
])

// Absolute IRIs: they do not depend on the URL the document is read from.
const VOCAB = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://example.org/vocab#Thing> a rdfs:Class ; rdfs:label "Vocab thing" .
`

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

async function fetchAction(s, uri, d) {
  const actions = await s.request("textDocument/codeAction", {
    textDocument: { uri },
    range: d.range,
    context: { diagnostics: [d] },
  })
  return actions.find((a) => a.command?.command === "turtle.fetchImport")
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
  // The imported files in a folder the index skips: only the catalog loads
  // them, so what hover and definition show comes from the import.
  fs.renameSync(path.join(ws, "vendor"), path.join(ws, "node_modules"))
  const catalog = path.join(ws, "catalog-v001.xml")
  fs.writeFileSync(catalog, fs.readFileSync(catalog, "utf8").replaceAll('uri="vendor/', 'uri="node_modules/'))
  const s = await start(ws, path.join(root, "cache"))
  try {
    const file = path.join(ws, "main.ttl")
    const uri = uriOf(file)
    const text = fs.readFileSync(file, "utf8")
    s.open(uri, text)
    const widget = await hoverUntil(s, uri, positionOf(text, "ext:Widget", 5), "Widget thing")
    assert.match(widget, /Something with a size/)
    assert.match(widget, /Described in \[ext\.ttl:\d+\]\(file:[^)]*ext\.ttl#L\d+\)/)
    // ext2 is imported by ext (which ext2 imports back).
    await hoverUntil(s, uri, positionOf(text, "ext2:size", 6), "size in millimetres")
    // RDF/XML.
    const teeth = await hoverUntil(s, uri, positionOf(text, "gear:teeth", 6), "number of teeth")
    assert.match(teeth, /How many teeth the gear has/)
    // Lines into the RDF/XML file are its own: the start tag that describes
    // the property, on line 9.
    assert.match(teeth, /Described in \[gear\.rdf:9\]\(file:[^)]*gear\.rdf#L9\)/)
    const def = await s.request("textDocument/definition", {
      textDocument: { uri },
      position: positionOf(text, "gear:teeth", 6),
    })
    assert.equal(def.length, 1, JSON.stringify(def))
    assert.match(def[0].uri, /\/node_modules\/gear\.rdf$/)
    // At the value that names it: `#teeth` of `rdf:about="#teeth"`.
    assert.deepEqual(def[0].range, { start: { line: 8, character: 37 }, end: { line: 8, character: 43 } })
    // Imported terms are described: nothing to report, and imports are never diagnosed.
    const diags = await diagnosticsUntil(s, uri, (d) => d.length === 0)
    assert.deepEqual(diags, [])
    assert.equal(hits.length, 0)
    // Nothing of the imported files is in the index.
    const symbols = await s.request("workspace/symbol", { query: "teeth" })
    assert.ok(!symbols.some((x) => x.location.uri.includes("/node_modules/")), JSON.stringify(symbols))
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
  assert.match(fetched[0].accept, /^text\/turtle, application\/trig;q=0\.9, .*application\/ld\+json;q=0\.8, application\/rdf\+xml;q=0\.8, text\/owl-functional;q=0\.7, application\/owl\+xml;q=0\.7$/)
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

test("the quick fix fetches one import on request; its cached copy is what definition opens, read-only", async () => {
  const before = hits.length
  const iri = `${base}/onto.ttl`
  const w = remoteWorkspace("quickfix", iri)
  const pos = positionOf(w.text, "r:Remote", 3)
  const s = await start(w.ws, w.cache)
  try {
    s.open(w.uri, w.text)
    const diags = await diagnosticsUntil(s, w.uri, (d) => d.some((x) => x.code === "unresolved-import"))
    const d = diags.find((x) => x.code === "unresolved-import")
    assert.match(d.message, /quick fix/)
    const action = await fetchAction(s, w.uri, d)
    assert.equal(action.title, `Fetch ${iri} and cache it`)
    assert.equal(action.kind, "quickfix")
    assert.deepEqual(action.diagnostics, [d])
    assert.deepEqual(action.command.arguments, [iri])
    assert.equal(action.edit, undefined)
    assert.equal(hits.length, before, "offering the fix fetches nothing")

    assert.equal(await s.request("workspace/executeCommand", action.command), null)
    await diagnosticsUntil(s, w.uri, (d) => !d.some((x) => x.code === "unresolved-import"))
    // Hover names the URL the cached copy came from.
    const hover = await hoverUntil(s, w.uri, pos, "Remote thing")
    assert.ok(hover.includes(`Described in ${iri}:`), hover)
    assert.deepEqual(hits.slice(before).map((h) => h.url), ["/onto.ttl"])

    // Definition opens the cached file, not the URL.
    const definition = () =>
      s.request("textDocument/definition", { textDocument: { uri: w.uri }, position: pos })
    const locs = await definition()
    assert.equal(locs.length, 1, JSON.stringify(locs))
    const cached = fileURLToPath(locs[0].uri)
    assert.ok(cached.startsWith(path.join(w.cache, "imports") + path.sep), cached)
    const text = fs.readFileSync(cached, "utf8")
    assert.ok(text.endsWith(ONTO), text)

    // Opened, the cached copy is a read-only view: no diagnostics, and its
    // terms are not indexed a second time.
    s.open(locs[0].uri, text)
    assert.deepEqual(await s.diagnosticsFor(locs[0].uri), [])
    assert.deepEqual(await definition(), locs)
  } finally {
    await s.stop()
  }
  assert.equal(hits.length, before + 1)
})

test("a fetch on request that fails keeps the diagnostic and the fix, and asking again retries at once", async () => {
  const before = hits.length
  const w = remoteWorkspace("quickfix-missing", `${base}/missing`)
  const s = await start(w.ws, w.cache)
  try {
    s.open(w.uri, w.text)
    let diags = await diagnosticsUntil(s, w.uri, (d) => d.some((x) => x.code === "unresolved-import"))
    let action = await fetchAction(s, w.uri, diags.find((x) => x.code === "unresolved-import"))
    await s.request("workspace/executeCommand", action.command)
    const failed = (d) => d.some((x) => x.code === "unresolved-import" && /could not be fetched/.test(x.message))
    diags = await diagnosticsUntil(s, w.uri, failed)
    assert.deepEqual(hits.slice(before).map((h) => h.url), ["/missing"])
    action = await fetchAction(s, w.uri, diags.find((x) => x.code === "unresolved-import"))
    assert.ok(action, "the fix is still offered")

    await s.request("workspace/executeCommand", action.command)
    const end = Date.now() + 5000
    while (hits.length < before + 2 && Date.now() < end) await sleep(50)
    assert.deepEqual(hits.slice(before).map((h) => h.url), ["/missing", "/missing"])
    await diagnosticsUntil(s, w.uri, failed)
  } finally {
    await s.stop()
  }
})

test("workspace/executeCommand rejects an unknown command and bad arguments", async () => {
  const w = remoteWorkspace("command", `${base}/onto.ttl`)
  const s = await start(w.ws, w.cache)
  try {
    for (const params of [
      { command: "turtle.nope", arguments: [] },
      { command: "turtle.fetchImport", arguments: [] },
      { command: "turtle.fetchImport", arguments: [42] },
    ]) {
      await assert.rejects(s.request("workspace/executeCommand", params), (e) => e.code === -32602)
    }
  } finally {
    await s.stop()
  }
})

// A shape whose sh:class result an unresolved import could change.
function shaclWorkspace(name) {
  const root = tempDir(name)
  const ws = path.join(root, "ws")
  fs.mkdirSync(ws)
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.com/> .
<http://example.com/ont> a owl:Ontology ;
    owl:imports <${base}/missing> .
ex:S a sh:NodeShape ; sh:targetClass ex:P ;
    sh:property [ sh:path ex:kind ; sh:class ex:Kind ] .
ex:a a ex:P ; ex:kind ex:k .
`
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, text)
  return { ws, cache: path.join(root, "cache"), uri: uriOf(file), text }
}

test("imports.unresolved: soften lowers what an import could change; ignore keeps full severity", async () => {
  const before = hits.length
  const violation = (d) => d.find((x) => x.code === "shacl-violation")
  const softened = (d) => violation(d)?.severity === 2 && /may change once these imports resolve/.test(violation(d).message)
  const full = (d) => violation(d)?.severity === 1 && !/imports/.test(violation(d).message)
  const imported = (d) => d.find((x) => x.code === "unresolved-import")
  for (const [init, first, then, changed] of [
    [undefined, softened, full, "ignore"],
    [{ imports: { unresolved: "ignore" } }, full, softened, "soften"],
  ]) {
    const w = shaclWorkspace(`unresolved-${changed}`)
    const s = await start(w.ws, w.cache, init)
    try {
      s.open(w.uri, w.text)
      for (const want of [first, then]) {
        const diags = await diagnosticsUntil(s, w.uri, (d) => want(d) && imported(d))
        assert.equal(imported(diags).severity, 3)
        assert.ok(await fetchAction(s, w.uri, imported(diags)), "the fetch fix is offered")
        if (want === first) {
          s.notify("workspace/didChangeConfiguration", { settings: { turtle: { imports: { unresolved: changed } } } })
        }
      }
    } finally {
      await s.stop()
    }
  }
  assert.equal(hits.length, before, JSON.stringify(hits.slice(before)))
})

// ---- fetch policy ----

// Like `start`, for a server that is not told to fetch from loopback (the
// suite's servers listen there, and are told to).
async function startStrict(cwd, cache, initializationOptions) {
  const prev = process.env.TURTLE_LSP_ALLOW_PRIVATE_HOSTS
  process.env.TURTLE_LSP_ALLOW_PRIVATE_HOSTS = "0"
  try {
    return await start(cwd, cache, initializationOptions)
  } finally {
    if (prev === undefined) delete process.env.TURTLE_LSP_ALLOW_PRIVATE_HOSTS
    else process.env.TURTLE_LSP_ALLOW_PRIVATE_HOSTS = prev
  }
}

// Without a cache an import is named by the IRI its importer wrote. An IRI
// may hold a letter outside ASCII, which a URI may not: Go to Definition on a
// term of such an import failed with "valid uri: ParseError".
test("an import read without a cache is located at its IRI as a URI", async () => {
  const root = tempDir("no-cache")
  const ws = path.join(root, "ws")
  fs.mkdirSync(ws)
  // The cache folder cannot be made below a file.
  fs.writeFileSync(path.join(root, "file"), "")
  const iri = `${base}/données.ttl`
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix v: <http://example.org/vocab#> .
<http://example.com/ont/app> a owl:Ontology ;
    owl:imports <${iri}> .
<http://example.com/ont/app#x> a v:Thing .
`
  fs.writeFileSync(path.join(ws, "app.ttl"), text)
  const uri = uriOf(path.join(ws, "app.ttl"))
  const pos = positionOf(text, "v:Thing", 3)
  const s = await start(ws, path.join(root, "file", "cache"), { imports: { network: true } })
  try {
    s.open(uri, text)
    await hoverUntil(s, uri, pos, "Vocab thing")
    const found = await s.request("textDocument/definition", { textDocument: { uri }, position: pos })
    assert.deepEqual(found.map((l) => l.uri), [`${base}/donn%C3%A9es.ttl`])
    // A reference lens lists no location in an import.
    const lenses = await s.request("textDocument/codeLens", { textDocument: { uri } })
    const resolved = await Promise.all(lenses.map((l) => s.request("codeLens/resolve", l)))
    assert.deepEqual(resolved.map((l) => l.command.title), ["0 references", "0 references"])
  } finally {
    await s.stop()
  }
})

test("by default an import is not fetched from a loopback address, even when the network is on", async () => {
  const before = hits.length
  const w = remoteWorkspace("loopback", `${base}/onto.ttl`)
  const s = await startStrict(w.ws, w.cache, { imports: { network: true } })
  try {
    s.open(w.uri, w.text)
    const diags = await diagnosticsUntil(s, w.uri, (d) => d.some((x) => x.code === "unresolved-import"))
    const d = diags.find((x) => x.code === "unresolved-import")
    assert.match(d.message, /loopback address/)
    assert.match(d.message, /TURTLE_LSP_ALLOW_PRIVATE_HOSTS/)
    await sleep(300)
    assert.equal(hits.length, before, "no request reached the server")
  } finally {
    await s.stop()
  }
  assert.ok(!fs.existsSync(path.join(w.cache, "imports")), "nothing cached")
})

test("a file: import that names a host is refused, on every system", async () => {
  const root = tempDir("filehost")
  const ws = path.join(root, "ws")
  fs.mkdirSync(ws)
  const iri = "file://example.invalid/share/x.ttl"
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.com/ont/app> a owl:Ontology ; owl:imports <${iri}> .
`
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, text)
  fs.writeFileSync(
    path.join(ws, "catalog-v001.xml"),
    `<catalog><uri name="http://example.com/mapped" uri="file://example.invalid/share/y.ttl"/></catalog>`,
  )
  const s = await start(ws, path.join(root, "cache"))
  try {
    const uri = uriOf(file)
    s.open(uri, text)
    const diags = await diagnosticsUntil(s, uri, (d) => d.some((x) => x.code === "unresolved-import"))
    assert.match(diags.find((x) => x.code === "unresolved-import").message, /naming a host/)
  } finally {
    await s.stop()
  }
})

test("a cache file that does not name the import's URL is not used, and is replaced by a fetch", async () => {
  const before = hits.length
  const iri = `${base}/onto.ttl`
  const w = remoteWorkspace("cache-header", iri)
  // The cache names files by the SHA-256 of the URL; a file there that was
  // not written for this URL (planted, or another URL's after a collision) says nothing.
  const dir = path.join(w.cache, "imports")
  fs.mkdirSync(dir, { recursive: true })
  const name = crypto.createHash("sha256").update(iri).digest("hex") + ".ttl"
  const planted = "<http://example.com/evil> a <http://www.w3.org/2002/07/owl#Class> .\n"
  fs.writeFileSync(path.join(dir, name), planted)
  const s = await start(w.ws, w.cache, { imports: { network: true } })
  try {
    s.open(w.uri, w.text)
    await hoverUntil(s, w.uri, positionOf(w.text, "r:Remote", 3), "Remote thing")
  } finally {
    await s.stop()
  }
  assert.deepEqual(hits.slice(before).map((h) => h.url), ["/onto.ttl"])
  const kept = fs.readFileSync(path.join(dir, name), "utf8")
  assert.ok(kept.startsWith(`# turtle-lsp cache for ${iri}\n`), kept)
  assert.ok(!kept.includes("evil"), kept)
  assert.deepEqual(fs.readdirSync(dir), [name], "no temporary file is left")
})

test("imports in TriG and JSON-LD are read in their own syntax, whatever their IRI looks like", async () => {
  const root = tempDir("syntaxes")
  const ws = path.join(root, "ws")
  // In a hidden folder, which the workspace does not index: the terms are
  // known only if the imports are read.
  fs.mkdirSync(path.join(ws, ".vendor"), { recursive: true })
  fs.writeFileSync(
    path.join(ws, "catalog-v001.xml"),
    `<catalog>
<uri name="http://example.com/ont/graphs" uri=".vendor/graphs.trig"/>
<uri name="http://example.com/ont/linked" uri=".vendor/linked.jsonld"/>
</catalog>`,
  )
  fs.writeFileSync(
    path.join(ws, ".vendor", "graphs.trig"),
    `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix g: <http://example.com/g#> .
<http://example.com/graph> { g:Trig a owl:Class ; rdfs:label "Trig thing" . }
`,
  )
  fs.writeFileSync(
    path.join(ws, ".vendor", "linked.jsonld"),
    JSON.stringify({
      "@context": { rdfs: "http://www.w3.org/2000/01/rdf-schema#", owl: "http://www.w3.org/2002/07/owl#" },
      "@id": "http://example.com/j#Json",
      "@type": "owl:Class",
      "rdfs:label": "Json thing",
    }),
  )
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix g: <http://example.com/g#> .
@prefix j: <http://example.com/j#> .
<http://example.com/ont/app> a owl:Ontology ;
    owl:imports <http://example.com/ont/graphs>, <http://example.com/ont/linked> .
<http://example.com/ont/app#x> a g:Trig, j:Json .
`
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, text)
  const s = await start(ws, path.join(root, "cache"))
  try {
    const uri = uriOf(file)
    s.open(uri, text)
    await hoverUntil(s, uri, positionOf(text, "g:Trig", 3), "Trig thing")
    await hoverUntil(s, uri, positionOf(text, "j:Json", 3), "Json thing")
    const diags = await diagnosticsUntil(s, uri, (d) => !d.some((x) => x.code === "unresolved-import"))
    assert.ok(!diags.some((x) => x.code === "unresolved-import"), JSON.stringify(diags))
  } finally {
    await s.stop()
  }
})

// One import per format in test/fixtures/imports/ws/vendor, each mapped by
// the catalog: what is not read says so. The `.owl` files are workspace files
// too: truncated.owl and quoted.owl state their ontology IRIs, so their
// imports resolve in the workspace, with no catalog; truncated.owl's error
// is its own (an import with an error from outside the workspace warns:
// see the fetched RDF/XML below). functional.owl, in OWL Functional syntax,
// is read: its import resolves. owlxml.owl, in OWL/XML, is read as a
// workspace file and states its ontology IRI, so its import resolves in the
// workspace too, with no loader (the loader reads it in the test after).
const UNREAD = "which turtle-lsp does not read\\. In Protégé: File > Save as > RDF/XML or Turtle"
const FORMATS = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix t: <http://example.org/onto/truncated#> .
@prefix q: <http://example.org/onto/quoted#> .
@prefix fn: <http://example.org/onto/functional#> .
@prefix ox: <http://example.org/onto/owlxml#> .
<http://example.org/app> a owl:Ontology ;
    owl:imports <http://example.org/onto/owlxml>, <http://example.org/onto/functional>,
        <http://example.org/onto/manchester>, <http://example.org/onto/frames>,
        <http://example.org/onto/truncated>, <http://example.org/onto/quoted> .
<http://example.org/app#x> a t:Dog, t:Cat, t:Cow, q:Bird, fn:Dog, ox:Dog .
`
const EXPECTED = [
  ["manchester", 3, "unresolved-import", new RegExp(`could not be loaded: it holds OWL Manchester syntax, ${UNREAD}$`)],
  ["frames", 3, "unresolved-import", /could not be loaded: read as Turtle, none of its triples is usable: each has an undeclared prefix \(first error: .+\)\. In Protégé: File > Save as > RDF\/XML or Turtle$/],
]

// The Manchester file the catalog maps an import to, and a copy nothing
// imports: the severity of each one's unsupported-syntax.
const UNREAD_FILES = [["vendor/manchester.owl", 1], ["copy-manchester.owl", 2]]

test("an import in an OWL syntax is named and not read, but for OWL Functional syntax and OWL/XML, which are; a workspace .owl with an XML error is read but for what the error breaks", async () => {
  const root = tempDir("formats")
  fs.cpSync(path.join(fixtures, "imports"), root, { recursive: true })
  const ws = path.join(root, "ws")
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, FORMATS)
  fs.copyFileSync(path.join(ws, "vendor", "owlxml.owl"), path.join(ws, "copy.owl"))
  fs.copyFileSync(path.join(ws, "vendor", "manchester.owl"), path.join(ws, "copy-manchester.owl"))
  const uri = uriOf(file)
  const s = await start(ws, path.join(root, "cache"))
  try {
    s.open(uri, FORMATS)
    // Read before the error, and in single-quoted entities.
    const dog = await hoverUntil(s, uri, positionOf(FORMATS, "t:Dog", 3), "Truncated dog")
    await hoverUntil(s, uri, positionOf(FORMATS, "t:Cat", 3), "Truncated cat")
    const bird = await hoverUntil(s, uri, positionOf(FORMATS, "q:Bird", 3), "Quoted bird")
    // At the line of each class's start tag in the .owl file, past a byte
    // order mark and a DOCTYPE.
    assert.match(dog, /Described in \[truncated\.owl:8\]\(file:[^)]*truncated\.owl#L8\)/)
    assert.match(bird, /Described in \[quoted\.owl:11\]\(file:[^)]*quoted\.owl#L11\)/)
    const def = await s.request("textDocument/definition", {
      textDocument: { uri },
      position: positionOf(FORMATS, "q:Bird", 3),
    })
    assert.equal(def.length, 1, JSON.stringify(def))
    assert.match(def[0].uri, /\/vendor\/quoted\.owl$/)
    assert.equal(def[0].range.start.line, 10, JSON.stringify(def))
    // The Functional one: its class is described at its declaration.
    const fnDog = positionOf(FORMATS, "fn:Dog", 3)
    await hoverUntil(s, uri, fnDog, "functional.owl:5")
    const fnDef = await s.request("textDocument/definition", { textDocument: { uri }, position: fnDog })
    assert.deepEqual(fnDef.map((l) => [l.uri.replace(/.*\/vendor\//, ""), l.range.start]), [["functional.owl", { line: 4, character: 18 }]])
    const ours = (d) => d.filter((x) => x.code === "unresolved-import" || x.code === "import-truncated")
    const diags = ours(await diagnosticsUntil(s, uri, (d) => ours(d).length === EXPECTED.length))
    for (const [name, severity, code, message] of EXPECTED) {
      const at = positionOf(FORMATS, `<http://example.org/onto/${name}>`, 0)
      const d = diags.find((x) => x.range.start.line === at.line && x.range.start.character === at.character)
      assert.ok(d, `${name}: ${JSON.stringify(diags)}`)
      assert.equal(d.code, code, name)
      assert.equal(d.severity, severity, name)
      assert.match(d.message, message, name)
    }
    for (const name of ["functional", "owlxml"]) {
      const at = positionOf(FORMATS, `<http://example.org/onto/${name}>`, 0)
      assert.ok(!diags.some((d) => d.range.start.line === at.line && d.message.includes(name)), JSON.stringify(diags))
    }
    // The OWL/XML one: its class is described where it is declared, in the
    // file and in its copy, both workspace files.
    const oxDog = positionOf(FORMATS, "ox:Dog", 3)
    await hoverUntil(s, uri, oxDog, "owl:Class")
    const oxDef = await s.request("textDocument/definition", { textDocument: { uri }, position: oxDog })
    assert.deepEqual(
      oxDef.map((l) => [l.uri.replace(/.*\/ws\//, ""), l.range.start]).sort(),
      [["copy.owl", { line: 13, character: 20 }], ["vendor/owlxml.owl", { line: 13, character: 20 }]],
    )
    // The class after the error is read as a property of the one the error
    // leaves open, so it has no label; the import itself is described in
    // the workspace, so it gets no warning.
    assert.doesNotMatch(await hoverText(s, uri, positionOf(FORMATS, "t:Cow", 3)), /Truncated cow/)
    assert.ok(!diags.some((d) => d.code === "import-truncated"), JSON.stringify(diags))
    // The Manchester file an import is read from: an error; a copy nothing
    // imports: a warning.
    for (const [name, severity] of UNREAD_FILES) {
      const owl = path.join(ws, name)
      const doc = uriOf(owl)
      s.open(doc, fs.readFileSync(owl, "utf8"))
      const unsupported = (d) => d.find((x) => x.code === "unsupported-syntax")
      const d = unsupported(await diagnosticsUntil(s, doc, (d) => unsupported(d)?.severity === severity))
      assert.match(d.message, /holds OWL Manchester syntax/, name)
    }
    // The OWL/XML file and its copy are read, clean: nothing to report.
    for (const name of ["vendor/owlxml.owl", "copy.owl"]) {
      const owl = path.join(ws, name)
      const doc = uriOf(owl)
      s.open(doc, fs.readFileSync(owl, "utf8"))
      assert.deepEqual(await s.diagnosticsFor(doc), [], name)
    }
  } finally {
    await s.stop()
  }

  // The same for the closed files, in workspace diagnostics: nothing is
  // published for the OWL/XML ones.
  const w = await start(ws, path.join(root, "cache"), { diagnostics: { workspace: true } })
  try {
    for (const [name, severity] of UNREAD_FILES) {
      const doc = uriOf(path.join(ws, name))
      const unsupported = (d) => d.find((x) => x.code === "unsupported-syntax")
      await diagnosticsUntil(w, doc, (d) => unsupported(d)?.severity === severity)
    }
    for (const name of ["vendor/owlxml.owl", "copy.owl"]) {
      const doc = uriOf(path.join(ws, name))
      const found = w.publications.filter((p) => p.uri === doc && p.diagnostics.length > 0)
      assert.deepEqual(found, [], name)
    }
  } finally {
    await w.stop()
  }

  // The command line says the same.
  const r = spawnSync(serverPath, ["check", "app.ttl"], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(root, "cache"), NO_COLOR: "1" },
  })
  assert.equal(r.status, 0, r.stderr)
  const lines = r.stdout.split("\n").filter(Boolean)
  for (const [name, , code, message] of EXPECTED) {
    const at = positionOf(FORMATS, `<http://example.org/onto/${name}>`, 0)
    const kind = code === "import-truncated" ? "warning" : "info"
    const prefix = `app.ttl:${at.line + 1}:${at.character + 1}: ${kind}[${code}] `
    const line = lines.find((l) => l.startsWith(prefix))
    assert.ok(line, `${prefix}\n${r.stdout}`)
    assert.match(line.slice(prefix.length), message)
  }
  assert.equal(lines.length, EXPECTED.length, r.stdout)
  // The error is in the .owl file's own report, at its line.
  const own = spawnSync(serverPath, ["check", "vendor/truncated.owl"], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(root, "cache"), NO_COLOR: "1" },
  })
  assert.equal(own.status, 1, own.stderr)
  assert.match(own.stdout, /^vendor\/truncated\.owl:13:\d+: error\[xml-syntax\] /m)
})

// The OWL/XML catalog targets in a folder the index skips, so no workspace
// document covers them: the loader reads vendor/owlxml.owl (an OWL/XML
// `.owl`) and a `.owx` that it imports by its `<Import>`.
const OWL_XML_CATALOG = `<?xml version="1.0"?>
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <uri name="http://example.org/onto/owlxml" uri="node_modules/owlxml.owl"/>
  <uri name="http://example.org/onto/deep" uri="node_modules/deep.owx"/>
</catalog>
`
const DEEP_OWX = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/onto/deep">
    <Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
    <Declaration><Class IRI="http://example.org/onto/deep#Cat"/></Declaration>
    <AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><IRI>http://example.org/onto/deep#Cat</IRI><Literal>Deep cat</Literal></AnnotationAssertion>
</Ontology>
`
const OWL_XML_APP = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix ox: <http://example.org/onto/owlxml#> .
@prefix deep: <http://example.org/onto/deep#> .
<http://example.org/app> a owl:Ontology ;
    owl:imports <http://example.org/onto/owlxml> .
<http://example.org/app#x> a ox:Dog, deep:Cat .
`

test("an OWL/XML catalog target that no workspace document covers is read through the loader: its terms are known, its <Import> followed, and no unresolved-import", async () => {
  const root = tempDir("owlxml-catalog")
  const ws = path.join(root, "ws")
  fs.mkdirSync(path.join(ws, "node_modules"), { recursive: true })
  fs.copyFileSync(path.join(fixtures, "imports", "ws", "vendor", "owlxml.owl"), path.join(ws, "node_modules", "owlxml.owl"))
  // owlxml.owl imports deep.
  const owlxml = path.join(ws, "node_modules", "owlxml.owl")
  fs.writeFileSync(owlxml, fs.readFileSync(owlxml, "utf8").replace("    <Declaration>", "    <Import>http://example.org/onto/deep</Import>\n    <Declaration>"))
  fs.writeFileSync(path.join(ws, "node_modules", "deep.owx"), DEEP_OWX)
  fs.writeFileSync(path.join(ws, "catalog-v001.xml"), OWL_XML_CATALOG)
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, OWL_XML_APP)
  const uri = uriOf(file)
  const s = await start(ws, path.join(root, "cache"))
  try {
    s.open(uri, OWL_XML_APP)
    const dog = positionOf(OWL_XML_APP, "ox:Dog", 3)
    const hover = await hoverUntil(s, uri, dog, "owl:Class")
    // At the line of its declaration in the file the loader read, one
    // further down for the `<Import>`.
    assert.match(hover, /Described in \[owlxml\.owl:15\]\(file:[^)]*owlxml\.owl#L15\)/)
    const def = await s.request("textDocument/definition", { textDocument: { uri }, position: dog })
    assert.deepEqual(def.map((l) => [l.uri.replace(/.*\/ws\//, ""), l.range.start]), [["node_modules/owlxml.owl", { line: 14, character: 20 }]])
    await hoverUntil(s, uri, positionOf(OWL_XML_APP, "deep:Cat", 5), "Deep cat")
    const diags = await diagnosticsUntil(s, uri, (d) => d.length === 0)
    assert.deepEqual(diags, [])
    // Nothing of the imported files is in the index.
    const symbols = await s.request("workspace/symbol", { query: "Dog" })
    assert.ok(!symbols.some((x) => x.location.uri.includes("/node_modules/")), JSON.stringify(symbols))
  } finally {
    await s.stop()
  }
  const r = spawnSync(serverPath, ["check", "app.ttl"], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(root, "cache"), NO_COLOR: "1" },
  })
  assert.equal(r.status, 0, r.stderr)
  assert.equal(r.stdout, "")
})

// An OWL/XML file writes `<Import>`, never owl:imports: its imports are
// followed all the same, and an unresolved one is noted at the element's
// content.
const IMPORT_CATALOG = `<?xml version="1.0"?>
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <uri name="http://example.org/dep" uri="node_modules/dep.owx"/>
  <uri name="http://example.org/tdep" uri="node_modules/tdep.ttl"/>
</catalog>
`
const DEP_OWX = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/dep">
    <Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
    <Declaration><Class IRI="http://example.org/One"/></Declaration>
    <AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><IRI>http://example.org/One</IRI><Literal>Thing one</Literal></AnnotationAssertion>
</Ontology>
`
const TDEP = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<http://example.org/Two> rdfs:label "Thing two" .
`
const APP_OWX = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/app">
    <Import>http://example.org/dep</Import>
    <Import>http://example.org/tdep</Import>
    <Import>http://example.org/absent</Import>
    <Declaration><Class IRI="http://example.org/One"/></Declaration>
    <Declaration><Class IRI="http://example.org/Two"/></Declaration>
</Ontology>
`

test("an OWL/XML file's <Import> loads in the editor and in check", async () => {
  const root = tempDir("owx-import")
  const ws = path.join(root, "ws")
  fs.mkdirSync(path.join(ws, "node_modules"), { recursive: true })
  fs.writeFileSync(path.join(ws, "catalog-v001.xml"), IMPORT_CATALOG)
  fs.writeFileSync(path.join(ws, "node_modules", "dep.owx"), DEP_OWX)
  fs.writeFileSync(path.join(ws, "node_modules", "tdep.ttl"), TDEP)
  const file = path.join(ws, "app.owx")
  fs.writeFileSync(file, APP_OWX)
  const uri = uriOf(file)
  const absent = positionOf(APP_OWX, "http://example.org/absent", 0)
  const unresolved = (d) => d.filter((x) => x.code === "unresolved-import")
  const s = await start(ws, path.join(root, "cache"))
  try {
    s.open(uri, APP_OWX, "xml")
    const d = unresolved(await diagnosticsUntil(s, uri, (d) => unresolved(d).length > 0))
    assert.equal(d.length, 1, JSON.stringify(d))
    assert.match(d[0].message, /<http:\/\/example\.org\/absent>/)
    assert.deepEqual(d[0].range, { start: absent, end: { line: absent.line, character: absent.character + "http://example.org/absent".length } })
    await hoverUntil(s, uri, positionOf(APP_OWX, "http://example.org/One\"", 1), "Thing one")
    await hoverUntil(s, uri, positionOf(APP_OWX, "http://example.org/Two\"", 1), "Thing two")
    const def = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(APP_OWX, "http://example.org/One\"", 1) })
    assert.ok(def.some((l) => /\/node_modules\/dep\.owx$/.test(l.uri) && l.range.start.line === 3), JSON.stringify(def))
  } finally {
    await s.stop()
  }
  const r = spawnSync(serverPath, ["check", "app.owx"], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(root, "cache"), NO_COLOR: "1" },
  })
  assert.equal(r.status, 0, r.stderr)
  const lines = r.stdout.split("\n").filter(Boolean)
  assert.equal(lines.length, 1, r.stdout)
  assert.match(lines[0], new RegExp(`^app\\.owx:${absent.line + 1}:${absent.character + 1}: info\\[unresolved-import\\] .*<http://example\\.org/absent>`))
})

test("an owl:imports of a workspace .owl that states its ontology IRI needs no catalog", async () => {
  const root = tempDir("no-catalog")
  const ws = path.join(root, "ws")
  fs.mkdirSync(path.join(ws, "onto"), { recursive: true })
  fs.writeFileSync(
    path.join(ws, "onto", "animals.owl"),
    `<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
     xmlns:owl="http://www.w3.org/2002/07/owl#"
     xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
     xml:base="http://example.org/animals">
    <owl:Ontology rdf:about="http://example.org/animals"/>
    <owl:Class rdf:about="#Horse">
        <rdfs:label>Horse</rdfs:label>
    </owl:Class>
</rdf:RDF>
`,
  )
  const text = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix a: <http://example.org/animals#> .
<http://example.org/app> a owl:Ontology ;
    owl:imports <http://example.org/animals> .
<http://example.org/app#h> a a:Horse .
`
  const file = path.join(ws, "app.ttl")
  fs.writeFileSync(file, text)
  const uri = uriOf(file)
  const s = await start(ws, path.join(root, "cache"))
  try {
    s.open(uri, text)
    const horse = await hoverUntil(s, uri, positionOf(text, "a:Horse", 3), "Horse")
    assert.match(horse, /Described in \[animals\.owl:7\]\(file:[^)]*animals\.owl#L7\)/)
    const diags = await diagnosticsUntil(s, uri, (d) => d.length === 0)
    assert.deepEqual(diags, [])
  } finally {
    await s.stop()
  }
  const r = spawnSync(serverPath, ["check", "app.ttl"], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(root, "cache"), NO_COLOR: "1" },
  })
  assert.equal(r.status, 0, r.stderr)
  assert.equal(r.stdout, "")
})

test("a fetched RDF/XML with an error warns, from the web and from the cache", async () => {
  const w = remoteWorkspace("cut", `${base}/cut.rdf`)
  const at = positionOf(w.text, `<${base}/cut.rdf>`, 0)
  const truncated = (d) => d.find((x) => x.code === "import-truncated")
  for (const network of [true, false]) {
    const s = await start(w.ws, w.cache, { imports: { network } })
    try {
      s.open(w.uri, w.text)
      const d = truncated(await diagnosticsUntil(s, w.uri, (d) => truncated(d) !== undefined))
      assert.equal(d.severity, 2)
      assert.deepEqual(d.range.start, at)
      assert.match(d.message, /has an error at line 5, column \d+: .+\. What its errors break is not loaded$/)
      const text = `${w.text}<http://example.org/cut#First> a <http://example.org/cut#Second> .\n`
      s.change(w.uri, text, 2)
      await hoverUntil(s, w.uri, positionOf(text, "cut#First", 3), "First cut")
    } finally {
      await s.stop()
    }
  }
  const cached = fs.readdirSync(path.join(w.cache, "imports"))
  assert.equal(cached.length, 1, JSON.stringify(cached))
  assert.match(fs.readFileSync(path.join(w.cache, "imports", cached[0]), "utf8"), /^# turtle-lsp cache for .*\n# turtle-lsp: the RDF\/XML has an error at line 5, /)
})

test("a fetched OWL Functional document is read whatever type it is served as, cached as N-Triples, and one with an error warns", async () => {
  for (const url of ["/onto.ofn", "/rdfxml-type.owl", "/plain.owl"]) {
    const w = remoteWorkspace(`fn${url.replace(/\W/g, "-")}`, `${base}${url}`)
    const text = `${w.text}<http://example.org/x> a <http://example.org/fn${url.replace(/\W/g, "_")}#Remote> .\n`
    const s = await start(w.ws, w.cache, { imports: { network: true } })
    try {
      s.open(w.uri, text)
      await hoverUntil(s, w.uri, positionOf(text, "#Remote>", 1), `Remote functional ${url}`)
      const ours = (d) => d.filter((x) => x.code === "unresolved-import" || x.code === "import-truncated")
      assert.deepEqual(ours(await s.diagnosticsFor(w.uri, 10000)), [], url)
    } finally {
      await s.stop()
    }
    const cached = fs.readdirSync(path.join(w.cache, "imports"))
    assert.equal(cached.length, 1, JSON.stringify(cached))
    assert.match(cached[0], /\.ttl$/)
    const nt = fs.readFileSync(path.join(w.cache, "imports", cached[0]), "utf8")
    assert.match(nt, new RegExp(`^<http://example.org/fn${url.replace(/\W/g, "_")}#Remote> <http://www.w3.org/2000/01/rdf-schema#label> "Remote functional ${url}" \\.$`, "m"), url)
  }
  // With an error: loaded up to what it breaks, from the web and from the cache.
  const w = remoteWorkspace("fn-cut", `${base}/cut.ofn`)
  const text = `${w.text}<http://example.org/x> a <http://example.org/cut-fn#Second> .\n`
  const truncated = (d) => d.find((x) => x.code === "import-truncated")
  for (const network of [true, false]) {
    const s = await start(w.ws, w.cache, { imports: { network } })
    try {
      s.open(w.uri, text)
      const d = truncated(await diagnosticsUntil(s, w.uri, (d) => truncated(d) !== undefined))
      assert.equal(d.severity, 2)
      assert.match(d.message, /has an error at line 4, column 29: Undeclared prefix 'nope:'\. What its errors break is not loaded$/)
      await hoverUntil(s, w.uri, positionOf(text, "#Second>", 1), "Second functional")
    } finally {
      await s.stop()
    }
  }
  const cached = fs.readdirSync(path.join(w.cache, "imports"))
  assert.match(fs.readFileSync(path.join(w.cache, "imports", cached[0]), "utf8"), /^# turtle-lsp cache for .*\n# turtle-lsp: the OWL Functional syntax has an error at line 4, /)
})

test("a fetched OWL/XML document is read whatever type it is served as, cached as N-Triples, and one with an error warns", async () => {
  for (const url of ["/onto.owx", "/rdfxml-type-ox.owl", "/plain-ox.owl"]) {
    const w = remoteWorkspace(`ox${url.replace(/\W/g, "-")}`, `${base}${url}`)
    const text = `${w.text}<http://example.org/x> a <http://example.org/ox${url.replace(/\W/g, "_")}#Remote> .\n`
    const s = await start(w.ws, w.cache, { imports: { network: true } })
    try {
      s.open(w.uri, text)
      await hoverUntil(s, w.uri, positionOf(text, "#Remote>", 1), `Remote OWL/XML ${url}`)
      const ours = (d) => d.filter((x) => x.code === "unresolved-import" || x.code === "import-truncated")
      assert.deepEqual(ours(await s.diagnosticsFor(w.uri, 10000)), [], url)
    } finally {
      await s.stop()
    }
    const cached = fs.readdirSync(path.join(w.cache, "imports"))
    assert.equal(cached.length, 1, JSON.stringify(cached))
    assert.match(cached[0], /\.ttl$/)
    const nt = fs.readFileSync(path.join(w.cache, "imports", cached[0]), "utf8")
    assert.match(nt, new RegExp(`^<http://example.org/ox${url.replace(/\W/g, "_")}#Remote> <http://www.w3.org/2000/01/rdf-schema#label> "Remote OWL/XML ${url}" \\.$`, "m"), url)
  }
  // With an error: loaded up to what it breaks, from the web and from the cache.
  const w = remoteWorkspace("ox-cut", `${base}/cut.owx`)
  const text = `${w.text}<http://example.org/x> a <http://example.org/cut-ox#Second> .\n`
  const truncated = (d) => d.find((x) => x.code === "import-truncated")
  for (const network of [true, false]) {
    const s = await start(w.ws, w.cache, { imports: { network } })
    try {
      s.open(w.uri, text)
      const d = truncated(await diagnosticsUntil(s, w.uri, (d) => truncated(d) !== undefined))
      assert.equal(d.severity, 2)
      assert.match(d.message, /has an error at line 5, column 82: Undeclared prefix 'nope:'.*\. What its errors break is not loaded$/)
      await hoverUntil(s, w.uri, positionOf(text, "#Second>", 1), "Second OWL/XML")
    } finally {
      await s.stop()
    }
  }
  const cached = fs.readdirSync(path.join(w.cache, "imports"))
  assert.match(fs.readFileSync(path.join(w.cache, "imports", cached[0]), "utf8"), /^# turtle-lsp cache for .*\n# turtle-lsp: the OWL\/XML has an error at line 5, /)
})

test("a fetched RDF/XML that is not UTF-8 is not read, and the import says why", async () => {
  const w = remoteWorkspace("latin1", `${base}/latin1.rdf`)
  const s = await start(w.ws, w.cache, { imports: { network: true } })
  try {
    s.open(w.uri, w.text)
    const unresolved = (d) => d.find((x) => x.code === "unresolved-import")
    const d = unresolved(await diagnosticsUntil(s, w.uri, (d) => unresolved(d) !== undefined))
    assert.deepEqual(d.range.start, positionOf(w.text, `<${base}/latin1.rdf>`, 0))
    const offset = LATIN1_RDF.indexOf(0xff)
    assert.match(
      d.message,
      new RegExp(`was fetched from ${base}/latin1\\.rdf but could not be read: not valid UTF-8 \\(first bad byte at offset ${offset}\\)`),
    )
    // Nothing of it is known, and nothing replaced the bad byte.
    const text = `${w.text}<http://example.org/latin1#A> a <http://example.org/latin1#A> .\n`
    s.change(w.uri, text, 2)
    await sleep(300)
    assert.doesNotMatch(await hoverText(s, w.uri, positionOf(text, "latin1#A", 3)), /a\uFFFDb/)
  } finally {
    await s.stop()
  }
  assert.ok(!fs.existsSync(path.join(w.cache, "imports")), "nothing cached")
})
