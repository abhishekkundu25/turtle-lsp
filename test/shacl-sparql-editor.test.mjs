// SPARQL in SHACL shapes: an sh:select literal gets the SPARQL colours and
// completion (keywords, the query's variables, the prefixes sh:prefixes
// declares), at positions in the Turtle text.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir, positionOf } from "./lsp-client.mjs"

const dir = tempDir("shacl-sparql-editor-")
// The cursor goes on the indented empty line of the query.
const blank = " ".repeat(16)
const shapes = `@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.com/ns#> .
ex:P sh:declare [ sh:prefix "my" ; sh:namespace "http://example.com/my#" ] .
ex:S a sh:NodeShape ;
    sh:sparql [
        sh:prefixes ex:P ;
        sh:select """
            SELECT $this WHERE {
                FILTER NOT EXISTS { $this my:next ?n }
${blank}
            }""" ] .
`
const uri = uriOf(path.join(dir, "shapes.ttl"))

let s
before(async () => {
  s = await startServer({ cwd: dir })
  s.open(uri, shapes)
  await s.diagnosticsFor(uri)
})
after(async () => {
  await s?.stop()
  fs.rmSync(dir, { recursive: true, force: true })
})

const complete = (needle, triggerCharacter) =>
  s.request("textDocument/completion", {
    textDocument: { uri },
    position: positionOf(shapes, needle),
    context: triggerCharacter ? { triggerKind: 2, triggerCharacter } : { triggerKind: 1 },
  })
const labels = (res) => (Array.isArray(res) ? res : res?.items ?? []).map((i) => i.label)

test("the query gets SPARQL tokens at their place in the Turtle text", async () => {
  const legend = s.init.capabilities.semanticTokensProvider.legend
  const res = await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
  const lines = shapes.split("\n")
  const tokens = []
  let line = 0
  let character = 0
  for (let i = 0; i < res.data.length; i += 5) {
    const [dl, ds, len, type] = res.data.slice(i, i + 4)
    line += dl
    character = dl ? ds : character + ds
    tokens.push([lines[line].slice(character, character + len), legend.tokenTypes[type], line])
  }
  const filter = positionOf(shapes, "FILTER", 0).line
  assert.deepEqual(
    tokens.filter((t) => t[2] === filter).map((t) => t.slice(0, 2)),
    [
      ["FILTER", "keyword"],
      ["NOT", "keyword"],
      ["EXISTS", "keyword"],
      ["$this", "variable"],
      ["my", "namespace"],
      ["next", "property"],
      ["?n", "variable"],
    ]
  )
  // The Turtle around the query keeps its tokens.
  assert.ok(tokens.some((t) => t[0] === "select" && t[1] === "property"))
})

test("completion in the query offers SPARQL keywords and sh:declare prefixes", async () => {
  const res = await complete(`?n }\n${blank}`)
  const l = labels(res)
  for (const w of ["OPTIONAL", "FILTER", "my:"]) assert.ok(l.includes(w), `${w} not in ${l.slice(0, 20)}`)
  assert.ok(!l.includes("@prefix"), l.slice(0, 20).join(", "))
  const item = res.items.find((i) => i.label === "OPTIONAL")
  const at = positionOf(shapes, `?n }\n${blank}`)
  assert.deepEqual(item.textEdit.range, { start: at, end: at })
})

test("'?' in the query offers its variables", async () => {
  const text = shapes.replace(`?n }\n${blank}\n`, `?n }\n${blank}FILTER (?\n`)
  s.change(uri, text)
  await s.diagnosticsFor(uri)
  const res = await s.request("textDocument/completion", {
    textDocument: { uri },
    position: positionOf(text, "FILTER (?"),
    context: { triggerKind: 2, triggerCharacter: "?" },
  })
  const l = labels(res)
  assert.ok(l.includes("?n") && l.includes("?this"), l.join(", "))
  s.change(uri, shapes)
  await s.diagnosticsFor(uri)
})

test("outside the query completion stays Turtle", async () => {
  const l = labels(await complete("ex:S a sh:NodeShape ;\n    "))
  assert.ok(!l.includes("OPTIONAL"), l.slice(0, 20).join(", "))
  assert.ok(l.includes("sh:"), l.slice(0, 20).join(", "))
})
