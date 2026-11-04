// The server worker's message handling (src/web/handler.ts) in Node, with the
// real engine (web/app/engine.wasm, built by `node web/build.mjs`): JSON-RPC
// messages in as the language client posts them, messages out.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../../../web/app/engine.js"
import { createHandler } from "../src/web/handler.ts"

const WASM = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..", "..", "web", "app", "engine.wasm")
const skip = fs.existsSync(WASM) ? false : `${WASM} is missing: build it with \`node web/build.mjs\``
const bytes = skip ? null : fs.readFileSync(WASM)

const ROOT = "vscode-test-web://mount"
const A = `${ROOT}/a.ttl`
const DEFS = `${ROOT}/defs.ttl`
const PREFIX = "@prefix ex: <http://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
const DEFS_TEXT = `${PREFIX}\nex:Thing a rdfs:Class ;\n  rdfs:label "Thing" .\n`
const A_TEXT = `${PREFIX}\nex:x a ex:Thing .\n`
const BROKEN = `${PREFIX}\nex:x a nope:Thing .\n`

// A client: the handler, what it posted, and helpers that speak JSON-RPC.
async function client({ engine } = {}) {
  const posted = []
  const handler = createHandler(engine ?? (await createEngine(bytes)), (m) => posted.push(m))
  let next = 0
  const c = {
    posted,
    handler,
    send: (m) => handler.receive({ jsonrpc: "2.0", ...m }),
    notify: (method, params) => c.send({ method, params }),
    // Sends a request; its answer (the response message) once posted.
    request(method, params, id = ++next) {
      c.send({ id, method, params })
      return c.answer(id)
    },
    async answer(id) {
      await handler.idle()
      return posted.find((m) => m.id === id && !("method" in m))
    },
    // The last diagnostics published for `uri`.
    diagnostics: (uri) => posted.filter((m) => m.method === "textDocument/publishDiagnostics" && m.params.uri === uri).at(-1)?.params.diagnostics,
    open: (uri, text, version = 1) => c.notify("textDocument/didOpen", { textDocument: { uri, languageId: "turtle", version, text } }),
    at: (uri, text, needle) => {
      const i = text.indexOf(needle)
      const before = text.slice(0, i).split("\n")
      return { textDocument: { uri }, position: { line: before.length - 1, character: before.at(-1).length + 1 } }
    },
  }
  return c
}

const initialize = (c) =>
  c.request("initialize", {
    processId: null,
    rootUri: ROOT,
    workspaceFolders: [{ uri: ROOT, name: "mount" }],
    capabilities: { workspace: { configuration: true, didChangeWatchedFiles: { dynamicRegistration: true } } },
  })

test("initialize answers under the client's id, with the server's capabilities", { skip }, async () => {
  const c = await client()
  const r = await initialize(c)
  assert.equal(r.jsonrpc, "2.0")
  assert.ok(r.result.capabilities.hoverProvider, JSON.stringify(r))
  // No watcher registration: the files come in turtle/web/files.
  assert.ok(!c.posted.some((m) => m.method === "client/registerCapability"))
})

test("a request before initialize is refused", { skip }, async () => {
  const c = await client()
  const r = await c.request("textDocument/hover", c.at(A, A_TEXT, "ex:Thing"))
  assert.equal(r.error.code, -32002)
})

test("an opened document gets its diagnostics, under the client's URI", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.open(A, BROKEN)
  await c.handler.idle()
  const list = c.diagnostics(A)
  assert.ok(list?.some((d) => /nope/.test(d.message)), JSON.stringify(list))
  assert.ok(!c.posted.some((m) => JSON.stringify(m).includes("file:///workspace")), "no server URI reaches the client")
})

test("hover", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.open(DEFS, DEFS_TEXT)
  const r = await c.request("textDocument/hover", c.at(DEFS, DEFS_TEXT, "Thing a"))
  assert.match(JSON.stringify(r.result.contents), /Thing/)
})

test("a request cancelled while queued is answered as cancelled and not run", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.open(DEFS, DEFS_TEXT)
  // Both arrive before the worker's next turn, so they are one batch.
  c.send({ id: 41, method: "textDocument/hover", params: c.at(DEFS, DEFS_TEXT, "Thing a") })
  c.send({ method: "$/cancelRequest", params: { id: 41 } })
  c.send({ id: 42, method: "textDocument/hover", params: c.at(DEFS, DEFS_TEXT, "Thing a") })
  const r = await c.answer(41)
  assert.equal(r.error.code, -32800)
  assert.equal(c.posted.filter((m) => m.id === 41).length, 1)
  assert.ok((await c.answer(42)).result)
})

test("a definition in a file only on disk, and closing a file keeps it found", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.notify("turtle/web/files", { changed: [{ uri: DEFS, text: DEFS_TEXT }], deleted: [] })
  c.open(A, A_TEXT)
  const where = c.at(A, A_TEXT, "Thing .")
  const def = await c.request("textDocument/definition", where)
  const uris = (r) => [r.result].flat().map((l) => l.uri ?? l.targetUri)
  assert.deepEqual(uris(def), [DEFS], JSON.stringify(def))
  // The disk file opened and edited in the editor, then closed: the disk text is back, still found.
  c.open(DEFS, `${PREFIX}\nex:Other a rdfs:Class .\n`)
  const edited = await c.request("textDocument/definition", where)
  assert.ok(!uris(edited).includes(DEFS), `the editor's text wins: ${JSON.stringify(edited)}`)
  c.notify("textDocument/didClose", { textDocument: { uri: DEFS } })
  assert.deepEqual(uris(await c.request("textDocument/definition", where)), [DEFS])
  const refs = await c.request("textDocument/references", { ...c.at(DEFS, DEFS_TEXT, "Thing a"), context: { includeDeclaration: true } })
  assert.deepEqual([...new Set(refs.result.map((l) => l.uri))].sort(), [A, DEFS])
  // A file deleted from disk is gone.
  c.notify("turtle/web/files", { changed: [], deleted: [DEFS] })
  const gone = await c.request("textDocument/definition", where)
  assert.ok(!JSON.stringify(gone.result ?? null).includes(DEFS), JSON.stringify(gone))
})

test("turtle/query over the workspace's files, with the prefixes of the root's prefixes.ttl", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.notify("turtle/web/files", {
    changed: [
      { uri: `${ROOT}/prefixes.ttl`, text: "@prefix ex: <http://example.org/> .\n" },
      { uri: `${ROOT}/data.ttl`, text: "ex:p1 ex:feeds ex:v1 .\n" },
      { uri: `${ROOT}/more.nt`, text: "<http://example.org/p2> <http://example.org/feeds> <http://example.org/v2> .\n" },
    ],
    deleted: [],
  })
  const subjects = (r) => (r.error ? r.error.message : r.result.results.bindings.map((b) => b.s.value))
  const FEEDS = "SELECT ?s WHERE { ?s ex:feeds ?o } ORDER BY ?s"
  // files omitted: the query is looked up as a query file at the root, which
  // the browser build knows only by its URI.
  assert.deepEqual(subjects(await c.request("turtle/query", { query: FEEDS })), ["http://example.org/p1", "http://example.org/p2"])
  assert.deepEqual(subjects(await c.request("turtle/query", { query: FEEDS, files: [`${ROOT}/more.nt`] })), ["http://example.org/p2"])
})

test("turtle/web/files never reaches the server, and a malformed one is ignored", { skip }, async () => {
  const real = await createEngine(bytes)
  const sent = []
  const engine = {
    call(op, payload) {
      if (op === "lsp") sent.push(...payload.messages.map((m) => m.method))
      return real.call(op, payload)
    },
  }
  const c = await client({ engine })
  await initialize(c)
  c.notify("turtle/web/files", { changed: [{ uri: DEFS, text: DEFS_TEXT }, { uri: 3 }], deleted: [null] })
  c.notify("turtle/web/files", "nonsense")
  await c.handler.idle()
  assert.ok(!sent.includes("turtle/web/files"))
  assert.equal(sent.filter((m) => m === "textDocument/didOpen").length, 1)
})

test("a configuration request is forwarded, and the client's answer reaches the server", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.open(A, BROKEN)
  await c.handler.idle()
  const code = c.diagnostics(A).find((d) => /nope/.test(d.message)).code
  // A client that only says "something changed" is asked for the section.
  c.notify("workspace/didChangeConfiguration", { settings: null })
  await c.handler.idle()
  const asked = c.posted.find((m) => m.method === "workspace/configuration")
  assert.ok(asked, "the server's request was forwarded")
  assert.equal(typeof asked.id, "string")
  c.send({ id: asked.id, result: [{ lint: { ignore: [code] } }] })
  await c.handler.idle()
  assert.ok(!c.diagnostics(A).some((d) => d.code === code), JSON.stringify(c.diagnostics(A)))
})

test("an engine crash restarts the server with the union and tells the client", { skip }, async () => {
  const real = await createEngine(bytes)
  let crashNext = false
  const engine = {
    call(op, payload) {
      if (crashNext && op === "lsp") {
        crashNext = false
        throw Object.assign(new Error("unreachable"), { crashed: true })
      }
      return real.call(op, payload)
    },
  }
  const c = await client({ engine })
  await initialize(c)
  c.notify("turtle/web/files", { changed: [{ uri: DEFS, text: DEFS_TEXT }], deleted: [] })
  c.open(A, A_TEXT)
  await c.handler.idle()
  crashNext = true
  const where = c.at(A, A_TEXT, "Thing .")
  const crashed = await c.request("textDocument/definition", where)
  assert.ok(crashed.error, JSON.stringify(crashed))
  const shown = c.posted.filter((m) => m.method === "window/showMessage")
  assert.equal(shown.length, 1)
  assert.match(shown[0].params.message, /crashed and was restarted/)
  // The fresh server has the disk file and the editor's document.
  assert.deepEqual([(await c.request("textDocument/definition", where)).result].flat().map((l) => l.uri ?? l.targetUri), [DEFS])
})

test("shutdown, exit and a new session: the disk files are back, the editor's documents are not", { skip }, async () => {
  const c = await client()
  await initialize(c)
  c.notify("turtle/web/files", { changed: [{ uri: DEFS, text: DEFS_TEXT }], deleted: [] })
  c.open(A, A_TEXT)
  assert.equal((await c.request("shutdown", null)).result, null)
  c.notify("exit", null)
  await c.handler.idle()
  const from = c.posted.length
  await initialize(c)
  await c.handler.idle()
  const published = c.posted.slice(from).filter((m) => m.method === "textDocument/publishDiagnostics").map((m) => m.params.uri)
  assert.ok(published.includes(DEFS) && !published.includes(A), JSON.stringify(published))
  c.open(A, A_TEXT)
  const where = c.at(A, A_TEXT, "Thing .")
  assert.deepEqual([(await c.request("textDocument/definition", where)).result].flat().map((l) => l.uri ?? l.targetUri), [DEFS])
})
