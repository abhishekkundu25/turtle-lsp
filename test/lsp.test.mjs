import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const ws = path.join(fixtures, "workspace")
const peoplePath = path.join(ws, "people.ttl")
const orgPath = path.join(ws, "org.ttl")
const people = fs.readFileSync(peoplePath, "utf8")
const org = fs.readFileSync(orgPath, "utf8")
const scratch = (name) => uriOf(path.join(ws, `__${name}.ttl`))

let s
before(async () => {
  s = await startServer()
})
after(async () => {
  await s.stop()
})

const problems = (diags) => diags.filter((d) => d.severity <= 2)
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.message}`).join("\n")

async function diagnose(name, text) {
  const uri = scratch(name)
  s.open(uri, text)
  return { uri, diags: await s.diagnosticsFor(uri) }
}

// ---------- protocol hygiene ----------

test("server writes nothing to stdout except LSP messages", async () => {
  assert.deepEqual(s.stray, [])
})

test("initialize answers quickly", async () => {
  assert.ok(s.initMs < 500, `initialize took ${s.initMs.toFixed(0)}ms`)
})

// ---------- diagnostics ----------

test("valid file has no warnings or errors", async () => {
  const { diags } = await diagnose("valid", people)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("object on the next line is not flagged as a missing terminator", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p
    ex:o .
`
  const { diags } = await diagnose("multiline-stmt", text)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("multi-line string literal content is not linted", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p """first line
  see also foo:bar and nothing ends here
last line""" .
`
  const { diags } = await diagnose("multiline-string", text)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("syntax error is reported on the offending line", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p ex:o .
ex:t ex:p "unterminated .
`
  const { diags } = await diagnose("syntax", text)
  const errors = diags.filter((d) => d.severity === 1)
  assert.ok(errors.length > 0, "expected a syntax error")
  assert.ok(errors.some((d) => d.range.start.line === 2), fmt(errors))
})

test("undeclared prefix is warned once, on the use", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p nope:o .
`
  const { diags } = await diagnose("undeclared", text)
  const w = diags.filter((d) => /nope/.test(d.message))
  assert.equal(w.length, 1, fmt(diags))
  assert.equal(w[0].range.start.line, 1)
  assert.equal(w[0].range.start.character, 10)
})

test("unknown term in a well-known vocabulary is warned on first open", async () => {
  const text = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.org/> .
ex:s rdfs:labell "x" .
`
  const { diags } = await diagnose("unknown-term", text)
  assert.ok(diags.some((d) => /labell/.test(d.message)), fmt(diags))
})

test("a well-known prefix name bound to a custom IRI is not checked against that vocabulary", async () => {
  const text = `@prefix foaf: <http://my.org/not-foaf#> .
foaf:thing foaf:whatever foaf:other .
`
  const uri = scratch("custom-iri")
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  // give the lazy vocab load a chance, then re-check
  await new Promise((r) => setTimeout(r, 1500))
  s.change(uri, text + "\n")
  const diags = await s.diagnosticsFor(uri)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("prefix declaration ranges point at the declaration", async () => {
  const text = `@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix : <http://example.org/> .
:a rdf:type :B .
`
  const uri = scratch("empty-prefix")
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const syms = await s.request("textDocument/documentSymbol", { textDocument: { uri } })
  const decls = syms.flatMap((x) => x.children ?? [x])
  const empty = decls.find((x) => x.name === ":" || x.name.startsWith(": "))
  assert.ok(empty, JSON.stringify(syms))
  const range = empty.selectionRange ?? empty.location.range
  assert.equal(range.start.line, 1, JSON.stringify(empty))
})

// ---------- completion ----------

async function complete(uri, text, needle) {
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const res = await s.request("textDocument/completion", {
    textDocument: { uri },
    position: positionOf(text, needle),
  })
  return Array.isArray(res) ? res : res.items
}

test("completion after 'foaf:' offers foaf terms only", async () => {
  const text = `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix ex: <http://example.org/> .
ex:a a foaf:
`
  const items = await complete(scratch("cmp-foaf"), text, "a foaf:")
  const labels = items.map((i) => i.label)
  assert.ok(labels.includes("foaf:Person"), labels.slice(0, 20).join(", "))
  const foreign = labels.filter((l) => /^[\w-]+:/.test(l) && !l.startsWith("foaf:"))
  assert.deepEqual(foreign, [])
})

test("completion at an empty position returns a bounded list", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a ex:p 
`
  const items = await complete(scratch("cmp-empty"), text, "ex:p ")
  assert.ok(items.length < 1000, `got ${items.length} items`)
})

test("completing a term from an undeclared well-known prefix adds the @prefix line", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a a skos:Conc
`
  const items = await complete(scratch("cmp-autoimport"), text, "skos:Conc")
  const concept = items.find((i) => i.label === "skos:Concept")
  assert.ok(concept, "skos:Concept offered")
  assert.ok(concept.additionalTextEdits?.some((e) => e.newText.includes("@prefix skos:")), JSON.stringify(concept))
})

test("hover on a vocabulary term shows its documentation", async () => {
  const uri = scratch("hover")
  s.open(uri, people)
  await s.diagnosticsFor(uri)
  const h = await s.request("textDocument/hover", {
    textDocument: { uri },
    position: positionOf(people, "a foaf:Pers"),
  })
  assert.ok(h && JSON.stringify(h.contents).length > 20, JSON.stringify(h))
})

test("completion without a prefix finds vocabulary terms by local name", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a a Pers
`
  const items = await complete(scratch("cmp-noprefix"), text, "a Pers")
  const person = items.find((i) => i.label === "foaf:Person")
  assert.ok(person, items.map((i) => i.label).slice(0, 20).join(", "))
  assert.ok(person.additionalTextEdits?.some((e) => e.newText.includes("@prefix foaf:")), "adds @prefix foaf")
})

test("'@' triggers completion of directives", async () => {
  assert.ok(s.init.capabilities.completionProvider.triggerCharacters.includes("@"))
  const text = `@
`
  const items = await complete(scratch("cmp-at"), text, "@")
  assert.ok(items.some((i) => i.label === "@prefix"), items.map((i) => i.label).join(", "))
})

test("nested [ ] and ( ) blocks fold", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a ex:knows [
        ex:name "B" ;
        ex:age 3
    ] ;
    ex:list (
        ex:x
        ex:y
    ) .
`
  const uri = scratch("fold")
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const folds = (await s.request("textDocument/foldingRange", { textDocument: { uri } })).map((f) => `${f.startLine}-${f.endLine}`)
  assert.ok(folds.includes("1-3"), folds.join(" "))
  assert.ok(folds.includes("5-7"), folds.join(" "))
  assert.ok(folds.includes("1-8"), folds.join(" "))
})

// ---------- navigation (own server: only the two fixture files are indexed) ----------

describe("navigation", () => {
  let n
  before(async () => {
    n = await startServer()
  })
  after(async () => {
    await n.stop()
  })


  test("go to definition of a subject jumps to where it is described, not to every use", async () => {
    const uri = uriOf(orgPath)
    n.open(uri, org)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/definition", {
      textDocument: { uri },
      position: positionOf(org, "ex:alic"),
    })
    assert.equal(locs.length, 1, JSON.stringify(locs))
    assert.equal(locs[0].uri, uriOf(peoplePath))
    assert.equal(locs[0].range.start.line, 4)
  })

  test("references have no duplicates", async () => {
    const uri = uriOf(peoplePath)
    n.open(uri, people)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/references", {
      textDocument: { uri },
      position: positionOf(people, "ex:bo"),
      context: { includeDeclaration: true },
    })
    const keys = locs.map((l) => `${l.uri}#${l.range.start.line}:${l.range.start.character}`)
    assert.deepEqual(keys, [...new Set(keys)])
    assert.equal(keys.length, 2, keys.join("\n"))
  })

  test("rename produces non-overlapping edits", async () => {
    const uri = uriOf(peoplePath)
    n.open(uri, people)
    await n.diagnosticsFor(uri)
    const edit = await n.request("textDocument/rename", {
      textDocument: { uri },
      position: positionOf(people, "ex:alic"),
      newName: "ex:alicia",
    })
    for (const [u, edits] of Object.entries(edit.changes)) {
      const keys = edits.map((e) => `${e.range.start.line}:${e.range.start.character}`)
      assert.deepEqual(keys, [...new Set(keys)], `duplicate edits in ${u}`)
    }
    const total = Object.values(edit.changes).flat().length
    assert.equal(total, 2, JSON.stringify(edit.changes)) // 1 in people.ttl, 1 in org.ttl
  })

  test("closing a workspace file keeps it in the workspace index", async () => {
    const uri = uriOf(peoplePath)
    n.open(uri, people)
    await n.diagnosticsFor(uri)
    n.close(uri)
    const orgUri = uriOf(orgPath)
    n.open(orgUri, org)
    await n.diagnosticsFor(orgUri)
    const locs = await n.request("textDocument/definition", {
      textDocument: { uri: orgUri },
      position: positionOf(org, "ex:alic"),
    })
    assert.ok(locs.some((l) => l.uri === uri), JSON.stringify(locs))
  })

  test("type definition jumps to the resource's class", async () => {
    const path_ = path.join(ws, "classes.ttl")
    const text = fs.readFileSync(path_, "utf8")
    const uri = uriOf(path_)
    n.open(uri, text)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/typeDefinition", { textDocument: { uri }, position: positionOf(text, "ex:dav") })
    assert.deepEqual(locs.map((l) => l.range.start.line), [7], JSON.stringify(locs))
  })

  test("implementation lists instances and subclasses", async () => {
    const path_ = path.join(ws, "classes.ttl")
    const text = fs.readFileSync(path_, "utf8")
    const uri = uriOf(path_)
    n.open(uri, text)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/implementation", { textDocument: { uri }, position: positionOf(text, "ex:Employe") })
    assert.deepEqual(locs.map((l) => l.range.start.line).sort(), [11, 7].sort(), JSON.stringify(locs))
  })
})

test("workspace root comes from the client's rootUri, not the process cwd", async () => {
  const other = await startServer({ cwd: os.tmpdir(), rootUri: uriOf(ws) })
  try {
    const orgUri = uriOf(orgPath)
    other.open(orgUri, org)
    await other.diagnosticsFor(orgUri)
    const locs = await other.request("textDocument/definition", {
      textDocument: { uri: orgUri },
      position: positionOf(org, "ex:alic"),
    })
    assert.ok(locs.some((l) => l.uri === uriOf(peoplePath)), JSON.stringify(locs))
  } finally {
    await other.stop()
  }
})
