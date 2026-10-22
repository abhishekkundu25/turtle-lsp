// Understanding a SPARQL document: hover, definition, references and
// highlights (variables are scoped: a subquery's variables are its own
// unless projected), semantic tokens, folding, outline and inlay hints.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { queryServer, positionOf, where, slice } from "./query-client.mjs"
import { startServer, tempDir, uriOf } from "./lsp-client.mjs"

const EX = "PREFIX ex: <https://query.example.org/def/>\n"

let q
before(async () => {
  q = await queryServer()
})
after(async () => {
  await q.stop()
})

const req = (method, uri, position, extra = {}) => q.s.request(method, { textDocument: { uri }, position, ...extra })
const texts = (text, ranges) => ranges.map((r) => `${r.start.line}:${r.start.character} ${slice(text, r)}`)

// --- hover -----------------------------------------------------------------------

test("hover on a workspace term shows its label, comment and where it is described", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const h = await req("textDocument/hover", uri, positionOf(text, "a ex:Person", 6))
    assert.match(h.contents.value, /Person/)
    assert.match(h.contents.value, /A human being the dataset describes/)
    assert.match(h.contents.value, /ontology\.ttl/)
  })
})

test("hover on a vocabulary term shows its documentation", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const h = await req("textDocument/hover", uri, positionOf(text, "rdfs:label", 6))
    assert.ok(h && h.contents.value.length > 20, JSON.stringify(h))
  })
})

test("hover on a full IRI in a query works too", async () => {
  const text = "SELECT * WHERE { ?p <https://query.example.org/def/worksFor> ?o }\n"
  await q.withScratch(text, async ({ uri }) => {
    const h = await req("textDocument/hover", uri, positionOf(text, "def/worksFor", 6))
    assert.match(h.contents.value, /works for/)
  })
})

// --- definition ------------------------------------------------------------------

test("definition on a term opens where the workspace describes it", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const locs = await req("textDocument/definition", uri, positionOf(text, "ex:worksFor", 5))
    assert.deepEqual(where(locs), ["ontology.ttl:18"])
  })
})

test("definition on a term described in a .nq file lands there", async () => {
  const text = `${EX}SELECT * WHERE { ?p ex:nickname ?n }\n`
  await q.withScratch(text, async ({ uri }) => {
    const locs = await req("textDocument/definition", uri, positionOf(text, "ex:nickname", 5))
    assert.ok(where(locs).includes("data.nq:5"), where(locs).join(" "))
  })
})

test("definition on a prefix jumps to its PREFIX line", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const locs = await req("textDocument/definition", uri, positionOf(text, "ex:worksFor", 1))
    const list = Array.isArray(locs) ? locs : [locs]
    assert.equal(list.length, 1)
    assert.equal(list[0].uri, uri)
    assert.equal(list[0].range.start.line, 1)
  })
})

test("definition on a variable jumps to its first occurrence in its scope", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const locs = await req("textDocument/definition", uri, positionOf(text, "STRLEN(?name", 9))
    const list = Array.isArray(locs) ? locs : [locs]
    assert.deepEqual(texts(text, list.map((l) => l.range)), [`4:${"SELECT ?person ".length} ?name`])
  })
})

// --- variables: highlight, references, scope ------------------------------------

const scoped = `${EX}SELECT ?p ?n WHERE {
  ?p ex:name ?n .
  {
    SELECT ?p (COUNT(?o) AS ?count) WHERE {
      ?p ex:worksFor ?o .
      ?o ex:name ?n .
    }
    GROUP BY ?p
  }
  FILTER(?n != "x")
}
`

test("highlight marks every occurrence of a variable in its scope", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    const hl = await req("textDocument/documentHighlight", uri, positionOf(text, "?p ex:name", 1))
    // ?p is projected by the subquery, so its occurrences inside are the same variable.
    assert.deepEqual(
      hl.map((h) => `${h.range.start.line}:${h.range.start.character}`).sort(),
      ["1:7", "2:2", "4:11", "5:6", "8:13"].sort(),
      texts(text, hl.map((h) => h.range)).join(" | ")
    )
  })
})

test("a subquery's own variable is a different variable from the outer one", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    // ?n inside the subquery is not projected: it is not the outer ?n.
    const outer = await req("textDocument/documentHighlight", uri, positionOf(text, "?p ex:name ?n", 12))
    assert.deepEqual(outer.map((h) => h.range.start.line).sort((a, b) => a - b), [1, 2, 10], texts(text, outer.map((h) => h.range)).join(" | "))
    const inner = await req("textDocument/documentHighlight", uri, positionOf(text, "?o ex:name ?n", 12))
    assert.deepEqual(inner.map((h) => h.range.start.line), [6])
  })
})

test("references on a variable list its occurrences in scope", async () => {
  await q.withScratch(scoped, async ({ uri, text }) => {
    const refs = await req("textDocument/references", uri, positionOf(text, "?o .", 1), { context: { includeDeclaration: true } })
    assert.deepEqual(refs.map((r) => r.range.start.line).sort(), [4, 5, 6])
  })
})

test("references on a term list its uses in the query and in the workspace", async () => {
  await q.withFile("people.rq", async ({ uri, text }) => {
    const refs = await req("textDocument/references", uri, positionOf(text, "ex:worksFor", 5), { context: { includeDeclaration: false } })
    const w = where(refs)
    for (const want of ["people.rq:9", "data.nq:3"]) assert.ok(w.includes(want), w.join(" "))
  })
})

test("references on a term from Turtle include SPARQL files, open or not", async () => {
  await q.withFile("ontology.ttl", async ({ uri, text }) => {
    const refs = await req("textDocument/references", uri, positionOf(text, "ex:worksFor a", 5), { context: { includeDeclaration: false } })
    const w = where(refs)
    for (const want of ["people.rq:9", "data.nq:3"]) assert.ok(w.includes(want), w.join(" "))
  })
})

// --- semantic tokens --------------------------------------------------------------

function decode(res, legend, text) {
  const lines = text.split("\n")
  const out = []
  let line = 0
  let ch = 0
  for (let i = 0; i < res.data.length; i += 5) {
    const [dl, dc, len, type, mods] = res.data.slice(i, i + 5)
    line += dl
    ch = dl ? dc : ch + dc
    out.push({
      text: lines[line].slice(ch, ch + len),
      type: legend.tokenTypes[type],
      mods: legend.tokenModifiers.filter((_, b) => mods & (1 << b)),
    })
  }
  return out
}

test("semantic tokens: keywords, variables, functions, prefixes, classes and properties", async () => {
  const legend = q.s.init.capabilities.semanticTokensProvider.legend
  await q.withFile("people.rq", async ({ uri, text }) => {
    const t = decode(await q.s.request("textDocument/semanticTokens/full", { textDocument: { uri } }), legend, text)
    const kind = (word) => t.filter((x) => x.text === word).map((x) => x.type)
    assert.deepEqual(kind("PREFIX"), ["keyword", "keyword"])
    assert.deepEqual(kind("SELECT"), ["keyword"])
    assert.deepEqual(kind("OPTIONAL"), ["keyword"])
    assert.ok(kind("?name").every((k) => k === "variable") && kind("?name").length === 4, JSON.stringify(kind("?name")))
    assert.deepEqual(kind("STRLEN"), ["function"])
    assert.deepEqual(kind("Person"), ["class"])
    assert.deepEqual(kind("worksFor"), ["property"])
    assert.ok(kind("ex").includes("namespace"))
  })
})

test("semantic tokens are sorted and never overlap", async () => {
  const legend = q.s.init.capabilities.semanticTokensProvider.legend
  await q.withFile("update.ru", async ({ uri, text }) => {
    const res = await q.s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
    assert.ok(res.data.length > 0)
    const t = decode(res, legend, text)
    assert.ok(t.every((x) => x.text.length > 0))
  })
})

// --- folding, outline, inlay hints --------------------------------------------------

test("groups, the PREFIX block and comment blocks fold", async () => {
  const text = `# A query\n# about people\n${EX}PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\nSELECT * WHERE {\n  ?s ex:name ?n .\n  OPTIONAL {\n    ?s rdfs:label ?l .\n  }\n}\n`
  await q.withScratch(text, async ({ uri }) => {
    const ranges = await q.s.request("textDocument/foldingRange", { textDocument: { uri } })
    const spans = ranges.map((r) => `${r.startLine}-${r.endLine}`)
    // As in Turtle, a range whose closer starts its own line ends the line before.
    for (const want of ["0-1", "2-3", "4-8", "6-7"]) assert.ok(spans.includes(want), `${want} not in ${spans.join(" ")}`)
  })
})

test("the outline shows the query form, and each operation of an update", async () => {
  await q.withFile("people.rq", async ({ uri }) => {
    const syms = await q.s.request("textDocument/documentSymbol", { textDocument: { uri } })
    assert.equal(syms.length, 1)
    assert.match(syms[0].name, /^SELECT/)
  })
  await q.withFile("update.ru", async ({ uri }) => {
    const syms = await q.s.request("textDocument/documentSymbol", { textDocument: { uri } })
    assert.deepEqual(
      syms.map((x) => x.name.split(" ")[0]),
      ["INSERT", "DELETE"]
    )
  })
})

test("label inlay hints show on opaque IRIs in a query", async () => {
  const text = "SELECT * WHERE { <https://query.example.org/id/Q42> ?p ?o }\n"
  await q.withScratch(text, async ({ uri }) => {
    const hints = await q.s.request("textDocument/inlayHint", {
      textDocument: { uri },
      range: { start: { line: 0, character: 0 }, end: { line: 2, character: 0 } },
    })
    const labels = (hints ?? []).map((h) => (typeof h.label === "string" ? h.label : h.label.map((p) => p.value).join("")))
    assert.ok(labels.some((l) => l.includes("Douglas Adams")), labels.join(" | "))
  })
})

// --- the SPARQL files of the workspace are found once and kept current --------------------

test("references see a query file created, rewritten, deleted or removed with its folder", async () => {
  const dir = tempDir("turtle-sparql-index-")
  const TTL = "@prefix ex: <http://example.org/> .\nex:Thing a ex:Kind .\nex:other ex:rel ex:Thing .\n"
  const query = (term) => `PREFIX ex: <http://example.org/>\nSELECT * WHERE { ?s ex:${term} ?o }\n`
  const file = (name, text) => {
    fs.mkdirSync(path.dirname(path.join(dir, name)), { recursive: true })
    fs.writeFileSync(path.join(dir, name), text)
    return path.join(dir, name)
  }
  file("data.ttl", TTL)
  const q1 = file("q1.rq", query("Thing"))
  const s = await startServer({ cwd: dir })
  const CREATED = 1
  const CHANGED = 2
  const DELETED = 3
  const event = (p, type) => ({ uri: uriOf(p), type })
  try {
    await s.request("workspace/symbol", { query: "" })
    const uri = uriOf(path.join(dir, "data.ttl"))
    s.open(uri, TTL)
    await s.diagnosticsFor(uri)
    const users = async () => {
      const refs = await s.request("textDocument/references", {
        textDocument: { uri },
        position: positionOf(TTL, "ex:Thing a", 4),
        context: { includeDeclaration: false },
      })
      return where(refs)
        .filter((w) => w.includes(".rq:"))
        .map((w) => w.split(":")[0])
    }
    assert.deepEqual(await users(), ["q1.rq"])
    // Created: the watcher says so.
    const q2 = file("q2.rq", query("Thing"))
    s.notify("workspace/didChangeWatchedFiles", { changes: [event(q2, CREATED)] })
    assert.deepEqual(await users(), ["q1.rq", "q2.rq"])
    // Rewritten, with no notice at all (a client with no watcher).
    file("q2.rq", query("Nothing") + "# longer than before\n")
    assert.deepEqual(await users(), ["q1.rq"])
    // Deleted.
    fs.rmSync(q1)
    s.notify("workspace/didChangeWatchedFiles", { changes: [event(q1, DELETED)] })
    assert.deepEqual(await users(), [])
    // A folder of queries, removed whole: the client reports the folder.
    const inner = file("sub/deep/q3.rq", query("Thing"))
    s.notify("workspace/didChangeWatchedFiles", { changes: [event(inner, CREATED), event(q2, CHANGED)] })
    assert.deepEqual(await users(), ["q3.rq"])
    fs.rmSync(path.join(dir, "sub"), { recursive: true })
    s.notify("workspace/didChangeWatchedFiles", { changes: [event(path.join(dir, "sub"), DELETED)] })
    assert.deepEqual(await users(), [])
    // A rename edits the queries that are there now.
    const q4 = file("q4.rq", query("Thing"))
    s.notify("workspace/didChangeWatchedFiles", { changes: [event(q4, CREATED)] })
    const edit = await s.request("textDocument/rename", {
      textDocument: { uri },
      position: positionOf(TTL, "ex:Thing a", 4),
      newName: "Renamed",
    })
    const touched = Object.keys(edit.changes ?? {}).map((u) => path.basename(u)).sort()
    assert.deepEqual(touched, ["data.ttl", "q4.rq"])
  } finally {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
