// completionItem/resolve in Turtle and SPARQL (the list carries no
// documentation; resolving an item adds it), and textDocument/prepareRename
// in Turtle: what can be renamed answers with its range, a keyword or a
// literal is refused (null).
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf, positionOf } from "./lsp-client.mjs"

const FOAF = "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n@prefix ex: <http://example.org/> .\n"

let s, dir
before(async () => {
  dir = tempDir("turtle-resolve-rename-")
  s = await startServer({ cwd: dir })
})
after(async () => {
  try {
    await s?.stop()
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

let n = 0
async function open(text, ext = "ttl", languageId = ext === "rq" ? "sparql" : "turtle") {
  const uri = uriOf(path.join(dir, `doc-${n++}.${ext}`))
  s.open(uri, text, languageId)
  await s.diagnosticsFor(uri)
  return uri
}

const itemsOf = (res) => (Array.isArray(res) ? res : (res?.items ?? []))

async function completionItem(text, needle, label, ext) {
  const uri = await open(text, ext)
  const res = await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, needle) })
  const item = itemsOf(res).find((i) => i.label === label)
  assert.ok(item, `${label} not offered: ${itemsOf(res).slice(0, 10).map((i) => i.label).join(", ")}`)
  return item
}

const docText = (item) => (typeof item.documentation === "string" ? item.documentation : item.documentation?.value)

test("resolving a Turtle completion item adds its documentation and keeps its detail", async () => {
  const text = `${FOAF}ex:alice a foaf:Per\n`
  const item = await completionItem(text, "foaf:Per", "foaf:Person")
  // The list stays small: no documentation until the item is resolved.
  assert.equal(item.documentation, undefined)
  assert.ok(item.data?.iri, JSON.stringify(item))
  const resolved = await s.request("completionItem/resolve", item)
  assert.equal(resolved.label, "foaf:Person")
  assert.equal(resolved.detail, item.detail)
  assert.equal(resolved.documentation?.kind, "markdown")
  assert.match(docText(resolved), /Person/)
  assert.match(docText(resolved), /http:\/\/xmlns\.com\/foaf\/0\.1\/Person/)
})

test("resolving a SPARQL completion item adds its documentation", async () => {
  const text = "PREFIX foaf: <http://xmlns.com/foaf/0.1/>\nSELECT * WHERE { ?s a foaf:Per }\n"
  const item = await completionItem(text, "foaf:Per", "foaf:Person", "rq")
  // Term items carry the IRI to look up, as in Turtle.
  assert.equal(item.data?.iri, "http://xmlns.com/foaf/0.1/Person", JSON.stringify(item))
  assert.equal(item.documentation, undefined)
  const resolved = await s.request("completionItem/resolve", item)
  assert.equal(resolved.detail, item.detail)
  assert.match(docText(resolved), /Person/)
})

test("resolving an item with nothing to look up returns it unchanged", async () => {
  const item = { label: "a", kind: 14 }
  assert.deepEqual(await s.request("completionItem/resolve", item), item)
})

test("prepareRename in Turtle: a prefixed name and a full IRI give their range; a keyword and a literal are refused", async () => {
  const text = `${FOAF}ex:alice a foaf:Person ;\n  foaf:knows <http://example.org/bob> ;\n  foaf:name "Alice" .\n`
  const uri = await open(text)
  const prepare = (needle, offset = 1) =>
    s.request("textDocument/prepareRename", { textDocument: { uri }, position: positionOf(text, needle, offset) })
  const at = (line, from, to) => ({ start: { line, character: from }, end: { line, character: to } })
  // On the local name of a prefixed name: the whole name (on its prefix
  // part, the prefix is renamed instead).
  assert.deepEqual(await prepare("ex:alice", 4), at(2, 0, 8))
  // A full IRI, brackets included.
  assert.deepEqual(await prepare("<http://example.org/bob>", 5), at(3, 13, 37))
  // The keyword `a` and a literal are refused.
  assert.equal(await prepare(" a foaf:Person", 1), null)
  assert.equal(await prepare('"Alice"', 2), null)
})
