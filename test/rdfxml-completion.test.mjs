// Completion in an RDF/XML file (.owl, .rdf) over stdio: Ctrl+Space after
// `<` offers the names the RDF/XML striping takes there (rdf:Description
// and the classes for a node element, the properties for a property
// element) as qualified names; a term whose namespace has no prefix in
// scope adds an xmlns declaration to the root element. In rdf:about,
// rdf:resource and rdf:datatype values it offers IRIs written whole, or
// after the entity reference the value starts with. Nothing comes in an XML
// literal. A .owl that holds Turtle gets Turtle's completion.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"
import { applyEdits } from "./jsonld-fixtures.mjs"

const HEAD = `<?xml version="1.0"?>
<!DOCTYPE rdf:RDF [
  <!ENTITY ex "http://example.org/">
]>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:owl="http://www.w3.org/2002/07/owl#"
         xmlns:ex="http://example.org/">
  <owl:Class rdf:about="http://example.org/Pump">
    <rdfs:label>Pump</rdfs:label>
  </owl:Class>
  <owl:Class rdf:about="http://example.org/Valve"/>
`
const ONTO = `${HEAD}
</rdf:RDF>
`
const TURTLE = `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
ex:Tank a owl:Class .
`

const labels = (res) => (res?.items ?? res ?? []).map((i) => i.label)

describe("completion in an RDF/XML document", () => {
  let dir, s, uri, turtle
  let version = 1
  before(async () => {
    dir = tempDir("turtle-rdfxml-completion-")
    fs.writeFileSync(path.join(dir, "onto.owl"), ONTO)
    fs.writeFileSync(path.join(dir, "turtle.owl"), TURTLE)
    uri = uriOf(path.join(dir, "onto.owl"))
    turtle = uriOf(path.join(dir, "turtle.owl"))
    s = await startServer({ cwd: dir, rootUri: uriOf(dir) })
    s.open(uri, ONTO, "xml")
    await s.diagnosticsFor(uri, 10000)
  })
  after(async () => {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  // The completion with `typed` written before `</rdf:RDF>`, at its `|`.
  async function complete(typed) {
    const caret = typed.indexOf("|")
    const line = typed.slice(0, caret) + typed.slice(caret + 1)
    const text = ONTO.replace("\n</rdf:RDF>", `\n${line}\n</rdf:RDF>`)
    s.change(uri, text, ++version)
    const before = HEAD + "\n" + typed.slice(0, caret)
    const position = positionOf(text, before)
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

  test("a node element: rdf:Description and the classes, as qualified names", async () => {
    const { res, text } = await complete("  <owl:Cl|")
    assert.equal(labels(res)[0], "owl:Class", labels(res).join(", "))
    const start = positionOf(text, `${HEAD}\n  <`)
    assert.deepEqual(res.items[0].textEdit.range, { start, end: { line: start.line, character: start.character + 6 } })
    const all = labels((await complete("  <|")).res)
    for (const want of ["rdf:Description", "ex:Pump", "ex:Valve", "owl:Class"]) assert.ok(all.includes(want), `${want} in ${all.join(", ")}`)
    assert.ok(!all.includes("rdfs:label"), all.join(", "))
  })

  test("a property element: the properties; one of a namespace not in scope adds xmlns to the root", async () => {
    const { res, text } = await complete('  <owl:Class rdf:about="http://example.org/Hose">\n    <skos:prefL|')
    const pref = res.items.find((i) => i.label === "skos:prefLabel")
    assert.ok(pref, labels(res).join(", "))
    assert.deepEqual(pref.additionalTextEdits, [
      { range: { start: { line: 7, character: 39 }, end: { line: 7, character: 39 } }, newText: '\n         xmlns:skos="http://www.w3.org/2004/02/skos/core#"' },
    ])
    const accepted = applyEdits(text, [pref.textEdit, ...pref.additionalTextEdits])
    assert.match(accepted, /xmlns:ex="http:\/\/example.org\/"\n {9}xmlns:skos="http:\/\/www.w3.org\/2004\/02\/skos\/core#">/)
    assert.match(accepted, /<skos:prefLabel\n/)
    const props = labels((await complete('  <owl:Class rdf:about="http://example.org/Hose">\n    <rdfs:|')).res)
    for (const want of ["rdfs:label", "rdfs:subClassOf"]) assert.ok(props.includes(want), `${want} in ${props.join(", ")}`)
    assert.ok(!props.includes("rdfs:Class"), props.join(", "))
  })

  test("an rdf:resource value: IRIs whole, classes for rdfs:subClassOf, with the label as detail", async () => {
    const { res, text } = await complete('  <owl:Class rdf:about="http://example.org/Hose">\n    <rdfs:subClassOf rdf:resource="http://example.org/P|"/>')
    assert.deepEqual(labels(res), ["http://example.org/Pump"])
    const pump = res.items[0]
    assert.equal(pump.detail, "Pump")
    const start = positionOf(text, 'rdf:resource="', 'rdf:resource="'.length)
    assert.deepEqual(pump.textEdit.range, { start, end: { line: start.line, character: start.character + 20 } })
    const resolved = await s.request("completionItem/resolve", pump)
    assert.match(resolved.documentation?.value ?? "", /Pump/, JSON.stringify(resolved))
  })

  test("a value that starts with an entity: that namespace's terms after it", async () => {
    const { res } = await complete('  <owl:Class rdf:about="http://example.org/Hose">\n    <rdfs:subClassOf rdf:resource="&ex;Va|"/>')
    assert.ok(labels(res).includes("&ex;Valve"), labels(res).join(", "))
    assert.ok(labels(res).every((l) => l.startsWith("&ex;")), labels(res).join(", "))
  })

  test("nothing in an XML literal", async () => {
    const { res } = await complete('  <owl:Class rdf:about="http://example.org/Hose">\n    <rdfs:comment rdf:parseType="Literal"><b>a</b><|')
    assert.deepEqual(labels(res), [])
  })

  test("a .owl that holds Turtle gets Turtle's completion", async () => {
    const text = `${TURTLE}ex:Hose a owl:Cl`
    s.open(turtle, text, "xml")
    await s.diagnosticsFor(turtle, 10000)
    const res = await s.request("textDocument/completion", { textDocument: { uri: turtle }, position: positionOf(text, "a owl:Cl") })
    assert.ok(labels(res).includes("owl:Class"), labels(res).join(", "))
  })

  test("a value with no closing quote: the edit keeps the end of the tag and what comes after it", async () => {
    const XSD = "http://www.w3.org/2001/XMLSchema#"
    const int = `${XSD}integer`
    const pump = "http://example.org/Pump"
    const desc = (l) => `  <rdf:Description rdf:about="http://example.org/s">\n    ${l}\n  </rdf:Description>`
    const cls = (l) => `  <owl:Class rdf:about="http://example.org/Hose">\n    ${l}\n  </owl:Class>`
    const last = (l) => `${HEAD}  <rdf:Description rdf:about="http://example.org/s">\n    ${l}`
    const cases = [
      // `>` and text after the value.
      [complete, desc(`<ex:age rdf:datatype="${XSD}int|>42</ex:age>`), int, `    <ex:age rdf:datatype="${int}>42</ex:age>`],
      // `/>`, then the line end.
      [complete, cls('<rdfs:subClassOf rdf:resource="http://example.org/P|/>'), pump, `    <rdfs:subClassOf rdf:resource="${pump}/>`],
      // `>` and a child element.
      [complete, '  <rdf:Description rdf:type="http://example.org/P|><rdfs:label>x</rdfs:label></rdf:Description>', pump, `  <rdf:Description rdf:type="${pump}><rdfs:label>x</rdfs:label></rdf:Description>`],
      // At the end of the text.
      [completeAll, last(`<ex:p rdf:datatype="${XSD}int|/>`), int, `    <ex:p rdf:datatype="${int}/>`],
      [completeAll, last(`<ex:p rdf:datatype="${XSD}int|`), int, `    <ex:p rdf:datatype="${int}`],
      // White space, then an attribute in the other quotes: to the white space.
      [complete, cls(`<rdfs:subClassOf rdf:resource="http://example.org/P| rdf:ID='r1'/>`), pump, `    <rdfs:subClassOf rdf:resource="${pump} rdf:ID='r1'/>`],
      // Closed: between the quotes.
      [complete, cls('<rdfs:subClassOf rdf:resource="http://example.org/P|"/>'), pump, `    <rdfs:subClassOf rdf:resource="${pump}"/>`],
    ]
    for (const [how, typed, label, want] of cases) {
      const { res, text, position } = await how(typed)
      const item = (res?.items ?? []).find((i) => i.label === label)
      assert.ok(item, `${label} in ${labels(res).join(", ")}: ${typed}`)
      const accepted = applyEdits(text, [item.textEdit, ...(item.additionalTextEdits ?? [])])
      assert.equal(accepted.split("\n")[position.line], want, typed)
    }
  })

  test("closing tag, attribute names, rdf:parseType and rdf:type values", async () => {
    const end = await complete('  <owl:Class rdf:about="http://example.org/Hose">\n    </|')
    assert.deepEqual(labels(end.res), ["owl:Class"])
    assert.equal(end.res.items[0].textEdit.newText, "owl:Class>")
    const attrs = await complete('  <rdf:Description rdf:about="http://example.org/Hose">\n    <ex:feeds |/>')
    for (const want of ["rdf:resource", "rdf:datatype", "rdf:parseType", "rdf:ID", "xml:lang"]) {
      assert.ok(labels(attrs.res).includes(want), `${want} in ${labels(attrs.res).join(", ")}`)
    }
    const pt = await complete('  <rdf:Description rdf:about="http://example.org/Hose">\n    <ex:feeds rdf:parseType="|"></ex:feeds>\n  </rdf:Description>')
    assert.deepEqual(labels(pt.res), ["Resource", "Collection", "Literal", "Triple"])
    const ty = await complete('  <rdf:Description rdf:about="http://example.org/Hose" rdf:type="http://example.org/P|"/>')
    assert.deepEqual(labels(ty.res), ["http://example.org/Pump"])
  })
})
