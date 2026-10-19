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

// --- prefixes.ttl ------------------------------------------------------------------------

test("prefixes.ttl supplies prefixes as natively, and editing it re-diagnoses", async () => {
  const project = {
    "prefixes.ttl": "@prefix ex: <https://example.org/def/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n",
    // The nearest prefixes.ttl alone applies (as natively): sub/ repeats ex:.
    "sub/prefixes.ttl": "@prefix ex: <https://example.org/def/> .\n@prefix sx: <https://example.org/sub/> .\n",
    "a.ttl": 'ex:a rdfs:label "A" .\n',
    "sub/b.ttl": "ex:b ex:p sx:c .\n",
  }
  const pdir = tempDir("turtle-web-prefixes-")
  for (const [name, text] of Object.entries(project)) {
    fs.mkdirSync(path.dirname(path.join(pdir, name)), { recursive: true })
    fs.writeFileSync(path.join(pdir, name), text)
  }
  const root = uriOf(pdir)
  const n = await startServer({ cwd: pdir })
  const w = new EmbeddedServer(await createEngine(fs.readFileSync(path.join(here, "..", "app", "engine.wasm"))))
  try {
    await w.initialize({ processId: null, rootUri: null, capabilities: {} })
    for (const [name, text] of Object.entries(project)) {
      n.open(`${root}/${name}`, text)
      w.notify("textDocument/didOpen", { textDocument: { uri: `${WORK}/${name}`, languageId: "turtle", version: 1, text } })
    }
    await w.settle()
    // Native diagnostics for `name` once `ok` holds (they may be republished).
    const nativeUntil = async (name, ok) => {
      const end = Date.now() + 10_000
      let d = await n.diagnosticsFor(`${root}/${name}`)
      while (!ok(d) && Date.now() < end) {
        await new Promise((r) => setTimeout(r, 50))
        d = await n.diagnosticsFor(`${root}/${name}`)
      }
      return JSON.parse(JSON.stringify(d).replaceAll(root, WORK))
    }
    const undeclared = (d) => d.filter((x) => x.code === "undeclared-prefix")
    for (const name of Object.keys(project)) {
      const got = w.diagnostics(`${WORK}/${name}`)
      assert.deepEqual(got, await nativeUntil(name, (d) => undeclared(d).length === 0), name)
      assert.deepEqual(undeclared(got), [], name)
    }

    // Remove ex: from the root prefixes.ttl: a.ttl now uses an undeclared
    // prefix; sub/b.ttl, under sub/prefixes.ttl, does not.
    const edited = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
    n.change(`${root}/prefixes.ttl`, edited)
    w.notify("textDocument/didChange", { textDocument: { uri: `${WORK}/prefixes.ttl`, version: 2 }, contentChanges: [{ text: edited }] })
    await w.settle()
    const a = w.diagnostics(`${WORK}/a.ttl`)
    assert.ok(undeclared(a).some((d) => d.message.includes("'ex:'")), JSON.stringify(a))
    assert.deepEqual(a, await nativeUntil("a.ttl", (d) => undeclared(d).length > 0))
    assert.deepEqual(undeclared(w.diagnostics(`${WORK}/sub/b.ttl`)), [])
    assert.deepEqual(w.diagnostics(`${WORK}/sub/b.ttl`), await nativeUntil("sub/b.ttl", () => true))

    // In the browser a closed prefixes.ttl is gone: sub/b.ttl falls back to
    // the root one, which no longer declares ex:, and sx: was only in sub/.
    w.notify("textDocument/didClose", { textDocument: { uri: `${WORK}/sub/prefixes.ttl` } })
    await w.settle()
    const b = undeclared(w.diagnostics(`${WORK}/sub/b.ttl`)).map((d) => d.message.match(/'(\w*):'/)?.[1])
    assert.deepEqual([...new Set(b)].sort(), ["ex", "sx"])
  } finally {
    await n.stop()
    fs.rmSync(pdir, { recursive: true, force: true })
  }
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
