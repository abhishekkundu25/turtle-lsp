// The language server inside the browser engine (the editor's) answers as
// the native server does: the same documents and requests go to both, and
// the answers must match once document URIs are mapped. Run
// `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { EmbeddedServer } from "../app/lsp-host.js"
import { SETS } from "../app/samples.js"
import { startServer, tempDir, uriOf, positionOf } from "../../test/lsp-client.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const WORK = "file:///work"
const files = [
  ...structuredClone(SETS.starter.files),
  {
    name: "labels.ttl",
    text: `@prefix ex: <https://toolbox.example.org/def/> .
@prefix id: <https://toolbox.example.org/id/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

id:alice rdfs:label "Alice Smith"@en , "Alice Schmidt"@de .
id:acme rdfs:label "ACME Corporation"@en .
`,
  },
]
const textOf = (name) => files.find((f) => f.name === name).text
const lang = (name) => (/\.(rq|ru|sparql)$/.test(name) ? "sparql" : "turtle")

let native, nativeRoot, wasm, dir
before(async () => {
  dir = tempDir("turtle-web-lsp-")
  nativeRoot = uriOf(dir)
  native = await startServer({ cwd: dir })
  wasm = new EmbeddedServer(await createEngine(fs.readFileSync(path.join(here, "..", "app", "engine.wasm"))))
  await wasm.initialize({ processId: null, rootUri: null, capabilities: {} })
  for (const f of files) {
    native.open(`${nativeRoot}/${f.name}`, f.text, lang(f.name))
    wasm.notify("textDocument/didOpen", { textDocument: { uri: `${WORK}/${f.name}`, languageId: lang(f.name), version: 1, text: f.text } })
  }
  // A save validates SHACL in both.
  for (const f of files) {
    native.notify("textDocument/didSave", { textDocument: { uri: `${nativeRoot}/${f.name}` } })
    wasm.notify("textDocument/didSave", { textDocument: { uri: `${WORK}/${f.name}` } })
  }
  await wasm.settle()
})
after(async () => {
  await native?.stop()
  fs.rmSync(dir, { recursive: true, force: true })
})

// Native answers with the temp directory's URIs; the embedded server with /work.
const mapped = (v) => JSON.parse(JSON.stringify(v ?? null).replaceAll(nativeRoot, WORK))
async function both(method, params) {
  const p = (root) => JSON.parse(JSON.stringify(params).replaceAll("$ROOT", root))
  const [n, w] = await Promise.all([native.request(method, p(nativeRoot)), wasm.request(method, p(WORK))])
  return { native: mapped(n), wasm: w }
}
const at = (name, needle, offset) => ({
  textDocument: { uri: `$ROOT/${name}` },
  position: positionOf(textOf(name), needle, offset),
})
const same = async (method, params) => {
  const r = await both(method, params)
  assert.deepEqual(r.wasm, r.native)
  return r.wasm
}

// --- diagnostics -----------------------------------------------------------------------

test("diagnostics, SHACL included, are the native server's", async () => {
  // Native validates SHACL on a worker thread: wait until publishing settles.
  let n = -1
  const end = Date.now() + 15_000
  while (n !== native.publications.length && Date.now() < end) {
    n = native.publications.length
    await new Promise((r) => setTimeout(r, 2000))
  }
  for (const f of files) {
    const want = mapped(await native.diagnosticsFor(`${nativeRoot}/${f.name}`))
    const got = wasm.diagnostics(`${WORK}/${f.name}`)
    assert.deepEqual(got, want, f.name)
  }
  assert.ok(wasm.diagnostics(`${WORK}/data.ttl`).some((d) => d.code === "shacl-violation"))
})

test("an edit is diagnosed again", async () => {
  const uri = `${WORK}/data.ttl`
  wasm.notify("textDocument/didChange", {
    textDocument: { uri, version: 2 },
    contentChanges: [{ text: textOf("data.ttl") + "\nid:x ex:name .\n" }],
  })
  await wasm.settle()
  assert.ok(wasm.diagnostics(uri).some((d) => d.severity === 1 && d.range.start.line === 10), JSON.stringify(wasm.diagnostics(uri)))
  wasm.notify("textDocument/didChange", { textDocument: { uri, version: 3 }, contentChanges: [{ text: textOf("data.ttl") }] })
  await wasm.settle()
  assert.ok(!wasm.diagnostics(uri).some((d) => d.range.start.line === 10))
})

// --- editor features ----------------------------------------------------------------------

test("hover", async () => {
  const h = await same("textDocument/hover", at("data.ttl", "ex:worksFor", 4))
  assert.match(JSON.stringify(h), /works for/)
  await same("textDocument/hover", at("people.rq", "ex:Person", 4))
})

test("completion and its resolve", async () => {
  const list = await same("textDocument/completion", at("data.ttl", "ex:name", 3))
  const items = list.items ?? list
  assert.ok(items.some((i) => i.label === "ex:worksFor"))
  await same("completionItem/resolve", items.find((i) => i.label === "ex:worksFor"))
  await same("textDocument/completion", at("people.rq", "?p a", 1))
})

test("definition, references and highlights, across files", async () => {
  const def = await same("textDocument/definition", at("data.ttl", "ex:worksFor", 4))
  assert.match(JSON.stringify(def), /ontology\.ttl/)
  await same("textDocument/references", { ...at("data.ttl", "ex:Person", 4), context: { includeDeclaration: true } })
  await same("textDocument/documentHighlight", at("people.rq", "?name", 2))
})

test("rename across files", async () => {
  await same("textDocument/prepareRename", at("ontology.ttl", "ex:worksFor", 4))
  const edit = await same("textDocument/rename", { ...at("ontology.ttl", "ex:worksFor", 4), newName: "ex:employedBy" })
  assert.ok(Object.keys(edit.changes ?? {}).length >= 2 || (edit.documentChanges ?? []).length >= 2, JSON.stringify(edit))
})

test("formatting", async () => {
  const opts = { tabSize: 4, insertSpaces: true }
  for (const name of ["data.ttl", "people.rq", "ontology.ttl"]) {
    await same("textDocument/formatting", { textDocument: { uri: `$ROOT/${name}` }, options: opts })
  }
})

test("code actions for the problems of a file", async () => {
  const uri = `${WORK}/ontology.ttl`
  const diagnostics = wasm.diagnostics(uri)
  for (const d of diagnostics) {
    const params = { textDocument: { uri: "$ROOT/ontology.ttl" }, range: d.range, context: { diagnostics: [d] } }
    const r = await both("textDocument/codeAction", JSON.parse(JSON.stringify(params).replaceAll(WORK, "$ROOT")))
    assert.deepEqual(r.wasm, r.native, d.message)
  }
})

test("outline, folding, semantic tokens and label hints", async () => {
  for (const name of ["ontology.ttl", "data.ttl", "people.rq"]) {
    const doc = { textDocument: { uri: `$ROOT/${name}` } }
    await same("textDocument/documentSymbol", doc)
    await same("textDocument/foldingRange", doc)
    await same("textDocument/semanticTokens/full", doc)
    const end = textOf(name).split("\n").length
    await same("textDocument/inlayHint", { ...doc, range: { start: { line: 0, character: 0 }, end: { line: end, character: 0 } } })
  }
})

test("type hierarchy", async () => {
  const items = await same("textDocument/prepareTypeHierarchy", at("ontology.ttl", "ex:Person a", 4))
  if (items?.length) await same("typeHierarchy/supertypes", { item: items[0] })
})

// --- the embedded driver --------------------------------------------------------------------

test("an unknown request is the native server's error, and the server keeps answering", async () => {
  const error = (p) => p.then(() => null, (e) => ({ code: e.code, message: e.message }))
  const [n, w] = await Promise.all([error(native.request("textDocument/teleport", {})), error(wasm.request("textDocument/teleport", {}))])
  assert.ok(w, "the embedded server answered an unknown request")
  assert.deepEqual(w, n)
  assert.ok(await wasm.request("textDocument/hover", { textDocument: { uri: `${WORK}/data.ttl` }, position: { line: 0, character: 2 } }) !== undefined)
})

test("shutdown is answered", async () => {
  const fresh = new EmbeddedServer(await createEngine(fs.readFileSync(path.join(here, "..", "app", "engine.wasm"))))
  await fresh.initialize({ processId: null, rootUri: null, capabilities: {} })
  assert.equal(await fresh.request("shutdown", null), null)
})
