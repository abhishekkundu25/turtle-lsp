// An OWL/XML file (.owx, or a .owl whose text is OWL/XML) open in an
// editor, over stdio, with XML's language id: the server reports its
// errors at their spans, with the codes owx-syntax and xml-syntax, as it
// publishes, as it is pulled and after an edit. Its axioms are read as RDF
// (the W3C mapping), so it reads as a Functional file does: hover,
// definition, references, highlights, symbols and folding, here and from
// the Turtle and Functional files that use its terms; no semantic tokens or
// inlay hints. Nothing writes it but completion
// (test/owx-completion.test.mjs) and rename: a client that opts in to the
// dynamic registrations gets completion and rename for it and none of the
// other writing features, a client that does not gets empty answers from
// them, and a rename of a term it uses edits it, here or from a Turtle file,
// or is refused with its path and line while it has a syntax error.
// Its terms are known to the Turtle files, its `Import`s load, its ontology
// is one a Turtle file imports, and closed it is part of the workspace
// diagnostics. Each test names the test of test/ofn-editor.test.mjs it
// follows, or says that it has none.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"
import { applyEdits } from "./jsonld-fixtures.mjs"

// Three errors: a prefix not declared (line 8), a class expression missing
// (line 9), an entity not declared (line 13, after a character that is two
// UTF-16 code units). The class :A is declared by its full IRI, and used by
// an IRI element, an abbreviated IRI and through the entity &o;.
const ONTO = `<?xml version="1.0"?>
<!DOCTYPE Ontology [
  <!ENTITY o "http://example.org/o#">
]>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">
<Prefix name="" IRI="http://example.org/o#"/>
<Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
<Declaration><Class IRI="http://example.org/o#A"/></Declaration>
<SubClassOf><Class abbreviatedIRI=":A"/><Class abbreviatedIRI="nope:B"/></SubClassOf>
<SubClassOf><Class abbreviatedIRI=":A"/></SubClassOf>
<AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><IRI>http://example.org/o#A</IRI><Literal>A</Literal></AnnotationAssertion>
<SubClassOf><Class abbreviatedIRI=":C"/><Class abbreviatedIRI=":A"/></SubClassOf>
<ClassAssertion><Class IRI="&o;A"/><NamedIndividual abbreviatedIRI=":i"/></ClassAssertion>
<AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:comment"/><AbbreviatedIRI>:Z</AbbreviatedIRI><Literal>😀 &bad;</Literal></AnnotationAssertion>
<ClassAssertion><Class abbreviatedIRI=":T"/><NamedIndividual abbreviatedIRI=":i"/></ClassAssertion>
</Ontology>
`
// Uses the class; declares :T, which the OWL/XML file uses.
const DATA = "@prefix ex: <http://example.org/o#> .\nex:a a ex:A .\nex:T a <http://www.w3.org/2002/07/owl#Class> .\n"
// A Functional file that uses the class (and declares nothing).
const FUNCTIONAL = `Prefix(:=<http://example.org/o#>)
Ontology(<http://example.org/f>
SubClassOf(:D :A)
)
`
// Line and character (UTF-16), code, message.
const ERRORS = [
  [8, 63, "owx-syntax", `Undeclared prefix 'nope:': an OWL/XML document declares each prefix it uses, as <Prefix name="nope" IRI="…"/>`],
  [9, 40, "owx-syntax", "Expected a class expression, found </SubClassOf>"],
  [13, 119, "xml-syntax", "the entity &bad; is not declared"],
]
// The whole range of each error.
const RANGES = [rangeOf(ONTO, "nope:B"), rangeOf(ONTO, "</SubClassOf>", 'nope:B"/></SubClassOf>'), rangeOf(ONTO, "&bad;")]
// With nope:B written :B, the namespace's lint notes it: the workspace
// describes enough of its terms (as it would for the same Functional text).
const UNDESCRIBED_B = [8, 63, "undescribed-term", "':B' is not described anywhere in the workspace"]
const CLEAN = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/clean">
<Declaration><Class IRI="http://example.org/clean#K"/></Declaration>
</Ontology>
`
// The class in RDF/XML. Read as OWL/XML, its root would be an owx-syntax
// error.
const RDFXML = `<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:owl="http://www.w3.org/2002/07/owl#">
  <owl:Class rdf:about="http://example.org/o#A">
    <rdfs:label>A</rdfs:label>
  </owl:Class>
</rdf:RDF>
`
const dynamic = { dynamicRegistration: true }
const DYNAMIC = {
  textDocument: { formatting: dynamic, rename: dynamic, completion: dynamic, codeAction: dynamic, diagnostic: dynamic },
  workspace: { didChangeWatchedFiles: dynamic },
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const shown = (diags) => diags.map((d) => [d.range.start.line, d.range.start.character, d.code, d.message])
const attempt = (s, method, params) => s.request(method, params).then((result) => ({ result }), (e) => ({ error: e.message }))
const same = (want) => (d) => JSON.stringify(shown(d)) === JSON.stringify(want)

// The range of `needle` in `text` (its first occurrence after the first
// `after`), in UTF-16 code units, as JavaScript counts them.
function rangeOf(text, needle, after = "") {
  const from = text.indexOf(after)
  const at = text.indexOf(needle, from + after.length)
  if (from < 0 || at < 0) throw new Error(`not found: ${needle}`)
  const pos = (off) => {
    const lines = text.slice(0, off).split("\n")
    return { line: lines.length - 1, character: lines.at(-1).length }
  }
  return { start: pos(at), end: pos(at + needle.length) }
}

async function diagnosticsUntil(s, uri, pred, ms = 10000) {
  const end = Date.now() + ms
  let last
  while (Date.now() < end) {
    last = await s.diagnosticsFor(uri, ms)
    if (pred(last)) return last
    await sleep(50)
  }
  assert.fail(`diagnostics never matched; last: ${JSON.stringify(last)}`)
}

function workspace() {
  const dir = tempDir("turtle-owx-editor-")
  fs.writeFileSync(path.join(dir, "onto.owx"), ONTO)
  fs.writeFileSync(path.join(dir, "owlxml.owl"), ONTO)
  fs.writeFileSync(path.join(dir, "data.ttl"), DATA)
  fs.writeFileSync(path.join(dir, "uses.ofn"), FUNCTIONAL)
  const uri = (name) => uriOf(path.join(dir, name))
  return { dir, uri, owx: uri("onto.owx"), owl: uri("owlxml.owl"), data: uri("data.ttl"), ofn: uri("uses.ofn") }
}

describe("an open OWL/XML document", () => {
  let w, s
  const pos = (needle, after) => rangeOf(ONTO, needle, after).start
  const at = (needle, after) => ({ textDocument: { uri: w.owx }, position: pos(needle, after) })
  // The declaration of :A: the IRI value.
  const declared = rangeOf(ONTO, "http://example.org/o#A", "<Declaration>")
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities: DYNAMIC, initializationOptions: { registerOutsideRdfXml: true } })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  // Follows "its errors are published at their spans, as .ofn (language id
  // ofn) and as a Functional .owl (xml)", for .owx.
  test("a .owx opened as xml has its owx-syntax and xml-syntax errors at their ranges in UTF-16, published and pulled; a clean one has none", async () => {
    // The entity error's column counts the emoji before it as two units.
    assert.deepEqual(RANGES[2].start, { line: 13, character: 119 })
    s.open(w.owx, ONTO, "xml")
    const d = await s.diagnosticsFor(w.owx, 10000)
    assert.deepEqual(shown(d), ERRORS)
    assert.deepEqual(d.map((x) => x.range), RANGES)
    for (const x of d) {
      assert.equal(x.severity, 1)
      assert.equal(x.source, "turtle")
    }
    // Pulled, the same.
    const pulled = await s.request("textDocument/diagnostic", { textDocument: { uri: w.owx } })
    assert.deepEqual(shown(pulled.items), ERRORS)
    const clean = w.uri("clean.owx")
    s.open(clean, CLEAN, "xml")
    assert.deepEqual(await s.diagnosticsFor(clean, 10000), [])
    // The Turtle file that uses its class: nothing unknown.
    s.open(w.data, DATA)
    const inData = await s.diagnosticsFor(w.data, 10000)
    assert.ok(!inData.some((x) => ["unknown-term", "undescribed-term"].includes(x.code)), JSON.stringify(inData))
  })

  // Follows "an edit that fixes one error leaves the other".
  test("an edit that fixes one error leaves the others", async () => {
    s.change(w.owx, ONTO.replace("nope:B", ":B"))
    const d = await diagnosticsUntil(s, w.owx, (d) => !d.some((x) => x.code === "owx-syntax" && x.range.start.line === 8))
    assert.deepEqual(shown(d), [...ERRORS.slice(1), UNDESCRIBED_B])
    s.change(w.owx, ONTO)
    await diagnosticsUntil(s, w.owx, same(ERRORS))
  })

  // Follows "its errors are published at their spans, as .ofn (language id
  // ofn) and as a Functional .owl (xml)", for .owl; the change of syntax
  // has no Functional counterpart in that file.
  test("a .owl whose text is OWL/XML is read the same; edited into RDF/XML it is read as RDF/XML at that change, and back", async () => {
    s.open(w.owl, ONTO, "xml")
    const d = await s.diagnosticsFor(w.owl, 10000)
    assert.deepEqual(shown(d), ERRORS)
    assert.ok(!d.some((x) => x.code === "unsupported-syntax"), JSON.stringify(d))
    s.change(w.owl, RDFXML)
    const rdf = await diagnosticsUntil(s, w.owl, (d) => !d.some((x) => x.severity === 1))
    assert.deepEqual(rdf.filter((x) => /syntax/.test(x.code)), [])
    const about = rangeOf(RDFXML, "http://example.org/o#A")
    const hover = await s.request("textDocument/hover", { textDocument: { uri: w.owl }, position: about.start })
    // Described by its owl:Class element, on line 5 of the RDF/XML text.
    assert.match(hover?.contents?.value ?? "", /\[owlxml\.owl:5\]/, JSON.stringify(hover))
    assert.deepEqual(hover.range, about)
    s.change(w.owl, ONTO)
    await diagnosticsUntil(s, w.owl, same(ERRORS))
  })

  // Follows "it reads as RDF/XML does: …", its hover.
  test("hover on a class shows its label at the IRI value; at an abbreviated IRI and at a value written through an entity too", async () => {
    const hover = await s.request("textDocument/hover", at("http://example.org/o#A", "<Declaration>"))
    assert.match(hover?.contents?.value ?? "", /\*\*A\*\*/, JSON.stringify(hover))
    assert.deepEqual(hover.range, declared)
    for (const [needle, after] of [[":A", '<SubClassOf><Class abbreviatedIRI=":C"/>'], ["&o;A", "<ClassAssertion>"]]) {
      const h = await s.request("textDocument/hover", at(needle, after))
      assert.match(h?.contents?.value ?? "", /\*\*A\*\*/, `${needle} ${JSON.stringify(h)}`)
      assert.deepEqual(h.range, rangeOf(ONTO, needle, after), needle)
    }
  })

  // Follows "it reads as RDF/XML does: …", its definition.
  test("definition leads to the Declaration, or to the first axiom of an undeclared term, here and across a Turtle file; type definition and implementations", async () => {
    // owlxml.owl holds the same ontology, so it is listed too, after this
    // document's.
    const decl = (uri) => ({ uri, range: declared })
    const here = await s.request("textDocument/definition", at("&o;A", "<ClassAssertion>"))
    assert.deepEqual([here].flat(), [decl(w.owx), decl(w.owl)])
    // From the Turtle file into this one, and back.
    s.open(w.data, DATA)
    const fromData = await s.request("textDocument/definition", { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:A", 3) })
    const byUri = (x, y) => x.uri.localeCompare(y.uri)
    assert.deepEqual([fromData].flat().sort(byUri), [decl(w.owx), decl(w.owl)].sort(byUri))
    const toData = await s.request("textDocument/definition", at(":T"))
    assert.deepEqual([toData].flat(), [{ uri: w.data, range: rangeOf(DATA, "ex:T") }])
    // A term with no declaration: its first axiom.
    const undeclared = await s.request("textDocument/definition", at(":C"))
    assert.deepEqual([undeclared].flat().filter((l) => l.uri === w.owx).map((l) => l.range), [rangeOf(ONTO, ":C")])
    // Type definition: the class of an individual; implementations: the
    // subclasses and members of a class.
    const types = await s.request("textDocument/typeDefinition", at(":i"))
    assert.deepEqual([types].flat().filter((l) => l.uri === w.owx).map((l) => l.range), [declared])
    const impls = await s.request("textDocument/implementation", at("http://example.org/o#A", "<Declaration>"))
    const inOwx = [impls].flat().filter((l) => l.uri === w.owx).map((l) => l.range.start)
    assert.deepEqual(inOwx.sort((x, y) => x.line - y.line), [pos(":C"), pos(":i")])
  })

  // Follows "it reads as RDF/XML does: …", its references and highlights.
  test("references and highlights of a term, across a Turtle, a Functional and an OWL/XML file", async () => {
    // The uses in the four files, the declarations with them; none in the
    // axioms with an error (lines 8 and 9), which are not read.
    const refs = await s.request("textDocument/references", { ...at("&o;A"), context: { includeDeclaration: true } })
    const name = (uri) => ({ [w.owx]: "owx", [w.owl]: "owl", [w.data]: "ttl", [w.ofn]: "ofn" })[uri]
    const where = (r) => r.map((l) => [name(l.uri), l.range.start.line, l.range.start.character]).sort()
    const inOnto = [
      pos("http://example.org/o#A", "<Declaration>"),
      pos("http://example.org/o#A", "<IRI>"),
      pos(":A", '<SubClassOf><Class abbreviatedIRI=":C"/>'),
      pos("&o;A"),
    ].map((p) => [p.line, p.character])
    const inOfn = positionOf(FUNCTIONAL, ":A)", 0)
    const inTtl = positionOf(DATA, "ex:A", 0)
    assert.deepEqual(
      where(refs),
      [
        ...inOnto.map((p) => ["owl", ...p]),
        ...inOnto.map((p) => ["owx", ...p]),
        ["ofn", inOfn.line, inOfn.character],
        ["ttl", inTtl.line, inTtl.character],
      ].sort(),
    )
    // Highlights: every use in this file, the declaration written to.
    const highlights = await s.request("textDocument/documentHighlight", at("&o;A"))
    assert.deepEqual(highlights.map((h) => [h.range.start.line, h.kind]), [[7, 3], [10, 2], [11, 2], [12, 2]])
  })

  // Follows "it reads as RDF/XML does: …", its symbols and folding.
  test("document symbols (the prefixes, the ontology, each term once), workspace symbols, folding ranges", async () => {
    const symbols = await s.request("textDocument/documentSymbol", { textDocument: { uri: w.owx } })
    assert.deepEqual(symbols.map((x) => x.name), ["prefixes", "http://example.org/o", ":A", ":C", ":i"])
    const found = await s.request("workspace/symbol", { query: ":A" })
    assert.ok(found.some((x) => x.location.uri === w.owx), JSON.stringify(found))
    // The prefixes fold; each axiom is one line.
    const folds = await s.request("textDocument/foldingRange", { textDocument: { uri: w.owx } })
    assert.deepEqual(folds, [{ startLine: 5, endLine: 6, kind: "imports" }])
  })

  // Follows "the writing features are registered for other documents only,
  // and answer nothing in it", its answers, and the tokens and hints of "it
  // reads as RDF/XML does: …".
  test("nothing writes but completion (test/owx-completion.test.mjs) and rename: no formatting, code actions, semantic tokens or inlay hints", async () => {
    const here = at("&o;A")
    const formatted = await attempt(s, "textDocument/formatting", { textDocument: { uri: w.owx }, options: { tabSize: 2, insertSpaces: true } })
    assert.deepEqual(formatted.result ?? [], [], JSON.stringify(formatted))
    // In the class's IRI value, written with an entity: the classes, in full.
    const completion = await attempt(s, "textDocument/completion", here)
    const offered = (completion.result?.items ?? []).map((i) => i.label)
    assert.ok(offered.includes("http://example.org/o#A"), JSON.stringify(completion))
    const actions = await attempt(s, "textDocument/codeAction", {
      textDocument: { uri: w.owx },
      range: { start: { line: 8, character: 0 }, end: { line: 8, character: 80 } },
      context: { diagnostics: [] },
    })
    assert.deepEqual(actions.result ?? [], [], JSON.stringify(actions))
    // No semantic tokens (the client's XML grammar colours it) and no hints.
    const tokens = await s.request("textDocument/semanticTokens/full", { textDocument: { uri: w.owx } })
    assert.deepEqual(tokens?.data ?? [], [])
    const hints = await s.request("textDocument/inlayHint", { textDocument: { uri: w.owx }, range: { start: { line: 0, character: 0 }, end: { line: 20, character: 0 } } })
    assert.deepEqual(hints ?? [], [])
  })

  // Follows "the writing features are registered for other documents only,
  // and answer nothing in it", its registrations.
  test("the dynamic registrations of a client that opts in leave .owx documents out of everything that writes, but completion and rename; the watcher sees them", async () => {
    const registrations = s.serverRequests.filter((r) => r.method === "client/registerCapability").flatMap((r) => r.params.registrations)
    const features = registrations.filter((r) => r.method !== "workspace/didChangeWatchedFiles")
    assert.equal(features.length, 5, JSON.stringify(registrations))
    for (const r of features) {
      const selector = r.registerOptions.documentSelector
      // Rename and XML completion take RDF/XML (.owl, .rdf) and OWL/XML
      // files by name, XML completion with XML-specific trigger characters,
      // and answer by the syntax they read.
      const xml = selector.filter((f) => /owl|rdf|xml|owx/.test(JSON.stringify(f)))
      const want = {
        "turtle-rename": [{ pattern: "**/*.{owl,rdf}" }, { pattern: "**/*.owx" }],
        "turtle-xml-completion": [{ pattern: "**/*.{owl,rdf}" }, { pattern: "**/*.owx" }],
      }
      assert.deepEqual(xml, want[r.id] ?? [], r.id)
    }
    const watch = registrations.find((r) => r.method === "workspace/didChangeWatchedFiles")
    assert.ok(watch.registerOptions.watchers[0].globPattern.includes("*.owx"), JSON.stringify(watch))
  })

  // Follows the rename part of "the writing features are registered for
  // other documents only, and answer nothing in it; …".
  test("a rename of the class, from it, from the OWL/XML .owl or from the Turtle file, is refused with its path and the line of its first error", async () => {
    // Both OWL/XML files have a syntax error that may hide a use: the first
    // of them by name refuses the whole rename.
    s.open(w.owl, ONTO, "xml")
    s.open(w.data, DATA)
    const inOwl = { textDocument: { uri: w.owl }, position: pos("http://example.org/o#A", "<Declaration>") }
    const fromData = { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:A", 3) }
    for (const where of [at("http://example.org/o#A", "<Declaration>"), at(":A", '<SubClassOf><Class abbreviatedIRI=":C"/>'), inOwl, fromData]) {
      const renamed = await attempt(s, "textDocument/rename", { ...where, newName: "<http://example.org/o#Thing>" })
      assert.match(renamed.error ?? "", /^can't rename in .*onto\.owx: line 9: the document has a syntax error \(Undeclared prefix 'nope:': .*\): it is not edited until it is fixed$/, JSON.stringify(renamed))
    }
  })
})

// The class written as an IRI value and as an abbreviated IRI, in an
// OWL/XML file with no error, beside the Turtle file.
describe("a rename in an OWL/XML document", () => {
  const CLEAN = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">
<Prefix name="" IRI="http://example.org/o#"/>
<Declaration><Class IRI="http://example.org/o#A"/></Declaration>
<SubClassOf><Class abbreviatedIRI=":A"/><Class IRI="http://example.org/o#B"/></SubClassOf>
<ClassAssertion><Class abbreviatedIRI=":A"/><NamedIndividual abbreviatedIRI=":i"/></ClassAssertion>
</Ontology>
`
  let dir, owx, data, s
  before(async () => {
    dir = tempDir("turtle-owx-rename-")
    fs.writeFileSync(path.join(dir, "onto.owx"), CLEAN)
    fs.writeFileSync(path.join(dir, "data.ttl"), DATA)
    owx = uriOf(path.join(dir, "onto.owx"))
    data = uriOf(path.join(dir, "data.ttl"))
    s = await startServer({ cwd: dir, rootUri: uriOf(dir), capabilities: DYNAMIC, initializationOptions: { registerOutsideRdfXml: true } })
    s.open(owx, CLEAN, "xml")
    s.open(data, DATA)
    const d = await s.diagnosticsFor(owx, 10000)
    assert.deepEqual(d.filter((x) => /syntax/.test(x.code)), [], JSON.stringify(d))
  })
  after(async () => {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  // Follows "prepareRename gives the written form, a prefixed name or a
  // full IRI with its brackets".
  test("prepareRename gives the value: an IRI value and an abbreviated IRI, without their quotes", async () => {
    for (const [needle, after] of [["http://example.org/o#A", "<Declaration>"], [":A", "<SubClassOf>"]]) {
      const range = rangeOf(CLEAN, needle, after)
      const position = { line: range.start.line, character: range.start.character + 1 }
      const prepared = await s.request("textDocument/prepareRename", { textDocument: { uri: owx }, position })
      assert.deepEqual(prepared, range, needle)
    }
  })

  // Follows "a rename from it or from the Turtle file edits both, each use
  // in its own form".
  test("a rename from it, at either form, or from the Turtle file edits both, each use in its own form", async () => {
    const want = {
      [owx]: CLEAN.replace('IRI="http://example.org/o#A"', 'IRI="http://example.org/o#Thing"').replaceAll('abbreviatedIRI=":A"', 'abbreviatedIRI=":Thing"'),
      [data]: DATA.replace("ex:A", "ex:Thing"),
    }
    for (const [where, newName] of [
      [{ textDocument: { uri: owx }, position: rangeOf(CLEAN, "http://example.org/o#A").start }, "Thing"],
      [{ textDocument: { uri: owx }, position: rangeOf(CLEAN, ":A", "<SubClassOf>").start }, ":Thing"],
      [{ textDocument: { uri: data }, position: positionOf(DATA, "ex:A", 3) }, "ex:Thing"],
    ]) {
      const renamed = await attempt(s, "textDocument/rename", { ...where, newName })
      const changes = renamed.result?.changes ?? {}
      assert.deepEqual(Object.keys(changes).sort(), [data, owx].sort(), JSON.stringify(renamed))
      assert.equal(applyEdits(CLEAN, changes[owx]), want[owx], `${where.textDocument.uri} ${newName}`)
      assert.equal(applyEdits(DATA, changes[data]), want[data], `${where.textDocument.uri} ${newName}`)
    }
  })
})

// No counterpart in test/ofn-editor.test.mjs (src/ofn/mod.rs tests the
// Functional case): the description and the deprecation of a term, across
// a Turtle and an OWL/XML file.
describe("the terms of an OWL/XML file, in a Turtle file", () => {
  const ONTO = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">
<Prefix name="" IRI="http://example.org/o#"/>
<Prefix name="owl" IRI="http://www.w3.org/2002/07/owl#"/>
<Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
<Declaration><Class abbreviatedIRI=":Old"/></Declaration>
<Declaration><Class abbreviatedIRI=":Kept"/></Declaration>
<AnnotationAssertion><AnnotationProperty abbreviatedIRI="owl:deprecated"/><AbbreviatedIRI>:Old</AbbreviatedIRI><Literal datatypeIRI="http://www.w3.org/2001/XMLSchema#boolean">true</Literal></AnnotationAssertion>
<AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><AbbreviatedIRI>:Old</AbbreviatedIRI><Literal>old</Literal></AnnotationAssertion>
<SubClassOf><Class abbreviatedIRI=":Old"/><Class abbreviatedIRI=":Kept"/></SubClassOf>
<SubClassOf><Class abbreviatedIRI=":New"/><Class abbreviatedIRI=":Old"/></SubClassOf>
</Ontology>
`
  const DATA = "@prefix ex: <http://example.org/o#> .\nex:a a ex:Kept .\nex:b a ex:Old .\nex:c a ex:Gizmo .\n"
  let dir, s
  const uri = (name) => uriOf(path.join(dir, name))
  before(async () => {
    dir = tempDir("turtle-owx-terms-")
    fs.writeFileSync(path.join(dir, "terms.owx"), ONTO)
    fs.writeFileSync(path.join(dir, "data.ttl"), DATA)
    s = await startServer({ cwd: dir, rootUri: uriOf(dir) })
    await s.request("workspace/symbol", { query: "" })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("a term declared in it is described; a deprecated class is reported at its uses, in the Turtle file and in it, not at its own axioms", async () => {
    const codes = ["unknown-term", "undescribed-term", "deprecated-term"]
    const found = (d) => d.filter((x) => codes.includes(x.code)).map((x) => [x.code, x.range.start.line, x.range.start.character])
    const p = (needle) => positionOf(DATA, needle, 0)
    s.open(uri("data.ttl"), DATA)
    const inData = found(await s.diagnosticsFor(uri("data.ttl"), 10000))
    assert.deepEqual(inData.filter(([c]) => c === "deprecated-term"), [["deprecated-term", p("ex:Old").line, p("ex:Old").character]])
    // Only the term no file describes is reported so.
    assert.deepEqual(inData.filter(([c]) => c !== "deprecated-term").map(([, l, c]) => [l, c]), [[p("ex:Gizmo").line, p("ex:Gizmo").character]])
    s.open(uri("terms.owx"), ONTO, "xml")
    const inOnto = found(await s.diagnosticsFor(uri("terms.owx"), 10000))
    const use = rangeOf(ONTO, ":Old", '<Class abbreviatedIRI=":New"/>').start
    assert.deepEqual(inOnto, [["deprecated-term", use.line, use.character]])
  })
})

describe("the imports of OWL/XML documents", () => {
  const CATALOG = `<?xml version="1.0"?>
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <uri name="http://example.org/later" uri="later.owx"/>
</catalog>
`
  const DEP = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/dep">
<Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>
<Declaration><Class IRI="http://example.org/One"/></Declaration>
<AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><IRI>http://example.org/One</IRI><Literal>Thing one</Literal></AnnotationAssertion>
</Ontology>
`
  const APP = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/app">
<Import>http://example.org/dep</Import>
<Import>http://example.org/absent-a</Import>
<ClassAssertion><Class IRI="http://example.org/One"/><NamedIndividual IRI="http://example.org/x"/></ClassAssertion>
</Ontology>
`
  const LIB = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/lib">
<Declaration><Class IRI="http://example.org/Three"/></Declaration>
</Ontology>
`
  const USER = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://example.org/user> a owl:Ontology ;
    owl:imports <http://example.org/lib>, <http://example.org/later>, <http://example.org/absent-b> .
`
  const LATER = `<?xml version="1.0"?>
<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/later">
<Declaration><Class IRI="http://example.org/Four"/></Declaration>
</Ontology>
`
  let dir, s
  const uri = (name) => uriOf(path.join(dir, name))
  const unresolved = (d) => d.filter((x) => x.code === "unresolved-import").map((x) => x.message.match(/<([^>]*)>/)[1])
  before(async () => {
    dir = tempDir("turtle-owx-imports-")
    fs.writeFileSync(path.join(dir, "catalog-v001.xml"), CATALOG)
    fs.writeFileSync(path.join(dir, "dep.owx"), DEP)
    fs.writeFileSync(path.join(dir, "app.owx"), APP)
    fs.writeFileSync(path.join(dir, "lib.owx"), LIB)
    fs.writeFileSync(path.join(dir, "user.ttl"), USER)
    s = await startServer({ cwd: dir, rootUri: uriOf(dir), capabilities: DYNAMIC })
    await s.request("workspace/symbol", { query: "" })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  // Follows "a Functional file's imports, Functional and Turtle, are
  // loaded: hover and definition read them, and only the absent one is
  // noted".
  test("an Import of a workspace OWL/XML file loads: hover and definition read it; the unresolved one is noted at the element's content", async () => {
    s.open(uri("app.owx"), APP, "xml")
    const d = await diagnosticsUntil(s, uri("app.owx"), (d) => unresolved(d).length > 0)
    assert.deepEqual(unresolved(d), ["http://example.org/absent-a"])
    const absent = d.find((x) => x.code === "unresolved-import")
    assert.deepEqual(absent.range, rangeOf(APP, "http://example.org/absent-a"))
    const at = { textDocument: { uri: uri("app.owx") }, position: rangeOf(APP, "http://example.org/One").start }
    const hover = await s.request("textDocument/hover", at)
    assert.match(hover?.contents?.value ?? "", /Thing one/, JSON.stringify(hover))
    const def = await s.request("textDocument/definition", at)
    assert.deepEqual([def].flat(), [{ uri: uri("dep.owx"), range: rangeOf(DEP, "http://example.org/One") }])
  })

  // Follows "a Turtle import of a workspace Functional ontology is
  // resolved, and so is one whose catalog-mapped .ofn is created later".
  test("a Turtle import of a workspace OWL/XML ontology is resolved, and so is one whose catalog-mapped .owx is created later", async () => {
    s.open(uri("user.ttl"), USER)
    const d = await diagnosticsUntil(s, uri("user.ttl"), (d) => unresolved(d).length > 0)
    assert.deepEqual(unresolved(d), ["http://example.org/later", "http://example.org/absent-b"])
    fs.writeFileSync(path.join(dir, "later.owx"), LATER)
    s.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: uri("later.owx"), type: 1 }] })
    const after = await diagnosticsUntil(s, uri("user.ttl"), (d) => !unresolved(d).some((iri) => iri === "http://example.org/later"))
    assert.deepEqual(unresolved(after), ["http://example.org/absent-b"])
  })
})

describe("a closed OWL/XML file", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities: DYNAMIC, initializationOptions: { diagnostics: { workspace: true } } })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  // Follows "a closed OWL Functional file has its errors in the workspace
  // diagnostics".
  test("has its errors in the workspace diagnostics", async () => {
    for (const uri of [w.owx, w.owl]) {
      const d = await s.diagnosticsFor(uri, 15000)
      assert.deepEqual(shown(d), ERRORS, uri)
    }
  })

  // No Functional counterpart: changed on disk, it is read again.
  test("changed on disk, it is read again", async () => {
    fs.writeFileSync(path.join(w.dir, "onto.owx"), ONTO.replace("nope:B", ":B"))
    s.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: w.owx, type: 2 }] })
    const d = await diagnosticsUntil(s, w.owx, same([...ERRORS.slice(1), UNDESCRIBED_B]), 15000)
    assert.deepEqual(d.map((x) => x.range.start.line), [9, 13, 8])
  })
})
