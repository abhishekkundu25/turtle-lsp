// Completion in an OWL Functional file (.ofn) over stdio: Ctrl+Space offers
// what the grammar takes at the cursor, as Turtle does: the axiom keywords
// inside Ontology( … ), the expressions and the terms of the slot's kind
// inside a construct (classes where a class expression goes), the terms of a
// namespace after its prefix, from the vocabularies and the workspace. A term
// whose prefix the file does not declare adds a Prefix( … ) line. A client
// that takes snippets gets each keyword with its operands. A client that
// opts in to the dynamic registrations gets completion registered for the
// ofn language too.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"

const ONTO = `Prefix(:=<http://example.org/o#>)
Ontology(<http://example.org/o>
Declaration(Class(:Pump))
Declaration(ObjectProperty(:feeds))
Declaration(NamedIndividual(:p1))

)
`
// A Turtle file of the workspace, in the same namespace.
const PARTS = `@prefix : <http://example.org/o#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
:Valve a owl:Class ; rdfs:label "A valve" .
`
const dynamic = { dynamicRegistration: true }
const CAPABILITIES = {
  textDocument: { completion: { ...dynamic, completionItem: { snippetSupport: true } }, formatting: dynamic, rename: dynamic, codeAction: dynamic },
}

const labels = (res) => (res?.items ?? res ?? []).map((i) => i.label)

describe("completion in an OWL Functional document", () => {
  let dir, s, uri
  let version = 1
  before(async () => {
    dir = tempDir("turtle-ofn-completion-")
    fs.writeFileSync(path.join(dir, "onto.ofn"), ONTO)
    fs.writeFileSync(path.join(dir, "parts.ttl"), PARTS)
    uri = uriOf(path.join(dir, "onto.ofn"))
    s = await startServer({ cwd: dir, rootUri: uriOf(dir), capabilities: CAPABILITIES, initializationOptions: { registerOutsideRdfXml: true } })
    // The workspace index has read parts.ttl.
    await s.request("workspace/symbol", { query: "" })
    s.open(uri, ONTO, "ofn")
    await s.diagnosticsFor(uri, 10000)
  })
  after(async () => {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  // The completion with `typed` written on the empty line of the ontology,
  // the cursor at its end.
  async function complete(typed) {
    const text = ONTO.replace("\n\n)", `\n${typed}\n)`)
    s.change(uri, text, ++version)
    const res = await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, typed) })
    return { res, text }
  }

  test("is registered for the ofn language, and only completion is", async () => {
    const registrations = s.serverRequests.filter((r) => r.method === "client/registerCapability").flatMap((r) => r.params.registrations)
    for (const r of registrations.filter((r) => r.method !== "workspace/didChangeWatchedFiles")) {
      const languages = r.registerOptions.documentSelector.map((f) => f.language).filter(Boolean)
      assert.equal(languages.includes("ofn"), r.id === "turtle-completion", r.id)
    }
  })

  test("inside the ontology: the axiom keywords, as snippets", async () => {
    const { res } = await complete("SubCl")
    assert.equal(labels(res)[0], "SubClassOf", labels(res).join(", "))
    const item = res.items[0]
    assert.equal(item.insertTextFormat, 2)
    assert.equal(item.textEdit.newText, "SubClassOf(${1:class} ${2:class})")
    assert.ok(!labels(res).includes("ObjectSomeValuesFrom"))
  })

  test("in a class slot: the classes, this file's and the workspace's, and the class expressions", async () => {
    const { res } = await complete("SubClassOf(:Pump ")
    const got = labels(res)
    for (const want of [":Pump", ":Valve", "owl:Thing", "ObjectSomeValuesFrom", ":"]) assert.ok(got.includes(want), `${want} in ${got.join(", ")}`)
    for (const not of [":feeds", ":p1", "SubClassOf"]) assert.ok(!got.includes(not), `${not} in ${got.join(", ")}`)
    const valve = res.items.find((i) => i.label === ":Valve")
    assert.equal(valve.detail, "A valve")
    // Resolved, the item has the hover's documentation.
    const resolved = await s.request("completionItem/resolve", valve)
    assert.match(resolved.documentation?.value ?? "", /valve/i, JSON.stringify(resolved))
  })

  test("after a prefix: its terms; one not declared adds its Prefix line", async () => {
    const { res, text } = await complete("SubClassOf(:Pump skos:Con")
    const concept = res.items.find((i) => i.label === "skos:Concept")
    assert.ok(concept, labels(res).join(", "))
    const start = positionOf(text, "skos:Con", 0)
    assert.deepEqual(concept.textEdit.range, { start, end: { line: start.line, character: start.character + 8 } })
    assert.deepEqual(concept.additionalTextEdits, [
      { range: { start: { line: 1, character: 0 }, end: { line: 1, character: 0 } }, newText: "Prefix(skos:=<http://www.w3.org/2004/02/skos/core#>)\n" },
    ])
  })

  test("a slot of individuals, while the axiom is not closed", async () => {
    const { res } = await complete("ClassAssertion(:Pump ")
    assert.deepEqual(labels(res).filter((l) => l.startsWith(":") && l.length > 1), [":p1"])
  })

  test("formatting, rename and code actions still offer nothing", async () => {
    const at = { textDocument: { uri }, position: positionOf(ONTO, "Class(:Pump", 7) }
    s.change(uri, ONTO, ++version)
    const formatted = await s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 2, insertSpaces: true } })
    assert.deepEqual(formatted ?? [], [])
    const renamed = await s.request("textDocument/rename", { ...at, newName: ":Thing" }).then(() => "", (e) => e.message)
    assert.match(renamed, /OWL Functional syntax document is not edited/)
    const actions = await s.request("textDocument/codeAction", { textDocument: { uri }, range: { start: at.position, end: at.position }, context: { diagnostics: [] } })
    assert.deepEqual(actions ?? [], [])
  })
})
