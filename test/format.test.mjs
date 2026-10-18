// textDocument/formatting over stdio: the golden fixtures (Turtle, TriG,
// N-Triples, N-Quads), refusal on syntax errors, no edits when already formatted, and
// the client's indent options.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures } from "./lsp-client.mjs"

const dir = path.join(fixtures, "format")
const read = (f) => fs.readFileSync(path.join(dir, f), "utf8")
const spaces = { tabSize: 4, insertSpaces: true }

// Apply LSP edits (UTF-16 positions) to a text.
function apply(text, edits) {
  const lines = text.split("\n")
  const offset = ({ line, character }) => {
    let o = 0
    for (let i = 0; i < line && i < lines.length; i++) o += lines[i].length + 1
    return Math.min(o + character, text.length)
  }
  const sorted = [...edits].sort((a, b) => offset(b.range.start) - offset(a.range.start))
  for (const e of sorted) text = text.slice(0, offset(e.range.start)) + e.newText + text.slice(offset(e.range.end))
  return text
}

let s
before(async () => {
  s = await startServer({ cwd: dir })
})
after(async () => {
  await s.stop()
})

let n = 0
async function format(text, options = spaces, ext = "ttl") {
  const uri = uriOf(path.join(dir, `doc${n++}.${ext}`))
  const languageId = { ttl: "turtle", trig: "trig", nt: "ntriples", nq: "nquads" }[ext]
  s.conn.sendNotification("textDocument/didOpen", { textDocument: { uri, languageId, version: 1, text } })
  await s.diagnosticsFor(uri)
  return s.request("textDocument/formatting", { textDocument: { uri }, options })
}

test("advertises document formatting", () => {
  assert.equal(s.init.capabilities.documentFormattingProvider, true)
})

test("formats the messy fixture into the golden file", async () => {
  const text = read("messy.ttl")
  const edits = await format(text)
  assert.equal(edits.length, 1)
  assert.equal(apply(text, edits), read("expected.ttl"))
})

for (const ext of ["trig", "nt", "nq"]) {
  test(`formats the messy ${ext} fixture into the golden file`, async () => {
    const text = read(`messy.${ext}`)
    const edits = await format(text, spaces, ext)
    assert.equal(edits.length, 1)
    assert.equal(apply(text, edits), read(`expected.${ext}`))
    assert.deepEqual(await format(read(`expected.${ext}`), spaces, ext), [])
  })
}

test("an already formatted document gets no edits", async () => {
  assert.deepEqual(await format(read("expected.ttl")), [])
})

test("a document with a syntax error gets no edits", async () => {
  assert.deepEqual(await format(read("broken.ttl")), [])
})

test("uses tabs when the client asks for them", async () => {
  const text = "@prefix ex: <https://example.com/ns#> .\nex:a ex:p ex:b ; ex:q [ ex:r ex:s ; ex:t ( ex:u [ ex:v ex:w ] ) ] .\n"
  const out = apply(text, await format(text, { tabSize: 8, insertSpaces: false }))
  assert.equal(
    out,
    "@prefix ex: <https://example.com/ns#> .\n\nex:a ex:p ex:b ;\n\tex:q [\n\t\tex:r ex:s ;\n\t\tex:t (\n\t\t\tex:u\n\t\t\t[ ex:v ex:w ]\n\t\t)\n\t] .\n"
  )
})

test("the edit covers the whole document with multi-byte text (UTF-16 positions)", async () => {
  const text = "@prefix ex: <https://example.com/ns#> .\nex:a ex:label \"😀 smile\"@en ;   ex:b \"日本\" .   "
  const out = apply(text, await format(text))
  assert.equal(out, "@prefix ex: <https://example.com/ns#> .\n\nex:a ex:label \"😀 smile\"@en ;\n    ex:b \"日本\" .\n")
})
