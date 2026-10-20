// JSON-LD, every planned feature, through the LSP protocol and the command
// line. Each test is pending (`todo`), tagged with its phase and, for phase
// 1, the ticket in docs/tickets/jsonld/ that promotes it. A pending test runs
// and reports but does not fail the build; the ticket that makes one pass
// removes its `todo` in the same change.
//
// Tests assert codes and ranges, never message wording. Every expected range
// is written out in test/jsonld-fixtures.mjs, next to the fixture text it
// points into, with the convention it follows.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { serverPath, startServer, tempDir, uriOf } from "./lsp-client.mjs"
import * as F from "./jsonld-fixtures.mjs"

const phase1 = (ticket) => ({ todo: `jsonld:phase1 (ticket ${ticket})`, timeout: 25_000 })
const phase2 = { todo: "jsonld:phase2", timeout: 25_000 }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const withCode = (diags, code) => diags.filter((d) => d.code === code)
const rangesOf = (diags, code) => withCode(diags, code).map((d) => d.range)
const sortRanges = (rs) => [...rs].sort((a, b) => F.rangeKey(a).localeCompare(F.rangeKey(b)))
const show = (diags) => JSON.stringify(diags.map((d) => `${F.rangeKey(d.range)} ${d.severity} ${d.code}`))
const languageOf = (name) => (name.endsWith(".jsonld") ? "jsonld" : /\.(rq|ru|sparql)$/.test(name) ? "sparql" : "turtle")

// The diagnostics of `uri` once they satisfy `ok`, or the last ones after
// `ms` (for the assertion that follows to report).
async function until(s, uri, ok, ms = 2500) {
  const end = Date.now() + ms
  let last = await s.diagnosticsFor(uri, ms)
  while (!ok(last) && Date.now() < end) {
    await sleep(50)
    last = await s.diagnosticsFor(uri, ms)
  }
  return last
}

// Locations from a definition or references answer (Location, Location[] or LocationLink[]).
function locations(res) {
  if (!res) return []
  return (Array.isArray(res) ? res : [res]).map((l) => ({ uri: l.targetUri ?? l.uri, range: l.targetSelectionRange ?? l.range }))
}

// A WorkspaceEdit as { uri: TextEdit[] }.
function changesOf(edit) {
  if (!edit) return {}
  if (edit.changes) return edit.changes
  const out = {}
  for (const c of edit.documentChanges ?? []) out[c.textDocument.uri] = [...(out[c.textDocument.uri] ?? []), ...c.edits]
  return out
}

// The text each code action's edit makes of `text` (`uri`'s).
async function fixedTexts(s, uri, text, d) {
  const actions = (await s.request("textDocument/codeAction", { textDocument: { uri }, range: d.range, context: { diagnostics: [d] } })) ?? []
  return actions.map((a) => changesOf(a.edit)[uri]).filter(Boolean).map((edits) => F.applyEdits(text, edits))
}

function decodeTokens(data, types) {
  const out = []
  let line = 0
  let character = 0
  for (let i = 0; i < data.length; i += 5) {
    line += data[i]
    character = data[i] ? data[i + 1] : character + data[i + 1]
    out.push({ line, character, length: data[i + 2], type: types[data[i + 3]] })
  }
  return out
}

function flattenSymbols(syms) {
  return (syms ?? []).flatMap((x) => [x, ...flattenSymbols(x.children)])
}

// ---- the command line ----

function workspace(t, files) {
  const ws = tempDir("turtle-jsonld-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
    fs.writeFileSync(path.join(ws, name), text)
  }
  return ws
}

const cliEnv = (ws) => ({ ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1", NO_PROXY: "127.0.0.1,localhost" })

function run(ws, args) {
  const r = spawnSync(serverPath, args, { cwd: ws, encoding: "utf8", timeout: 60_000, env: cliEnv(ws) })
  return { code: r.status, out: r.stdout, err: r.stderr, lines: r.stdout.split("\n").filter(Boolean) }
}

// For runs that reach this process's HTTP server: spawnSync would block the
// event loop that answers it.
function runAsync(ws, args) {
  return new Promise((resolve) => {
    const p = spawn(serverPath, args, { cwd: ws, env: cliEnv(ws) })
    let out = ""
    let err = ""
    p.stdout.on("data", (d) => (out += d))
    p.stderr.on("data", (d) => (err += d))
    const timer = setTimeout(() => p.kill(), 60_000)
    p.on("close", (code) => {
      clearTimeout(timer)
      resolve({ code, out, err })
    })
  })
}

// `check --format json`: each file's diagnostics with `code`.
function checkJson(ws, args = []) {
  const r = run(ws, ["check", "--format", "json", ...args])
  const report = r.out ? JSON.parse(r.out) : { files: [] }
  const of = (file, code) => (report.files.find((f) => f.path === file)?.diagnostics ?? []).filter((d) => !code || d.code === code)
  return { ...r, report, of }
}
const rangeKey = (r) => [r.start.line, r.start.column, r.end.line, r.end.column]
const cliRanges = (ds) =>
  [...ds.map((d) => d.range)].sort((a, b) => {
    const [x, y] = [rangeKey(a), rangeKey(b)]
    return x.map((v, i) => v - y[i]).find((d) => d !== 0) ?? 0
  })
const cliExpected = (rs) => cliRanges(rs.map((r) => ({ range: F.toCli(r) })))

// ---- one workspace for the editor tests ----
//
// vocab.jsonld describes ex: terms; uses.ttl uses them (and ex:Gadgt, a typo);
// edit.jsonld is the phase-2 document; people.context.jsonld is a context
// file other documents name by a relative URL.

let ws
let s
let scratchCount = 0
before(async () => {
  ws = tempDir("turtle-jsonld-ws-")
  fs.writeFileSync(path.join(ws, "vocab.jsonld"), F.VOCAB)
  fs.writeFileSync(path.join(ws, "uses.ttl"), F.USES)
  fs.writeFileSync(path.join(ws, "edit.jsonld"), F.EDIT)
  fs.writeFileSync(path.join(ws, "people.context.jsonld"), F.CTX_FILE)
  s = await startServer({ cwd: ws })
  await s.request("workspace/symbol", { query: "" })
})
after(async () => {
  await s?.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

const fileUri = (name) => uriOf(path.join(ws, name))

// Opens `text` as a new unsaved document in the workspace; returns its uri
// and first diagnostics. Closed again after the test.
async function openScratch(t, text, ext = "jsonld") {
  const uri = fileUri(`scratch-${++scratchCount}.${ext}`)
  s.open(uri, text, languageOf(`x.${ext}`))
  t.after(() => s.close(uri))
  return { uri, diags: await s.diagnosticsFor(uri) }
}

// Opens a workspace file with its text on disk.
async function openFile(name) {
  const uri = fileUri(name)
  s.open(uri, fs.readFileSync(path.join(ws, name), "utf8"), languageOf(name))
  await s.diagnosticsFor(uri)
  return uri
}

// =====================================================================================
// Phase 1
// =====================================================================================

// ---- JSON syntax (ticket 05) ----

test("a missing comma is one json-syntax error on the value before it; parsing goes on to the next error", { timeout: 25_000 }, async (t) => {
  const { uri, diags } = await openScratch(t, F.SYNTAX)
  assert.deepEqual(sortRanges(rangesOf(diags, "json-syntax")), [F.SYNTAX_MISSING_COMMA, F.SYNTAX_BAD_LITERAL], show(diags))
  assert.ok(withCode(diags, "json-syntax").every((d) => d.severity === 1), show(diags))
  assert.deepEqual(diags.filter((d) => d.severity === 1 && d.code !== "json-syntax"), [], "no errors cascade from the syntax errors")
  // Typing the comma clears its error and keeps the other.
  s.change(uri, F.SYNTAX_FIXED_COMMA)
  const fixed = await s.diagnosticsFor(uri)
  assert.deepEqual(rangesOf(fixed, "json-syntax"), [F.SYNTAX_BAD_LITERAL], show(fixed))
})

test("a trailing comma is a json-syntax error on the comma", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.TRAILING_COMMA)
  assert.deepEqual(rangesOf(diags, "json-syntax"), [F.TRAILING_COMMA_AT], show(diags))
})

test("check: a JSON syntax error exits 1 at its line and column", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "broken.jsonld": F.SYNTAX })
  const r = run(w, ["check"])
  assert.equal(r.code, 1, r.err)
  assert.equal(r.lines.length, 2, r.out)
  assert.match(r.lines[0], /^broken\.jsonld:4:16: error\[json-syntax\] /)
  assert.match(r.lines[1], /^broken\.jsonld:6:47: error\[json-syntax\] /)
})

// ---- @context errors (ticket 07) ----

test("an invalid term definition is a jsonld-context error on the term", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.CTX_INVALID)
  assert.deepEqual(rangesOf(diags, "jsonld-context"), [F.CTX_INVALID_AT], show(diags))
  assert.equal(withCode(diags, "jsonld-context")[0].severity, 1)
})

test("a cyclic IRI mapping is one jsonld-context error, on a term of the cycle", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.CTX_CYCLE)
  const found = rangesOf(diags, "jsonld-context")
  assert.equal(found.length, 1, show(diags))
  assert.ok([F.rangeKey(F.CTX_CYCLE_A), F.rangeKey(F.CTX_CYCLE_B)].includes(F.rangeKey(found[0])), show(diags))
})

test("redefining a protected term is a jsonld-context error on the redefinition", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.CTX_PROTECTED)
  assert.deepEqual(rangesOf(diags, "jsonld-context"), [F.CTX_PROTECTED_AT], show(diags))
})

test("check: a context error is reported at the definition", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "context-error.jsonld": F.CTX_INVALID })
  const r = checkJson(w)
  assert.equal(r.code, 1, r.err)
  assert.deepEqual(cliRanges(r.of("context-error.jsonld", "jsonld-context")), cliExpected([F.CTX_INVALID_AT]), r.out)
})

// ---- lints (ticket 11) ----

test("a key the context does not define is a jsonld-undefined-term warning on the key", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.LINTS)
  assert.deepEqual(rangesOf(diags, "jsonld-undefined-term"), [F.LINTS_UNDEFINED], show(diags))
  assert.equal(withCode(diags, "jsonld-undefined-term")[0].severity, 2)
  // Keywords, compact IRIs, absolute IRIs, defined terms and @vocab keys are all defined.
  for (const text of [F.LINTS_CLEAN, F.VOCAB_KEYS]) {
    const other = await openScratch(t, text)
    assert.deepEqual(withCode(other.diags, "jsonld-undefined-term"), [], show(other.diags))
  }
})

test("a misspelled vocabulary key is unknown-term with a did-you-mean fix", { timeout: 25_000 }, async (t) => {
  const { uri, diags } = await openScratch(t, F.LINTS)
  const typos = withCode(diags, "unknown-term").filter((d) => F.rangeKey(d.range) === F.rangeKey(F.LINTS_KEY_TYPO))
  assert.equal(typos.length, 1, show(diags))
  assert.equal(typos[0].severity, 2)
  const texts = await fixedTexts(s, uri, F.LINTS, typos[0])
  assert.ok(texts.includes(F.LINTS.replace('"rdfs:labl"', '"rdfs:label"')), JSON.stringify(texts))
})

test("a misspelled vocabulary IRI in a term definition is unknown-term on the definition, not on each use", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.LINTS)
  assert.deepEqual(sortRanges(rangesOf(diags, "unknown-term")), sortRanges([F.LINTS_DEF_TYPO, F.LINTS_KEY_TYPO]), show(diags))
})

test("a deprecated vocabulary term reached through the context is deprecated-term", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.LINTS)
  const d = withCode(diags, "deprecated-term")
  assert.deepEqual(d.map((x) => x.range), [F.LINTS_DEPRECATED], show(diags))
  assert.deepEqual(d[0].tags, [2])
})

test("an invalid IRI is invalid-iri at the value", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.LINTS)
  assert.deepEqual(rangesOf(diags, "invalid-iri"), [F.LINTS_BAD_IRI], show(diags))
})

test("a malformed language tag is invalid-language-tag at the tag", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.LINTS)
  assert.deepEqual(rangesOf(diags, "invalid-language-tag"), [F.LINTS_BAD_LANG], show(diags))
})

test("a misspelled type expanded through a bundled context's @vocab is unknown-term with a fix", { timeout: 25_000 }, async (t) => {
  const { uri, diags } = await openScratch(t, F.SCHEMA_DOC)
  const d = withCode(diags, "unknown-term")
  assert.deepEqual(d.map((x) => x.range), [F.SCHEMA_DOC_TYPO], show(diags))
  const texts = await fixedTexts(s, uri, F.SCHEMA_DOC, d[0])
  assert.ok(texts.includes(F.SCHEMA_DOC.replace('"Persn"', '"Person"')), JSON.stringify(texts))
})

test("check: each JSON-LD lint at its position", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "lints.jsonld": F.LINTS })
  const r = checkJson(w)
  const expect = {
    "jsonld-undefined-term": [F.LINTS_UNDEFINED],
    "unknown-term": [F.LINTS_DEF_TYPO, F.LINTS_KEY_TYPO],
    "deprecated-term": [F.LINTS_DEPRECATED],
    "invalid-iri": [F.LINTS_BAD_IRI],
    "invalid-language-tag": [F.LINTS_BAD_LANG],
  }
  for (const [code, ranges] of Object.entries(expect)) {
    assert.deepEqual(cliRanges(r.of("lints.jsonld", code)), cliExpected(ranges), `${code}: ${r.out}`)
  }
})

// ---- context loading (ticket 06) ----

// A local web server for remote contexts, counting requests.
let web
let webBase
const hits = []
before(async () => {
  web = http.createServer((req, res) => {
    hits.push(req.url)
    if (req.url === "/ctx.jsonld") {
      res.writeHead(200, { "content-type": "application/ld+json" })
      res.end(F.REMOTE_CONTEXT)
    } else {
      res.writeHead(404)
      res.end()
    }
  })
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  webBase = `http://127.0.0.1:${web.address().port}`
  process.env.NO_PROXY = "127.0.0.1,localhost"
})
after(() => web?.close())

// A server on `cwd` with its own import cache `cache`.
async function startWithCache(cwd, cache, initializationOptions) {
  const prev = process.env.TURTLE_LSP_CACHE_DIR
  process.env.TURTLE_LSP_CACHE_DIR = cache
  // startServer reads the environment before its first await.
  const started = startServer({ cwd, initializationOptions })
  if (prev === undefined) delete process.env.TURTLE_LSP_CACHE_DIR
  else process.env.TURTLE_LSP_CACHE_DIR = prev
  const server = await started
  await server.request("workspace/symbol", { query: "" })
  return server
}

test("offline, bundled contexts load and an unknown remote context is reported, not fetched", { timeout: 25_000 }, async (t) => {
  for (const url of F.BUNDLED_CONTEXTS) {
    const { diags } = await openScratch(t, F.remoteDoc(url))
    assert.deepEqual(withCode(diags, "jsonld-context"), [], `${url}: ${show(diags)}`)
  }
  const before = hits.length
  const url = `${webBase}/ctx.jsonld`
  const { diags } = await openScratch(t, F.remoteDoc(url))
  assert.deepEqual(rangesOf(diags, "jsonld-context"), [F.remoteDocUrl(url)], show(diags))
  const unknown = await openScratch(t, F.remoteDoc(F.UNKNOWN_CONTEXT_URL))
  assert.deepEqual(rangesOf(unknown.diags, "jsonld-context"), [F.remoteDocUrl(F.UNKNOWN_CONTEXT_URL)], show(unknown.diags))
  await sleep(300)
  assert.deepEqual(hits.slice(before), [], "nothing goes online by default")
})

test("with imports.network on, a remote context is fetched once and then served from the cache", { timeout: 25_000 }, async () => {
  const root = tempDir("turtle-jsonld-net-")
  const cache = path.join(root, "cache")
  const dir = path.join(root, "ws")
  fs.mkdirSync(dir)
  const url = `${webBase}/ctx.jsonld`
  const text = F.remoteDoc(url)
  fs.writeFileSync(path.join(dir, "remote.jsonld"), text)
  const uri = uriOf(path.join(dir, "remote.jsonld"))
  const before = hits.length
  const loaded = (d) => withCode(d, "jsonld-context").length === 0 && withCode(d, "jsonld-undefined-term").length > 0
  try {
    for (const network of [true, false]) {
      const server = await startWithCache(dir, cache, { imports: { network } })
      try {
        server.open(uri, text, "jsonld")
        const diags = await until(server, uri, loaded, 4000)
        assert.deepEqual(withCode(diags, "jsonld-context"), [], `network ${network}: ${show(diags)}`)
        // "colour" comes from the context; "flavour" does not.
        assert.deepEqual(rangesOf(diags, "jsonld-undefined-term"), [F.REMOTE_DOC_FLAVOUR], `network ${network}: ${show(diags)}`)
      } finally {
        await server.stop()
      }
    }
    assert.deepEqual(hits.slice(before), ["/ctx.jsonld"], "fetched once, then cached")
    assert.ok(fs.existsSync(cache) && fs.readdirSync(cache, { recursive: true }).length > 0, "cached in the imports cache directory")
  } finally {
    fs.rmSync(root, { recursive: true, force: true })
  }
})

test("a context in a file next to the document is loaded", { timeout: 25_000 }, async (t) => {
  const { diags } = await openScratch(t, F.USES_CTX_FILE)
  assert.deepEqual(withCode(diags, "jsonld-context"), [], show(diags))
  assert.deepEqual(rangesOf(diags, "jsonld-undefined-term"), [F.USES_CTX_FILE_TYPO], show(diags))
})

test("check: a remote context is fetched only with --network", { timeout: 25_000 }, async (t) => {
  const url = `${webBase}/ctx.jsonld`
  const w = workspace(t, { "remote.jsonld": F.remoteDoc(url) })
  const before = hits.length
  const off = await runAsync(w, ["check", "--format", "json"])
  const offDiags = JSON.parse(off.out || '{"files":[]}').files.flatMap((f) => f.diagnostics)
  assert.deepEqual(
    offDiags.filter((d) => d.code === "jsonld-context").map((d) => d.range),
    [F.toCli(F.remoteDocUrl(url))],
    off.out,
  )
  assert.equal(off.code, 1)
  assert.deepEqual(hits.slice(before), [])
  const on = await runAsync(w, ["check", "--format", "json", "--network"])
  const onDiags = JSON.parse(on.out || '{"files":[]}').files.flatMap((f) => f.diagnostics)
  assert.deepEqual(onDiags.filter((d) => d.code === "jsonld-context"), [], on.out)
  assert.deepEqual(
    onDiags.filter((d) => d.code === "jsonld-undefined-term").map((d) => d.range),
    [F.toCli(F.REMOTE_DOC_FLAVOUR)],
    on.out,
  )
  assert.deepEqual(hits.slice(before), ["/ctx.jsonld"])
})

// ---- cross-file knowledge (ticket 09) ----

test("Turtle completion offers terms a JSON-LD file describes", { timeout: 25_000 }, async (t) => {
  const text = "@prefix ex: <https://jsonld.example.org/def/> .\nex:w2 a ex:Gad\n"
  const { uri } = await openScratch(t, text, "ttl")
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position: { line: 1, character: 14 } })
  const labels = (Array.isArray(res) ? res : res?.items ?? []).map((i) => i.label)
  assert.ok(labels.includes("ex:Gadget"), labels.slice(0, 30).join(", "))
})

test("SPARQL completion offers terms a JSON-LD file describes", { timeout: 25_000 }, async (t) => {
  const text = "PREFIX ex: <https://jsonld.example.org/def/>\nSELECT * WHERE { ?w a ex:Gad }\n"
  const { uri } = await openScratch(t, text, "rq")
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position: { line: 1, character: 28 } })
  const labels = (Array.isArray(res) ? res : res?.items ?? []).map((i) => i.label)
  assert.ok(labels.includes("ex:Gadget"), labels.slice(0, 30).join(", "))
})

test("Turtle hover shows what a JSON-LD file says about a term", { timeout: 25_000 }, async () => {
  const uri = await openFile("uses.ttl")
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: F.inside(F.USES_WIDGET, 4) })
  assert.match(JSON.stringify(h?.contents ?? ""), /Widget from JSON-LD/)
})

test("references from Turtle include the JSON-LD @id that describes the term", { timeout: 25_000 }, async () => {
  const uri = await openFile("uses.ttl")
  const res = await s.request("textDocument/references", {
    textDocument: { uri },
    position: F.inside(F.USES_WIDGET, 4),
    context: { includeDeclaration: true },
  })
  const found = locations(res).filter((l) => l.uri === fileUri("vocab.jsonld"))
  assert.deepEqual(found.map((l) => l.range), [F.VOCAB_WIDGET], JSON.stringify(res))
})

test("terms a JSON-LD file describes are not undescribed-term in Turtle; a typo of one is", { timeout: 25_000 }, async () => {
  const uri = await openFile("uses.ttl")
  const diags = await until(s, uri, (d) => withCode(d, "undescribed-term").length > 0)
  assert.deepEqual(rangesOf(diags, "undescribed-term"), [F.USES_GADGT], show(diags))
  const texts = await fixedTexts(s, uri, F.USES, withCode(diags, "undescribed-term")[0])
  assert.ok(texts.includes(F.USES.replace("ex:Gadgt", "ex:Gadget")), JSON.stringify(texts))
})

test("owl:imports of an ontology a JSON-LD file declares resolve", { timeout: 25_000 }, async () => {
  const uri = await openFile("uses.ttl")
  const diags = await until(s, uri, (d) => withCode(d, "unresolved-import").length === 0 && withCode(d, "undescribed-term").length > 0)
  assert.deepEqual(rangesOf(diags, "unresolved-import"), [], show(diags))
  // The JSON-LD file was read: its terms are known.
  assert.deepEqual(rangesOf(diags, "undescribed-term"), [F.USES_GADGT], show(diags))
})

test("in JSON-LD, a typo of a term Turtle describes is undescribed-term with a fix", { timeout: 25_000 }, async (t) => {
  const { uri } = await openScratch(t, F.BACK)
  const diags = await until(s, uri, (d) => withCode(d, "undescribed-term").length > 0)
  assert.deepEqual(rangesOf(diags, "undescribed-term"), [F.BACK_TYPO], show(diags))
  const texts = await fixedTexts(s, uri, F.BACK, withCode(diags, "undescribed-term")[0])
  assert.ok(texts.includes(F.BACK.replace('"ex:Sprockett"', '"ex:Sprocket"')), JSON.stringify(texts))
})

test("in JSON-LD, a key expanded through @vocab to an undescribed own term is undescribed-term", { timeout: 25_000 }, async (t) => {
  const { uri } = await openScratch(t, F.VOCAB_KEYS)
  const diags = await until(s, uri, (d) => withCode(d, "undescribed-term").length > 0)
  assert.deepEqual(rangesOf(diags, "undescribed-term"), [F.VOCAB_KEYS_TYPO], show(diags))
})

test("check: JSON and SARIF output name JSON-LD files and their positions", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "broken.jsonld": F.SYNTAX })
  const json = checkJson(w)
  assert.deepEqual(json.report.files.map((f) => f.path), ["broken.jsonld"])
  assert.deepEqual(cliRanges(json.of("broken.jsonld", "json-syntax")), cliExpected([F.SYNTAX_MISSING_COMMA, F.SYNTAX_BAD_LITERAL]))
  assert.ok(json.of("broken.jsonld", "json-syntax").every((d) => d.severity === "error"))
  assert.equal(json.report.summary.errors, 2)

  const sarif = run(w, ["check", "--format", "sarif"])
  assert.equal(sarif.code, 1)
  const results = JSON.parse(sarif.out).runs[0].results
  const regions = results
    .filter((r) => r.ruleId === "json-syntax")
    .map((r) => {
      const loc = r.locations[0].physicalLocation
      assert.equal(loc.artifactLocation.uri, "broken.jsonld")
      return `${loc.region.startLine}:${loc.region.startColumn}-${loc.region.endLine}:${loc.region.endColumn}`
    })
    .sort()
  assert.deepEqual(regions, ["4:16-4:17", "6:47-6:50"])
  assert.ok(results.filter((r) => r.ruleId === "json-syntax").every((r) => r.level === "error"))
})

test("check: JSON-LD warnings exit as Turtle warnings do", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "lints.jsonld": F.LINTS })
  const plain = checkJson(w, ["--select", "jsonld-undefined-term"])
  assert.equal(plain.code, 0, "warnings alone pass")
  assert.deepEqual(cliRanges(plain.of("lints.jsonld")), cliExpected([F.LINTS_UNDEFINED]), plain.out)
  assert.equal(run(w, ["check", "--select", "jsonld-undefined-term", "--max-warnings=0"]).code, 1)
  const ignored = checkJson(w, ["--ignore", "jsonld-undefined-term"])
  assert.deepEqual(ignored.of("lints.jsonld", "jsonld-undefined-term"), [])
  fs.writeFileSync(path.join(w, "broken.jsonld"), F.SYNTAX)
  assert.equal(run(w, ["check"]).code, 1)
  assert.equal(run(w, ["check", "--exit-zero"]).code, 0)
})

test("the editor and check report the same diagnostics on a mixed Turtle and JSON-LD project", { timeout: 25_000 }, async (t) => {
  const files = { "vocab.jsonld": F.VOCAB, "uses.ttl": F.USES, "lints.jsonld": F.LINTS, "broken.jsonld": F.SYNTAX, "back.jsonld": F.BACK }
  const w = workspace(t, files)
  const cli = checkJson(w)
  const severity = { error: 1, warning: 2, info: 3, hint: 4 }
  const cliKeys = (name) =>
    cli.of(name).map((d) => `${d.range.start.line - 1}:${d.range.start.column - 1}-${d.range.end.line - 1}:${d.range.end.column - 1} ${severity[d.severity]} ${d.code}`).sort()
  const server = await startWithCache(w, path.join(w, ".cache"))
  try {
    for (const [name, text] of Object.entries(files)) server.open(uriOf(path.join(w, name)), text, languageOf(name))
    for (const name of Object.keys(files)) await server.diagnosticsFor(uriOf(path.join(w, name)))
    // Settled: no publication for 1.5 s (SHACL waits 1 s after a change).
    let n = -1
    const end = Date.now() + 8000
    while (n !== server.publications.length && Date.now() < end) {
      n = server.publications.length
      await sleep(1500)
    }
    for (const name of ["lints.jsonld", "broken.jsonld", "back.jsonld"]) assert.ok(cliKeys(name).length > 0, `check reports ${name}: ${cli.out}`)
    for (const name of Object.keys(files)) {
      const editor = (await server.diagnosticsFor(uriOf(path.join(w, name)))).map((d) => `${F.rangeKey(d.range)} ${d.severity} ${d.code}`).sort()
      assert.deepEqual(editor, cliKeys(name), name)
    }
  } finally {
    await server.stop()
  }
})

// ---- SHACL (ticket 10) ----

const shaclRanges = (diags) => sortRanges(rangesOf(diags, "shacl-violation"))

async function shaclSession(t, files) {
  const w = workspace(t, files)
  const server = await startWithCache(w, path.join(w, ".cache"))
  t.after(() => server.stop())
  // Opens and saves `name`, which validates.
  const save = (name) => {
    const uri = uriOf(path.join(w, name))
    server.open(uri, files[name], languageOf(name))
    server.notify("textDocument/didSave", { textDocument: { uri } })
    return uri
  }
  return { w, server, save }
}

test("Turtle shapes validate JSON-LD data, each result on the key, value or @id that breaks the shape", { timeout: 25_000 }, async (t) => {
  const { server, save } = await shaclSession(t, { "shapes.ttl": F.PERSON_SHAPES, "people.jsonld": F.PEOPLE })
  const uri = save("people.jsonld")
  const diags = await until(server, uri, (d) => withCode(d, "shacl-violation").length >= F.PEOPLE_RESULTS.length, 5000)
  assert.deepEqual(shaclRanges(diags), sortRanges(F.PEOPLE_RESULTS), show(diags))
  assert.ok(withCode(diags, "shacl-violation").every((d) => d.severity === 1))
})

test("check: SHACL results in JSON-LD data at the same positions", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "shapes.ttl": F.PERSON_SHAPES, "people.jsonld": F.PEOPLE })
  const r = checkJson(w)
  assert.equal(r.code, 1)
  assert.deepEqual(cliRanges(r.of("people.jsonld", "shacl-violation")), cliExpected(F.PEOPLE_RESULTS), r.out)
})

test("check: JSON-LD named graphs follow the TriG rules: never merged, results that need a dataset in it, shared results not repeated", { timeout: 25_000 }, (t) => {
  const w = workspace(t, {
    "shapes.ttl": F.ASSET_SHAPES,
    "shared.ttl": F.SHARED_ASSETS,
    "run1.jsonld": F.assetRun("run1", '"A"'),
    "run2.jsonld": F.assetRun("run2", '"B"'),
    "run3.jsonld": F.assetRun("run3", '["A", "B"]'),
    "run4.jsonld": F.SECOND_DESIGNATION,
    "mentions.jsonld": F.MENTIONS,
  })
  const r = checkJson(w)
  const got = {}
  for (const f of r.report.files) {
    const v = f.diagnostics.filter((d) => d.code === "shacl-violation")
    if (v.length) got[f.path] = cliRanges(v)
  }
  assert.deepEqual(
    got,
    {
      // ex:p8 has one designation in each of run1 and run2: no result; run3 gives it two.
      "run3.jsonld": cliExpected([F.ASSET_RUN_DESIGNATION]),
      // A second designation for the shared ex:p9 exists only with run4 merged in.
      "run4.jsonld": cliExpected([F.SECOND_DESIGNATION_KEY]),
      // ex:p7's missing designation is the shared file's; mentions.jsonld does not repeat it.
      "shared.ttl": cliExpected([F.SHARED_P7]),
    },
    r.out,
  )
})

test("a JSON-LD dataset that mentions a violating shared node does not repeat its result in the editor", { timeout: 25_000 }, async (t) => {
  const files = { "shapes.ttl": F.ASSET_SHAPES, "shared.ttl": F.SHARED_ASSETS, "mentions.jsonld": F.MENTIONS, "run3.jsonld": F.assetRun("run3", '["A", "B"]') }
  const { server, save } = await shaclSession(t, files)
  const shared = save("shared.ttl")
  const mentions = save("mentions.jsonld")
  const run3 = save("run3.jsonld")
  const d3 = await until(server, run3, (d) => withCode(d, "shacl-violation").length > 0, 5000)
  assert.deepEqual(shaclRanges(d3), [F.ASSET_RUN_DESIGNATION], show(d3))
  // The other files' diagnostics come from the same validation round: wait
  // for the shared file's result rather than reading an earlier round.
  const ds = await until(server, shared, (d) => withCode(d, "shacl-violation").length > 0, 5000)
  assert.deepEqual(shaclRanges(ds), [F.SHARED_P7], show(ds))
  const dm = await server.diagnosticsFor(mentions)
  assert.deepEqual(shaclRanges(dm), [], show(dm))
})

test("check: a SHACL-SPARQL constraint reports a JSON-LD focus node on its @id", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "shapes.ttl": F.ASSET_SHAPES, "bad.jsonld": F.BAD_ASSET })
  const r = checkJson(w)
  assert.deepEqual(cliRanges(r.of("bad.jsonld", "shacl-violation")), cliExpected([F.BAD_ASSET_ID]), r.out)
})

test("check: shapes written in JSON-LD validate Turtle data", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "shapes.jsonld": F.JSONLD_SHAPES, "data.ttl": F.THING_DATA })
  const r = checkJson(w)
  assert.equal(r.code, 1)
  assert.deepEqual(cliRanges(r.of("data.ttl", "shacl-violation")), cliExpected([F.THING_T1]), r.out)
})

// =====================================================================================
// Phase 2: editing inside JSON-LD
// =====================================================================================

async function openEdit() {
  return openFile("edit.jsonld")
}
const completionLabels = async (uri, position) => {
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position })
  return (Array.isArray(res) ? res : res?.items ?? []).map((i) => i.label)
}

test("completion in a key offers the context's terms", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const labels = await completionLabels(uri, F.inside(F.EDIT_ANN_NAME, 2))
  assert.ok(labels.includes("name"), labels.slice(0, 30).join(", "))
  assert.ok(labels.includes("knows"), labels.slice(0, 30).join(", "))
})

test("completion in a key offers @vocab terms the workspace describes", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const labels = await completionLabels(uri, F.inside(F.EDIT_BOB_WEIGHT, 2))
  assert.ok(labels.includes("weight"), labels.slice(0, 30).join(", "))
})

test("completion in an @type value offers @vocab classes", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const labels = await completionLabels(uri, F.inside(F.EDIT_ANN_TYPE, 3))
  assert.ok(labels.includes("Widget"), labels.slice(0, 30).join(", "))
})

const completionItems = async (uri, position) => {
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position })
  return Array.isArray(res) ? res : res?.items ?? []
}

test("completion in a half-typed key (an unclosed string) offers the context's terms inside the quotes", { timeout: 25_000 }, async (t) => {
  const text = `{
  "@context": { "foaf": "http://xmlns.com/foaf/0.1/", "knows": "foaf:knows" },
  "@id": "x",
  "kn
}
`
  const { uri } = await openScratch(t, text)
  const items = await completionItems(uri, { line: 3, character: 5 })
  const knows = items.find((i) => i.label === "knows")
  assert.ok(knows, items.slice(0, 30).map((i) => i.label).join(", "))
  assert.equal(knows.detail, "http://xmlns.com/foaf/0.1/knows")
  assert.deepEqual(knows.textEdit.range, { start: { line: 3, character: 3 }, end: { line: 3, character: 5 } })
  assert.equal(knows.textEdit.newText, "knows")
})

test("completion in a key offers the terms of a scoped context there only", { timeout: 25_000 }, async (t) => {
  const text = `{
  "@context": {
    "@vocab": "https://jsonld.example.org/def/",
    "part": { "@id": "partOf", "@context": { "colourName": "https://jsonld.example.org/def/colour" } }
  },
  "part": { "co": 1 },
  "other": { "co": 1 }
}
`
  const { uri } = await openScratch(t, text)
  const inPart = await completionLabels(uri, { line: 5, character: 14 })
  assert.ok(inPart.includes("colourName"), inPart.slice(0, 30).join(", "))
  const inOther = await completionLabels(uri, { line: 6, character: 15 })
  assert.ok(!inOther.includes("colourName"), inOther.slice(0, 30).join(", "))
  assert.ok(inOther.includes("part"), inOther.slice(0, 30).join(", "))
})

test("completing a compact IRI whose prefix the context lacks adds the prefix to @context", { timeout: 25_000 }, async (t) => {
  const text = `{
  "@context": {
    "@vocab": "https://jsonld.example.org/def/"
  },
  "@id": "x",
  "foaf:na"
}
`
  const { uri } = await openScratch(t, text)
  const items = await completionItems(uri, { line: 5, character: 10 })
  const name = items.find((i) => i.label === "foaf:name")
  assert.ok(name, items.slice(0, 30).map((i) => i.label).join(", "))
  assert.equal(
    F.applyEdits(text, [name.textEdit, ...(name.additionalTextEdits ?? [])]),
    `{
  "@context": {
    "foaf": "http://xmlns.com/foaf/0.1/",
    "@vocab": "https://jsonld.example.org/def/"
  },
  "@id": "x",
  "foaf:name"
}
`,
  )
})

test("hover on a key shows its expanded IRI, over the key", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_NAME, 1) })
  assert.match(JSON.stringify(h?.contents ?? ""), /http:\/\/xmlns\.com\/foaf\/0\.1\/name/)
  assert.deepEqual(h.range, F.EDIT_ANN_NAME)
})

test("hover on an @type value shows its expanded IRI and what the workspace says about it", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_TYPE, 1) })
  const md = JSON.stringify(h?.contents ?? "")
  assert.match(md, /https:\/\/jsonld\.example\.org\/def\/Widget/)
  assert.match(md, /Widget from JSON-LD/)
  assert.deepEqual(h.range, F.EDIT_ANN_TYPE)
})

test("go to definition on a key goes to its @context entry", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const res = await s.request("textDocument/definition", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_NAME, 1) })
  assert.deepEqual(locations(res), [{ uri, range: F.EDIT_NAME_DEF }])
})

test("go to definition on an @vocab key goes to where the workspace describes the term", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const res = await s.request("textDocument/definition", { textDocument: { uri }, position: F.inside(F.EDIT_BOB_WEIGHT, 1) })
  assert.deepEqual(locations(res), [{ uri: fileUri("vocab.jsonld"), range: F.VOCAB_WEIGHT }])
})

test("go to definition on a key goes into a context file", { timeout: 25_000 }, async (t) => {
  const { uri } = await openScratch(t, F.USES_CTX_FILE)
  const res = await s.request("textDocument/definition", { textDocument: { uri }, position: F.inside(F.USES_CTX_FILE_NICKNAME, 1) })
  assert.deepEqual(locations(res), [{ uri: fileUri("people.context.jsonld"), range: F.CTX_FILE_NICKNAME }])
})

test("references on a key list its @context entry, its uses and the IRI's uses elsewhere", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const res = await s.request("textDocument/references", {
    textDocument: { uri },
    position: F.inside(F.EDIT_ANN_NAME, 1),
    context: { includeDeclaration: true },
  })
  const here = locations(res).filter((l) => l.uri === uri).map((l) => l.range)
  assert.deepEqual(sortRanges(here), sortRanges([F.EDIT_NAME_DEF, F.EDIT_ANN_NAME, F.EDIT_BOB_NAME]), JSON.stringify(res))
  assert.ok(
    locations(res).some((l) => l.uri === fileUri("uses.ttl") && F.rangeKey(l.range) === F.rangeKey(F.USES_FOAF_NAME)),
    JSON.stringify(res),
  )
})

test("highlights on a key mark its @context entry as written and its uses as read", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const res = await s.request("textDocument/documentHighlight", { textDocument: { uri }, position: F.inside(F.EDIT_BOB_NAME, 1) })
  const got = (res ?? []).map((h) => `${F.rangeKey(h.range)} ${h.kind}`).sort()
  const want = [`${F.rangeKey(F.EDIT_NAME_DEF)} 3`, `${F.rangeKey(F.EDIT_ANN_NAME)} 2`, `${F.rangeKey(F.EDIT_BOB_NAME)} 2`].sort()
  assert.deepEqual(got, want, JSON.stringify(res))
})

test("renaming a term renames its @context entry and every use, and nothing else", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const prepared = await s.request("textDocument/prepareRename", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_NAME, 1) })
  assert.deepEqual(prepared?.range ?? prepared, F.EDIT_ANN_NAME)
  const edit = await s.request("textDocument/rename", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_NAME, 1), newName: "fullName" })
  const changes = changesOf(edit)
  assert.deepEqual(Object.keys(changes), [uri], JSON.stringify(edit))
  assert.equal(F.applyEdits(F.EDIT, changes[uri]), F.EDIT.replaceAll('"name":', '"fullName":'))
})

test("renaming a keyword alias renames its @context entry and every use", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const edit = await s.request("textDocument/rename", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_ID_KEY, 1), newName: "ident" })
  const changes = changesOf(edit)
  assert.deepEqual(Object.keys(changes), [uri], JSON.stringify(edit))
  assert.equal(F.applyEdits(F.EDIT, changes[uri]), F.EDIT.replaceAll('"id":', '"ident":'))
})

test("renaming an IRI from Turtle rewrites it in JSON-LD, compact and @vocab-relative", { timeout: 25_000 }, async () => {
  const uri = await openFile("uses.ttl")
  const edit = await s.request("textDocument/rename", { textDocument: { uri }, position: F.inside(F.USES_WIDGET, 4), newName: "ex:Doohickey" })
  const changes = changesOf(edit)
  assert.equal(F.applyEdits(F.USES, changes[uri] ?? []), F.USES.replace("ex:Widget", "ex:Doohickey"))
  assert.deepEqual(changes[fileUri("vocab.jsonld")], [{ range: F.VOCAB_WIDGET, newText: "ex:Doohickey" }])
  assert.deepEqual(
    sortRanges((changes[fileUri("edit.jsonld")] ?? []).map((e) => e.range)),
    sortRanges([F.EDIT_ANN_TYPE, F.EDIT_BOB_TYPE]),
  )
  assert.equal(F.applyEdits(F.EDIT, changes[fileUri("edit.jsonld")] ?? []), F.EDIT.replaceAll('"Widget"', '"Doohickey"'))
})

test("renaming a term of a bundled context is refused", { timeout: 25_000 }, async (t) => {
  const text = '{ "@context": "https://schema.org/", "@type": "Person", "name": "Ann" }'
  const { uri } = await openScratch(t, text)
  const at = { line: 0, character: text.indexOf('"name"') + 2 }
  // Both messages name the context.
  await assert.rejects(s.request("textDocument/prepareRename", { textDocument: { uri }, position: at }), /schema\.org/)
  await assert.rejects(s.request("textDocument/rename", { textDocument: { uri }, position: at, newName: "fullName" }), /schema\.org/)
})

test("renaming an IRI that a JSON-LD document reaches only through a bundled term is refused", { timeout: 25_000 }, async (t) => {
  const { uri: jsonld } = await openScratch(t, '{ "@context": "https://schema.org/", "@id": "https://jsonld.example.org/def/w30", "name": "Thirty" }')
  const ttl = '@prefix schema: <http://schema.org/> .\n<https://jsonld.example.org/def/w31> schema:name "Thirty-one" .\n'
  const { uri } = await openScratch(t, ttl, "ttl")
  const at = { line: 1, character: ttl.split("\n")[1].indexOf("schema:name") + 8 }
  // The message names the JSON-LD document.
  const name = jsonld.split("/").pop()
  await assert.rejects(s.request("textDocument/rename", { textDocument: { uri }, position: at, newName: "schema:fullName" }), new RegExp(name))
})

test("renaming a term to an invalid name is refused", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  for (const newName of ["", "@name", "full name", 'full"name', "foaf:fullName"]) {
    await assert.rejects(
      s.request("textDocument/rename", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_NAME, 1), newName }),
      undefined,
      JSON.stringify(newName),
    )
  }
})

test("renaming a term of a workspace context file renames it there and in the documents using it", { timeout: 25_000 }, async (t) => {
  const { uri } = await openScratch(t, F.USES_CTX_FILE)
  const prepared = await s.request("textDocument/prepareRename", { textDocument: { uri }, position: F.inside(F.USES_CTX_FILE_NICKNAME, 1) })
  assert.deepEqual(prepared?.range ?? prepared, F.USES_CTX_FILE_NICKNAME)
  const edit = await s.request("textDocument/rename", { textDocument: { uri }, position: F.inside(F.USES_CTX_FILE_NICKNAME, 1), newName: "alias" })
  const changes = changesOf(edit)
  assert.deepEqual(Object.keys(changes).sort(), [fileUri("people.context.jsonld"), uri].sort(), JSON.stringify(edit))
  assert.deepEqual(changes[fileUri("people.context.jsonld")], [{ range: F.CTX_FILE_NICKNAME, newText: "alias" }])
  assert.equal(F.applyEdits(F.USES_CTX_FILE, changes[uri]), F.USES_CTX_FILE.replace('"nickname":', '"alias":'))
})

test("renaming an IRI out of @vocab writes the JSON-LD use as a compact IRI", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const edit = await s.request("textDocument/rename", { textDocument: { uri }, position: F.inside(F.EDIT_ANN_TYPE, 1), newName: "foaf:Gizmo" })
  const changes = changesOf(edit)
  assert.equal(F.applyEdits(F.EDIT, changes[uri] ?? []), F.EDIT.replaceAll('"Widget"', '"foaf:Gizmo"'))
  // No foaf prefix in vocab.jsonld: absolute, without angle brackets.
  assert.deepEqual(changes[fileUri("vocab.jsonld")], [{ range: F.VOCAB_WIDGET, newText: "http://xmlns.com/foaf/0.1/Gizmo" }])
  assert.equal(F.applyEdits(F.USES, changes[fileUri("uses.ttl")] ?? []), F.USES.replace("ex:Widget", "foaf:Gizmo"))
})

test("formatting JSON-LD keeps its JSON value, and formatted text stays as it is", { timeout: 25_000 }, async (t) => {
  const { uri } = await openScratch(t, F.MESSY)
  const options = { tabSize: 2, insertSpaces: true }
  const edits = (await s.request("textDocument/formatting", { textDocument: { uri }, options })) ?? []
  const once = F.applyEdits(F.MESSY, edits)
  assert.notEqual(once, F.MESSY)
  // The same JSON value (key order aside) is the same graph.
  assert.deepEqual(JSON.parse(once), JSON.parse(F.MESSY))
  s.change(uri, once)
  await s.diagnosticsFor(uri)
  assert.deepEqual((await s.request("textDocument/formatting", { textDocument: { uri }, options })) ?? [], [])
})

test("format: JSON-LD files are formatted in place, and a second run changes nothing", { timeout: 25_000 }, (t) => {
  const w = workspace(t, { "messy.jsonld": F.MESSY })
  const check = run(w, ["format", "--check"])
  assert.equal(check.code, 1)
  assert.deepEqual(check.lines, ["messy.jsonld"])
  assert.equal(run(w, ["format"]).code, 0)
  const once = fs.readFileSync(path.join(w, "messy.jsonld"), "utf8")
  assert.deepEqual(JSON.parse(once), JSON.parse(F.MESSY))
  assert.equal(run(w, ["format", "--check"]).code, 0)
  assert.equal(fs.readFileSync(path.join(w, "messy.jsonld"), "utf8"), once)
})

test("semantic tokens mark keywords, properties and classes", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const types = s.init.capabilities.semanticTokensProvider.legend.tokenTypes
  const res = await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
  const tokens = decodeTokens(res?.data ?? [], types)
  const tokenAt = (r) => tokens.find((x) => x.line === r.start.line && x.character === r.start.character)
  const expect = [
    [F.EDIT_CONTEXT_KEYWORD, "keyword"],
    [F.EDIT_ANN_ID_KEY, "keyword"],
    [F.EDIT_ANN_NAME, "property"],
    [F.EDIT_ANN_KNOWS, "property"],
    [F.EDIT_ANN_TYPE, "class"],
  ]
  for (const [r, type] of expect) {
    assert.deepEqual(tokenAt(r), { line: r.start.line, character: r.start.character, length: r.end.character - r.start.character, type }, JSON.stringify(tokens))
  }
})

test("an alias used as a value is a keyword", { timeout: 25_000 }, async (t) => {
  const text = '{ "@context": { "ident": "@id", "name": "http://schema.org/name" }, "ident": "http://ex.org/a", "@type": "ident", "name": "A" }'
  const { uri } = await openScratch(t, text)
  const types = s.init.capabilities.semanticTokensProvider.legend.tokenTypes
  const res = await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
  const tokens = decodeTokens(res?.data ?? [], types)
  const typeAt = (needle) => tokens.find((x) => x.character === text.indexOf(needle) + 1)?.type
  const shown = JSON.stringify(tokens)
  assert.equal(typeAt('"ident": "@id"'), "keyword", shown) // definition key
  assert.equal(typeAt('"ident": "http'), "keyword", shown) // as a key
  assert.equal(typeAt('"ident", "name"'), "keyword", shown) // as a value
  assert.equal(typeAt('"name": "A"'), "property", shown)
})

test("a semantic tokens range request returns only tokens in range", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const types = s.init.capabilities.semanticTokensProvider.legend.tokenTypes
  const r = F.EDIT_ANN_OBJECT
  const res = await s.request("textDocument/semanticTokens/range", { textDocument: { uri }, range: r })
  const tokens = decodeTokens(res?.data ?? [], types)
  assert.ok(tokens.length >= 4, JSON.stringify(tokens))
  for (const x of tokens) {
    assert.equal(x.line, r.start.line, JSON.stringify(tokens))
    assert.ok(x.character >= r.start.character && x.character + x.length <= r.end.character, JSON.stringify(tokens))
  }
  assert.ok(tokens.some((x) => x.character === F.EDIT_ANN_NAME.start.character && x.type === "property"), JSON.stringify(tokens))
})

test("the outline lists each node object by its @id", { timeout: 25_000 }, async () => {
  const uri = await openEdit()
  const syms = flattenSymbols(await s.request("textDocument/documentSymbol", { textDocument: { uri } }))
  const byName = (name) => syms.find((x) => x.name === name)
  assert.deepEqual(byName("ann") && { range: byName("ann").range, selectionRange: byName("ann").selectionRange }, { range: F.EDIT_ANN_OBJECT, selectionRange: F.EDIT_ANN_ID }, JSON.stringify(syms))
  assert.deepEqual(byName("bob") && { range: byName("bob").range, selectionRange: byName("bob").selectionRange }, { range: F.EDIT_BOB_OBJECT, selectionRange: F.EDIT_BOB_ID }, JSON.stringify(syms))
})

test("the outline nests a named graph's nodes under the graph", { timeout: 25_000 }, async (t) => {
  const { uri } = await openScratch(t, F.assetRun("run1", '"A"'))
  const syms = await s.request("textDocument/documentSymbol", { textDocument: { uri } })
  const graph = flattenSymbols(syms).find((x) => x.name === "ex:run1")
  assert.ok(graph, JSON.stringify(syms))
  assert.deepEqual(flattenSymbols(graph.children).map((x) => x.name), ["ex:p8"])
})
