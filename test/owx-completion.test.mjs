// Completion in an OWL/XML file (.owx) over stdio, with XML's language id:
// Ctrl+Space after `<` offers the elements the structural specification
// takes there (the class expressions inside SubClassOf), an IRI value the
// terms of its element's kind in full (classes in <Class IRI="…"/>), this
// file's and the workspace's, and an abbreviatedIRI value the terms over the
// file's <Prefix> elements: one whose prefix the file does not declare adds
// its <Prefix> element. Nothing in a comment. A client that takes snippets
// gets an element whole.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"
import { applyEdits } from "./jsonld-fixtures.mjs"

const ONTO = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">
    <Prefix name="" IRI="http://example.org/o#"/>
    <Declaration><Class IRI="http://example.org/o#Pump"/></Declaration>
    <Declaration><ObjectProperty abbreviatedIRI=":feeds"/></Declaration>
    <Declaration><NamedIndividual abbreviatedIRI=":p1"/></Declaration>

</Ontology>
`
// A Turtle file of the workspace, in the same namespace.
const PARTS = `@prefix : <http://example.org/o#> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
:Valve a owl:Class ; rdfs:label "A valve" .
`
const CAPABILITIES = { textDocument: { completion: { completionItem: { snippetSupport: true } } } }

const labels = (res) => (res?.items ?? res ?? []).map((i) => i.label)

describe("completion in an OWL/XML document", () => {
  let dir, s, uri
  let version = 1
  before(async () => {
    dir = tempDir("turtle-owx-completion-")
    fs.writeFileSync(path.join(dir, "onto.owx"), ONTO)
    fs.writeFileSync(path.join(dir, "parts.ttl"), PARTS)
    uri = uriOf(path.join(dir, "onto.owx"))
    s = await startServer({ cwd: dir, rootUri: uriOf(dir), capabilities: CAPABILITIES })
    // The workspace index has read parts.ttl.
    await s.request("workspace/symbol", { query: "" })
    s.open(uri, ONTO, "xml")
    await s.diagnosticsFor(uri, 10000)
  })
  after(async () => {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  // The completion with `typed` written on the empty line of the ontology,
  // the cursor at its `|`.
  async function complete(typed) {
    const caret = typed.indexOf("|")
    const line = typed.slice(0, caret) + typed.slice(caret + 1)
    const text = ONTO.replace("\n\n</Ontology>", `\n    ${line}\n</Ontology>`)
    s.change(uri, text, ++version)
    const position = positionOf(text, line, caret)
    const res = await s.request("textDocument/completion", { textDocument: { uri }, position })
    return { res, text, position }
  }

  // The completion in the whole text `marked`, at its `|`.
  async function completeAll(marked) {
    const caret = marked.indexOf("|")
    const text = marked.slice(0, caret) + marked.slice(caret + 1)
    s.change(uri, text, ++version)
    const position = positionOf(text, marked.slice(0, caret))
    const res = await s.request("textDocument/completion", { textDocument: { uri }, position })
    return { res, text, position }
  }

  test("inside SubClassOf: the class and the class expressions, as snippets", async () => {
    const { res } = await complete("<SubClassOf><Class abbreviatedIRI=\":Pump\"/><|</SubClassOf>")
    const got = labels(res)
    for (const want of ["Class", "ObjectSomeValuesFrom", "ObjectIntersectionOf"]) assert.ok(got.includes(want), `${want} in ${got.join(", ")}`)
    for (const not of ["ObjectProperty", "SubClassOf", "Literal", ":Pump"]) assert.ok(!got.includes(not), `${not} in ${got.join(", ")}`)
    const item = (label) => res.items.find((i) => i.label === label)
    assert.equal(item("Class").insertTextFormat, 2)
    assert.equal(item("Class").textEdit.newText, 'Class IRI="$1"/>')
    assert.equal(item("ObjectSomeValuesFrom").textEdit.newText, "ObjectSomeValuesFrom>$0</ObjectSomeValuesFrom>")
  })

  test("in a class's IRI value: the classes in full, this file's and the workspace's", async () => {
    const { res, text } = await complete('<SubClassOf><Class abbreviatedIRI=":Pump"/><Class IRI="http://example.org/o#|"/></SubClassOf>')
    const got = labels(res)
    for (const want of ["http://example.org/o#Pump", "http://example.org/o#Valve"]) assert.ok(got.includes(want), `${want} in ${got.join(", ")}`)
    for (const not of ["http://example.org/o#feeds", "http://example.org/o#p1"]) assert.ok(!got.includes(not), `${not} in ${got.join(", ")}`)
    const valve = res.items.find((i) => i.label === "http://example.org/o#Valve")
    assert.equal(valve.detail, "A valve")
    const start = positionOf(text, '<Class IRI="http://example.org/o#"/>', 12)
    assert.deepEqual(valve.textEdit.range, { start, end: { line: start.line, character: start.character + 21 } })
    // Resolved, the item has the hover's documentation.
    const resolved = await s.request("completionItem/resolve", valve)
    assert.match(resolved.documentation?.value ?? "", /valve/i, JSON.stringify(resolved))
  })

  test("in an abbreviatedIRI value: a term of a prefix not declared adds its <Prefix> element", async () => {
    const { res, text } = await complete('<SubClassOf><Class abbreviatedIRI=":Pump"/><Class abbreviatedIRI="skos:Con|"/></SubClassOf>')
    const concept = res.items.find((i) => i.label === "skos:Concept")
    assert.ok(concept, labels(res).join(", "))
    const start = positionOf(text, "skos:Con", 0)
    assert.deepEqual(concept.textEdit.range, { start, end: { line: start.line, character: start.character + 8 } })
    assert.deepEqual(concept.additionalTextEdits, [
      {
        range: { start: { line: 2, character: 49 }, end: { line: 2, character: 49 } },
        newText: '\n    <Prefix name="skos" IRI="http://www.w3.org/2004/02/skos/core#"/>',
      },
    ])
  })

  test("a value with no closing quote: the edit keeps the end of the tag and what comes after it", async () => {
    const XSD = "http://www.w3.org/2001/XMLSchema#"
    const int = `${XSD}integer`
    const pump = "http://example.org/o#Pump"
    const literal = (l) => `<DataPropertyAssertion><DataProperty abbreviatedIRI=":power"/><NamedIndividual abbreviatedIRI=":p1"/>${l}</DataPropertyAssertion>`
    const sub = (l) => `<SubClassOf>${l}<Class abbreviatedIRI=":Pump"/></SubClassOf>`
    const last = (l) => `${ONTO.slice(0, ONTO.indexOf("\n\n</Ontology>"))}\n    ${l}`
    const cases = [
      // `>` and text after the value.
      [complete, literal(`<Literal datatypeIRI="${XSD}int|>42</Literal>`), int, `    ${literal(`<Literal datatypeIRI="${int}>42</Literal>`)}`],
      // `/>` after the value.
      [complete, sub('<Class IRI="http://example.org/o#P|/>'), pump, `    ${sub(`<Class IRI="${pump}/>`)}`],
      // `>` and a child element.
      [complete, sub('<Class IRI="http://example.org/o#P|>'), pump, `    ${sub(`<Class IRI="${pump}>`)}`],
      // At the end of the text.
      [completeAll, last(`<Declaration><Datatype IRI="${XSD}int|/>`), int, `    <Declaration><Datatype IRI="${int}/>`],
      [completeAll, last(`<Declaration><Datatype IRI="${XSD}int|`), int, `    <Declaration><Datatype IRI="${int}`],
      // White space, then an attribute in the other quotes: to the white space.
      [complete, sub(`<Class IRI="http://example.org/o#P| abbreviatedIRI=':Pump'/>`), pump, `    ${sub(`<Class IRI="${pump} abbreviatedIRI=':Pump'/>`)}`],
      // Closed: between the quotes.
      [complete, sub('<Class IRI="http://example.org/o#P|"/>'), pump, `    ${sub(`<Class IRI="${pump}"/>`)}`],
    ]
    for (const [how, typed, label, want] of cases) {
      const { res, text, position } = await how(typed)
      const item = (res?.items ?? []).find((i) => i.label === label)
      assert.ok(item, `${label} in ${labels(res).join(", ")}: ${typed}`)
      const accepted = applyEdits(text, [item.textEdit, ...(item.additionalTextEdits ?? [])])
      assert.equal(accepted.split("\n")[position.line], want, typed)
    }
  })

  test("nothing in a comment", async () => {
    const { res } = await complete("<!-- <SubClassOf><Cl| -->")
    assert.deepEqual(labels(res), [])
  })

  test("closing tag and attribute names", async () => {
    const end = await complete('<SubClassOf><Class abbreviatedIRI=":Pump"/></|')
    assert.deepEqual(labels(end.res), ["SubClassOf"])
    assert.equal(end.res.items[0].textEdit.newText, "SubClassOf>")
    const attrs = await complete("<Declaration><Class |/></Declaration>")
    assert.deepEqual(labels(attrs.res), ["IRI", "abbreviatedIRI"])
    assert.equal(attrs.res.items[0].insertTextFormat, 2)
    assert.equal(attrs.res.items[0].textEdit.newText, 'IRI="$0"')
  })
})
