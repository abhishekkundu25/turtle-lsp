// Refactorings: organise prefixes, shorten / expand IRIs, extract a prefix.
// Each test requests code actions, applies the returned edit and checks the text.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const ws = path.join(fixtures, "workspace")
const project = path.join(fixtures, "project")
let n = 0
const scratch = (dir = ws) => uriOf(path.join(dir, `__refactor${++n}.ttl`))

let s
let p
before(async () => {
  s = await startServer()
  p = await startServer({ cwd: project })
})
after(async () => {
  await s.stop()
  await p.stop()
})

// UTF-16 offset of an LSP position.
function offsetAt(text, pos) {
  let line = 0
  let i = 0
  while (line < pos.line && i < text.length) {
    if (text[i++] === "\n") line++
  }
  return i + pos.character
}

// Apply text edits (any order in the response) from the last one to the first.
function applyEdits(text, edits) {
  const sorted = edits
    .map((e) => ({ start: offsetAt(text, e.range.start), end: offsetAt(text, e.range.end), newText: e.newText }))
    .sort((a, b) => b.start - a.start || b.end - a.end)
  for (const e of sorted) text = text.slice(0, e.start) + e.newText + text.slice(e.end)
  return text
}

async function actionsAt(server, text, needle, { only, offsetInNeedle = 0, dir } = {}) {
  const uri = scratch(dir)
  server.open(uri, text)
  await server.diagnosticsFor(uri)
  const pos = needle ? positionOf(text, needle, offsetInNeedle) : { line: 0, character: 0 }
  const actions = await server.request("textDocument/codeAction", {
    textDocument: { uri },
    range: { start: pos, end: pos },
    context: { diagnostics: [], ...(only ? { only } : {}) },
  })
  return { uri, actions }
}

async function apply(text, needle, title, opts = {}) {
  const server = opts.server ?? s
  const { uri, actions } = await actionsAt(server, text, needle, opts)
  const action = actions.find((a) => a.title.startsWith(title))
  assert.ok(action, `no "${title}" in: ${actions.map((a) => a.title).join(", ")}`)
  return { action, text: applyEdits(text, action.edit.changes[uri]) }
}

const titles = (actions) => actions.map((a) => a.title)

// ---------- organise prefixes ----------

const messy = `# Header comment stays put.

PREFIX z: <http://z.example/>
# About ex.
@prefix ex: <http://ex.example/> .
@prefix unused: <http://unused.example/> .

@prefix : <http://default.example/> .
@prefix ex: <http://ex.example/> . # duplicate
@prefix b: <http://b.example/> . # trailing comment

ex:a z:p :c ; b:q "x" .
`

test("organise prefixes sorts, dedupes and drops unused, keeping comments and style", async () => {
  const { action, text } = await apply(messy, "ex:a", "Organise prefixes")
  assert.equal(action.kind, "source.organizeImports")
  assert.equal(
    text,
    `# Header comment stays put.

@prefix : <http://default.example/> .
@prefix b: <http://b.example/> . # trailing comment
# About ex.
@prefix ex: <http://ex.example/> .
PREFIX z: <http://z.example/>

ex:a z:p :c ; b:q "x" .
`
  )
})

test("organise is not offered when a prefix is redefined", async () => {
  const text = `@prefix b: <http://b/> .\n@prefix a: <http://a/> .\na:x b:y a:z .\n@prefix a: <http://other/> .\na:x b:y a:z .\n`
  const { actions } = await actionsAt(s, text, "a:x")
  assert.ok(!titles(actions).includes("Organise prefixes"), titles(actions).join(", "))
})

test("organise is not offered with a @base inside the block, or when nothing changes", async () => {
  const based = `@prefix b: <http://b/> .\n@base <http://base/> .\n@prefix a: <http://a/> .\na:x b:y <z> .\n`
  const sorted = `@prefix a: <http://a/> .\n\n@prefix b: <http://b/> .\na:x b:y a:z .\n`
  for (const text of [based, sorted]) {
    const { actions } = await actionsAt(s, text, "a:x")
    assert.ok(!titles(actions).includes("Organise prefixes"), titles(actions).join(", "))
  }
})

test("context.only filters by kind", async () => {
  const text = `@prefix b: <http://b/> .\n@prefix a: <http://a/> .\na:x b:y <http://a/z> .\n`
  const source = await actionsAt(s, text, "<http://a/z>", { only: ["source"], offsetInNeedle: 1 })
  assert.deepEqual(titles(source.actions), ["Organise prefixes"])
  const refactor = await actionsAt(s, text, "<http://a/z>", { only: ["refactor"], offsetInNeedle: 1 })
  assert.deepEqual(titles(refactor.actions), ["Shorten IRI to a:z", "Shorten all IRIs in file"])
  assert.ok(refactor.actions.every((a) => a.kind === "refactor.rewrite"))
})

// ---------- shorten / expand ----------

const iris = `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix ex: <http://ex.example/> .
ex:alice a<http://xmlns.com/foaf/0.1/Person> ; foaf:knows <http://ex.example/bob>, <http://ex.example/a/b> .
<http://ex.example/bob> foaf:name "Bob" .
`

test("shorten IRI rewrites the IRI at the cursor", async () => {
  const { action, text } = await apply(iris, "<http://ex.example/bob>,", "Shorten IRI", { offsetInNeedle: 3 })
  assert.equal(action.title, "Shorten IRI to ex:bob")
  assert.equal(action.kind, "refactor.rewrite")
  assert.equal(text, iris.replace("<http://ex.example/bob>,", "ex:bob,"))
})

test("shorten all IRIs keeps tokens apart and skips locals that need escapes", async () => {
  const { text } = await apply(iris, null, "Shorten all IRIs in file")
  assert.equal(
    text,
    `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix ex: <http://ex.example/> .
ex:alice a foaf:Person ; foaf:knows ex:bob, <http://ex.example/a/b> .
ex:bob foaf:name "Bob" .
`
  )
})

test("shorten all is resolved lazily through codeAction/resolve", async () => {
  const { uri, actions } = await actionsAt(s, iris, null)
  const all = actions.find((a) => a.title === "Shorten all IRIs in file")
  const { edit, ...bare } = all
  const resolved = await s.request("codeAction/resolve", bare)
  assert.deepEqual(resolved.edit.changes[uri], edit.changes[uri])
})

test("shorten is not offered without a covering prefix and expand not on 'a'", async () => {
  const { actions } = await actionsAt(s, iris, "ex:alice a<", { offsetInNeedle: 9 })
  assert.ok(!titles(actions).includes("Expand to full IRI"), titles(actions).join(", "))
  const plain = "<http://nowhere.example/x> <http://nowhere.example/y> 1 .\n"
  const none = await actionsAt(s, plain, "<http://nowhere", { offsetInNeedle: 2 })
  assert.ok(!titles(none.actions).some((t) => t.startsWith("Shorten")), titles(none.actions).join(", "))
})

test("shorten uses the project's prefixes.ttl", async () => {
  const text = `<urn:x:a> <http://www.w3.org/2000/01/rdf-schema#label> "A" .\n`
  const { text: out } = await apply(text, "rdf-schema", "Shorten IRI", { server: p, dir: path.join(project, "data") })
  assert.equal(out, `<urn:x:a> rdfs:label "A" .\n`)
})

test("expand turns a prefixed name into its full IRI", async () => {
  const { action, text } = await apply(iris, "foaf:knows", "Expand to full IRI", { offsetInNeedle: 2 })
  assert.equal(action.kind, "refactor.rewrite")
  assert.equal(text, iris.replace("foaf:knows", "<http://xmlns.com/foaf/0.1/knows>"))
})

test("positions are UTF-16 code units", async () => {
  const text = `@prefix ex: <http://ex.example/> .\nex:s ex:p "😀é", <http://ex.example/o> .\n`
  const { text: out } = await apply(text, "<http://ex.example/o>", "Shorten IRI", { offsetInNeedle: 5 })
  assert.equal(out, `@prefix ex: <http://ex.example/> .\nex:s ex:p "😀é", ex:o .\n`)
})

// ---------- extract prefix ----------

test("extract prefix uses the well-known name and rewrites the namespace", async () => {
  const text = `@prefix ex: <http://ex.example/> .\nex:a a <http://xmlns.com/foaf/0.1/Person> ; <http://xmlns.com/foaf/0.1/name> "A" .\n`
  const { action, text: out } = await apply(text, "<http://xmlns.com/foaf/0.1/Person>", "Extract prefix", { offsetInNeedle: 3 })
  assert.equal(action.title, "Extract prefix foaf: for <http://xmlns.com/foaf/0.1/>")
  assert.equal(action.kind, "refactor.rewrite")
  assert.equal(
    out,
    `@prefix ex: <http://ex.example/> .\n@prefix foaf: <http://xmlns.com/foaf/0.1/> .\nex:a a foaf:Person ; foaf:name "A" .\n`
  )
})

test("extract prefix derives a unique name from the path", async () => {
  const text = `PREFIX core: <http://other.example/>\ncore:a <http://acme.example/ont/core/0.2/hasPart> <http://acme.example/ont/core/0.2/b>, <http://acme.example/ont/core/0.2/x/y> .\n`
  const { action, text: out } = await apply(text, "hasPart", "Extract prefix")
  assert.equal(action.title, "Extract prefix core1: for <http://acme.example/ont/core/0.2/>")
  assert.equal(
    out,
    `PREFIX core: <http://other.example/>\nPREFIX core1: <http://acme.example/ont/core/0.2/>\ncore:a core1:hasPart core1:b, <http://acme.example/ont/core/0.2/x/y> .\n`
  )
})

test("extract prefix is not offered when a prefix already covers the IRI", async () => {
  const { actions } = await actionsAt(s, iris, "<http://ex.example/bob>,", { offsetInNeedle: 3 })
  assert.ok(!titles(actions).some((t) => t.startsWith("Extract")), titles(actions).join(", "))
})
