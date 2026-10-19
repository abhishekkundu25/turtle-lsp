// N-Quads documents (test/fixtures/query): data.nq holds quads in named
// graphs, the default graph and a blank-node graph; ontology.ttl describes
// the terms they use; uses-nickname.ttl uses a term only data.nq describes.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const dir = path.join(fixtures, "query")
const read = (f) => fs.readFileSync(path.join(dir, f), "utf8")
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.code} ${d.message}`).join("\n")
const problems = (diags) => diags.filter((d) => d.severity <= 2)
const errorLines = (diags) => [...new Set(diags.filter((d) => d.severity === 1).map((d) => d.range.start.line))].sort((a, b) => a - b)
const where = (locs) => locs.map((l) => `${path.basename(l.uri)}:${l.range.start.line}`).sort()

let s
before(async () => {
  s = await startServer({ cwd: dir })
  // Requests wait for the workspace index; diagnostics after this include
  // the workspace lints.
  await s.request("workspace/symbol", { query: "" })
})
after(async () => {
  await s.stop()
})

async function openAs(uri, text, languageId) {
  s.open(uri, text, languageId)
  return { uri, text, diags: await s.diagnosticsFor(uri) }
}
const close = (uri) => s.close(uri)
const scratch = (name) => uriOf(path.join(dir, name))
const openData = () => openAs(uriOf(path.join(dir, "data.nq")), read("data.nq"), "nquads")

test("data.nq has no problems", async () => {
  const { uri, diags } = await openData()
  close(uri)
  assert.deepEqual(problems(diags), [], fmt(problems(diags)))
})

for (const languageId of ["nquads", "n-quads", "turtle", "plaintext"]) {
  test(`a .nq file opened as '${languageId}' is read as N-Quads`, async () => {
    const uri = scratch(`__detect-${languageId}.nq`)
    const { diags } = await openAs(uri, "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> .\n", languageId)
    close(uri)
    assert.deepEqual(problems(diags), [], fmt(diags))
  })
}

test("the 'nquads' language id wins over the file name", async () => {
  const uri = scratch("__named-otherwise.txt")
  const { diags } = await openAs(uri, "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> .\n", "nquads")
  close(uri)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("a quad is an error in N-Triples, so the syntaxes are told apart", async () => {
  const uri = scratch("__quad.nt")
  const { diags } = await openAs(uri, "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> .\n", "ntriples")
  close(uri)
  assert.deepEqual(errorLines(diags), [0], fmt(diags))
})

test("bad lines are reported one by one and the good lines between them are not", async () => {
  const text = [
    '<http://e.org/s> <http://e.org/p> <http://e.org/o> "graph" .',
    "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> <http://e.org/h> .",
    "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> .",
    '<http://e.org/s> <http://e.org/p> "unterminated <http://e.org/g> .',
    "<http://e.org/s> <http://e.org/p> <http://e.org/o> <relative> .",
    "<http://e.org/s> <http://e.org/p> <http://e.org/o> ex:g .",
    "_:s <http://e.org/p> _:o _:g .",
    '<http://e.org/s> <http://e.org/p> "x"@en-- <http://e.org/g> .',
    '<http://e.org/s> <http://e.org/p> "x"@en <http://e.org/g>',
    "<http://e.org/s> <http://e.org/p> <<( <http://e.org/a> <http://e.org/b> <http://e.org/c> )>> <http://e.org/g> .",
    "",
  ].join("\n")
  const uri = scratch("__bad-lines.nq")
  const { diags } = await openAs(uri, text, "nquads")
  close(uri)
  assert.deepEqual(errorLines(diags), [0, 1, 3, 4, 5, 7, 8], fmt(diags))
})

test("the error on a literal graph label is on the label", async () => {
  const text = '<http://e.org/s> <http://e.org/p> <http://e.org/o> "graph" .\n'
  const uri = scratch("__literal-graph.nq")
  const { diags } = await openAs(uri, text, "nquads")
  close(uri)
  const e = diags.find((d) => d.severity === 1)
  assert.ok(e, fmt(diags))
  assert.equal(e.range.start.character, text.indexOf('"graph"'), fmt(diags))
})

test("N-Quads gets no prefix help: no undeclared-prefix, no completions, no add-prefix fix", async () => {
  const text = '<http://e.org/s> foaf:name "A" <http://e.org/g> .\n'
  const uri = scratch("__nq-prefix.nq")
  const { diags } = await openAs(uri, text, "nquads")
  try {
    assert.ok(diags.some((d) => d.severity === 1), fmt(diags))
    assert.equal(diags.filter((d) => d.code === "undeclared-prefix").length, 0, fmt(diags))
    const res = await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, "foaf:") })
    const items = Array.isArray(res) ? res : (res?.items ?? [])
    assert.equal(items.filter((i) => i.label.startsWith("foaf:")).length, 0)
    const actions = await s.request("textDocument/codeAction", {
      textDocument: { uri },
      range: { start: positionOf(text, "foaf:", 0), end: positionOf(text, "foaf:") },
      context: { diagnostics: diags },
    })
    assert.equal((actions ?? []).filter((a) => /prefix/i.test(a.title)).length, 0, JSON.stringify(actions))
  } finally {
    close(uri)
  }
})

test("outline groups statements under their graph", async () => {
  const { uri } = await openData()
  try {
    const syms = await s.request("textDocument/documentSymbol", { textDocument: { uri } })
    // One container per graph, wherever its quads are; one child per subject.
    const graphs = syms.filter((x) => x.children).map((x) => `${x.name}[${x.children.length}]`)
    assert.equal(new Set(graphs).size, graphs.length, graphs.join(" "))
    for (const g of ["<https://query.example.org/graph/people>[1]", "<https://query.example.org/graph/terms>[1]", "_:g1[1]"]) {
      assert.ok(graphs.includes(g), `${g} not in ${graphs.join(" ")}`)
    }
    const people = syms.find((x) => x.name === "<https://query.example.org/graph/people>")
    assert.equal(people.kind, 3) // Namespace
    assert.equal(people.children[0].name, "<https://query.example.org/id/alice>")
    // The default graph's statement is not inside any graph.
    assert.ok(syms.some((x) => !x.children && x.name === "<https://query.example.org/id/Q42>"), JSON.stringify(syms.map((x) => x.name)))
  } finally {
    close(uri)
  }
})

test("hover on an IRI in a .nq file shows its label and comment from the ontology", async () => {
  const { uri, text } = await openData()
  try {
    const h = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, "def/Person>", 4) })
    assert.match(h.contents.value, /A human being the dataset describes/)
  } finally {
    close(uri)
  }
})

test("definition from a .nq file lands on the class in the ontology", async () => {
  const { uri, text } = await openData()
  try {
    const locs = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(text, "def/Person>", 4) })
    assert.deepEqual(where(locs), ["ontology.ttl:6"])
  } finally {
    close(uri)
  }
})

test("definition from a Turtle file finds the subjects in a .nq file", async () => {
  const uri = uriOf(path.join(dir, "uses-nickname.ttl"))
  const text = read("uses-nickname.ttl")
  await openAs(uri, text, "turtle")
  try {
    const locs = await s.request("textDocument/definition", { textDocument: { uri }, position: positionOf(text, "id/alice>", 3) })
    for (const want of ["data.nq:1", "data.nq:2", "data.nq:3"]) assert.ok(where(locs).includes(want), where(locs).join(" "))
  } finally {
    close(uri)
  }
})

test("references of a subject cover every quad, highlights the ones in this file", async () => {
  const { uri, text } = await openData()
  try {
    const position = positionOf(text, "id/alice>", 3)
    const refs = await s.request("textDocument/references", { textDocument: { uri }, position, context: { includeDeclaration: true } })
    assert.deepEqual(where(refs), ["data.nq:1", "data.nq:2", "data.nq:3", "uses-nickname.ttl:2"])
    const hl = await s.request("textDocument/documentHighlight", { textDocument: { uri }, position })
    assert.deepEqual(hl.map((h) => h.range.start.line).sort(), [1, 2, 3])
  } finally {
    close(uri)
  }
})

test("a term described only in a .nq file counts as described in Turtle", async () => {
  const uri = uriOf(path.join(dir, "uses-nickname.ttl"))
  const { text, diags } = await openAs(uri, read("uses-nickname.ttl"), "turtle")
  close(uri)
  const undescribed = diags.filter((d) => d.code === "undescribed-term")
  // ex:nickname is described in data.nq; ex:nicknam is a typo of it.
  assert.deepEqual(
    undescribed.map((d) => d.range.start.line),
    [positionOf(text, "ex:nicknam ").line],
    fmt(diags)
  )
  assert.match(undescribed[0].message, /nickname/)
})

test("workspace symbols include subjects of .nq files", async () => {
  const q42 = await s.request("workspace/symbol", { query: "Douglas" })
  assert.ok(q42.some((x) => x.location.uri.endsWith("/data.nq")), JSON.stringify(q42))
})

test("label inlay hints show on opaque IRIs in a .nq file", async () => {
  const { uri, text } = await openData()
  try {
    const lines = text.split("\n").length
    const hints = await s.request("textDocument/inlayHint", {
      textDocument: { uri },
      range: { start: { line: 0, character: 0 }, end: { line: lines, character: 0 } },
    })
    const labels = (hints ?? []).map((h) => (typeof h.label === "string" ? h.label : h.label.map((p) => p.value).join("")))
    assert.ok(labels.some((l) => l.includes("Douglas Adams")), labels.join(" | "))
  } finally {
    close(uri)
  }
})

test("semantic tokens cover the IRIs of a .nq file", async () => {
  const { uri } = await openData()
  try {
    const t = await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
    assert.ok(t.data.length >= 5 * 10, `only ${t.data.length / 5} tokens`)
  } finally {
    close(uri)
  }
})

test("an N-Quads file with an error is not formatted", async () => {
  const uri = scratch("__broken-format.nq")
  await openAs(uri, '<http://e.org/s>   <http://e.org/p> "x" "g" .\n', "nquads")
  try {
    assert.deepEqual(await s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } }), [])
  } finally {
    close(uri)
  }
})

test("editing a .nq file updates its diagnostics", async () => {
  const uri = scratch("__edit.nq")
  await openAs(uri, "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> .\n", "nquads")
  try {
    s.conn.sendNotification("textDocument/didChange", {
      textDocument: { uri, version: 2 },
      contentChanges: [{ text: "<http://e.org/s> <http://e.org/p> <http://e.org/o> <http://e.org/g> <http://e.org/h> .\n" }],
    })
    let diags = []
    for (let i = 0; i < 40 && !diags.some((d) => d.severity === 1); i++) {
      await new Promise((r) => setTimeout(r, 50))
      diags = await s.diagnosticsFor(uri)
    }
    assert.deepEqual(errorLines(diags), [0], fmt(diags))
  } finally {
    close(uri)
  }
})
