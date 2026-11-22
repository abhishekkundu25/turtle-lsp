// An OWL Functional file (.ofn, or a .owl whose text is Functional) open in
// an editor, over stdio: the server reports its errors at their spans, with
// the code ofn-syntax, as it publishes, as it is pulled and after an edit.
// Its axioms are read as RDF (the W3C mapping), so it reads as an RDF/XML
// file does: hover, definition, references, highlights, symbols and
// folding, here and from the Turtle files that use its terms; no semantic
// tokens or inlay hints. Nothing writes it: a client that opts in to the
// dynamic registrations gets none of the writing features for it, a client
// that does not gets empty answers, and a rename of a term it uses is
// refused.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"

// Two errors: a prefix not declared (line 3), a class expression missing
// (line 4).
const ONTO = `Prefix(:=<http://example.org/o#>)
Ontology(<http://example.org/o>
Declaration(Class(:A))
SubClassOf(:A nope:B)
SubClassOf(:A)
AnnotationAssertion(rdfs:label :A "A")
)
`
const DATA = "@prefix ex: <http://example.org/o#> .\nex:a a ex:A .\n"
const ERRORS = [
  [3, 14, "Undeclared prefix 'nope:'"],
  [4, 13, "Expected a class expression, found ')'"],
]
const dynamic = { dynamicRegistration: true }
const DYNAMIC = {
  textDocument: { formatting: dynamic, rename: dynamic, completion: dynamic, codeAction: dynamic, diagnostic: dynamic },
  workspace: { didChangeWatchedFiles: dynamic },
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const shown = (diags) => diags.map((d) => [d.range.start.line, d.range.start.character, d.message])
const attempt = (s, method, params) => s.request(method, params).then((result) => ({ result }), (e) => ({ error: e.message }))

function workspace() {
  const dir = tempDir("turtle-ofn-editor-")
  fs.writeFileSync(path.join(dir, "onto.ofn"), ONTO)
  fs.writeFileSync(path.join(dir, "functional.owl"), ONTO)
  fs.writeFileSync(path.join(dir, "data.ttl"), DATA)
  return { dir, ofn: uriOf(path.join(dir, "onto.ofn")), owl: uriOf(path.join(dir, "functional.owl")), data: uriOf(path.join(dir, "data.ttl")) }
}

describe("an open OWL Functional document", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities: DYNAMIC, initializationOptions: { registerOutsideRdfXml: true } })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("its errors are published at their spans, as .ofn (language id ofn) and as a Functional .owl (xml)", async () => {
    for (const [uri, id] of [[w.ofn, "ofn"], [w.owl, "xml"]]) {
      s.open(uri, ONTO, id)
      const d = await s.diagnosticsFor(uri, 10000)
      assert.deepEqual(shown(d), ERRORS, uri)
      for (const x of d) {
        assert.equal(x.code, "ofn-syntax")
        assert.equal(x.severity, 1)
        assert.equal(x.source, "turtle")
      }
      assert.ok(!d.some((x) => x.code === "unsupported-syntax"), JSON.stringify(d))
    }
    // Pulled, the same.
    const pulled = await s.request("textDocument/diagnostic", { textDocument: { uri: w.ofn } })
    assert.deepEqual(shown(pulled.items), ERRORS)
    // The Turtle file that uses its class: nothing unknown.
    s.open(w.data, DATA)
    const d = await s.diagnosticsFor(w.data, 10000)
    assert.ok(!d.some((x) => ["unknown-term", "undescribed-term"].includes(x.code)), JSON.stringify(d))
  })

  test("an edit that fixes one error leaves the other", async () => {
    const fixed = ONTO.replace("nope:B", ":B")
    s.change(w.ofn, fixed)
    const end = Date.now() + 10000
    let d = []
    while (Date.now() < end) {
      d = await s.diagnosticsFor(w.ofn, 10000)
      if (d.length === 1) break
      await sleep(50)
    }
    assert.deepEqual(shown(d), [ERRORS[1]])
    s.change(w.ofn, ONTO)
  })

  test("it reads as RDF/XML does: hover, definition, references, highlights, symbols, folding; no tokens or hints", async () => {
    const at = { textDocument: { uri: w.ofn }, position: positionOf(ONTO, "Class(:A", 7) }
    // Hover: the label of the class.
    const hover = await s.request("textDocument/hover", at)
    assert.match(hover?.contents?.value ?? "", /\bA\b/, JSON.stringify(hover))
    assert.deepEqual(hover.range, { start: { line: 2, character: 18 }, end: { line: 2, character: 20 } })
    // Definition: the declaration, from here and from the Turtle file;
    // functional.owl holds the same ontology, so it is listed too, after
    // this document's.
    const decl = (uri) => ({ uri, range: { start: { line: 2, character: 18 }, end: { line: 2, character: 20 } } })
    const here = await s.request("textDocument/definition", { textDocument: { uri: w.ofn }, position: positionOf(ONTO, "rdfs:label :A", 12) })
    assert.deepEqual([here].flat(), [decl(w.ofn), decl(w.owl)])
    s.open(w.data, DATA)
    const fromData = await s.request("textDocument/definition", { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:A", 3) })
    assert.deepEqual([fromData].flat().sort((x, y) => x.uri.localeCompare(y.uri)), [decl(w.owl), decl(w.ofn)])
    // References: the uses in the three files, the declarations with them;
    // none in the axioms with an error (lines 3 and 4), which are not read.
    const refs = await s.request("textDocument/references", { ...at, context: { includeDeclaration: true } })
    const name = (uri) => ({ [w.ofn]: "ofn", [w.owl]: "owl", [w.data]: "ttl" })[uri]
    const where = (r) => r.map((l) => [name(l.uri), l.range.start.line, l.range.start.character]).sort()
    const inOnto = [[2, 18], [5, 31]]
    assert.deepEqual(where(refs), [...inOnto.map((p) => ["ofn", ...p]), ...inOnto.map((p) => ["owl", ...p]), ["ttl", 1, 7]])
    // Highlights: every use in this file, the declaration written to.
    const highlights = await s.request("textDocument/documentHighlight", at)
    assert.deepEqual(highlights.map((h) => [h.range.start.line, h.kind]), [[2, 3], [5, 2]])
    // The outline: the prefixes, the ontology, the class once.
    const symbols = await s.request("textDocument/documentSymbol", { textDocument: { uri: w.ofn } })
    assert.deepEqual(symbols.map((x) => x.name), ["prefixes", "http://example.org/o", ":A"])
    const found = await s.request("workspace/symbol", { query: ":A" })
    assert.ok(found.some((x) => x.location.uri === w.ofn), JSON.stringify(found))
    // Folding: the prefixes and each axiom of more than one line; here
    // there is one prefix, and each axiom is one line.
    const folds = await s.request("textDocument/foldingRange", { textDocument: { uri: w.ofn } })
    assert.deepEqual(folds ?? [], [])
    // No semantic tokens (the client's grammar colours it) and no hints.
    const tokens = await s.request("textDocument/semanticTokens/full", { textDocument: { uri: w.ofn } })
    assert.deepEqual(tokens?.data ?? [], [])
    const hints = await s.request("textDocument/inlayHint", { textDocument: { uri: w.ofn }, range: { start: { line: 0, character: 0 }, end: { line: 9, character: 0 } } })
    assert.deepEqual(hints ?? [], [])
  })

  test("the writing features are registered for other documents only, and answer nothing in it", async () => {
    const registrations = s.serverRequests.filter((r) => r.method === "client/registerCapability").flatMap((r) => r.params.registrations)
    const features = registrations.filter((r) => r.method !== "workspace/didChangeWatchedFiles")
    assert.equal(features.length, 4, JSON.stringify(registrations))
    for (const r of features) assert.doesNotMatch(JSON.stringify(r.registerOptions.documentSelector), /ofn|owl|xml/, r.method)
    const watch = registrations.find((r) => r.method === "workspace/didChangeWatchedFiles")
    assert.ok(watch.registerOptions.watchers[0].globPattern.includes("*.ofn"), JSON.stringify(watch))
    const at = { textDocument: { uri: w.ofn }, position: positionOf(ONTO, "Class(:A", 7) }
    const formatted = await attempt(s, "textDocument/formatting", { textDocument: { uri: w.ofn }, options: { tabSize: 2, insertSpaces: true } })
    assert.deepEqual(formatted.result ?? [], [], JSON.stringify(formatted))
    const completion = await attempt(s, "textDocument/completion", at)
    assert.deepEqual(completion.result?.items ?? completion.result ?? [], [], JSON.stringify(completion))
    const renamed = await attempt(s, "textDocument/rename", { ...at, newName: "ex:Thing" })
    assert.match(renamed.error ?? "", /OWL Functional syntax document is not edited/, JSON.stringify(renamed))
    // From the Turtle file that uses the class: refused, the Functional
    // file named.
    const fromData = await attempt(s, "textDocument/rename", { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:A", 3), newName: "ex:Thing" })
    assert.match(fromData.error ?? "", /can't rename in .*(onto\.ofn|functional\.owl): an OWL Functional syntax document is not edited/, JSON.stringify(fromData))
    const actions = await attempt(s, "textDocument/codeAction", {
      textDocument: { uri: w.ofn },
      range: { start: { line: 3, character: 0 }, end: { line: 3, character: 20 } },
      context: { diagnostics: [] },
    })
    assert.deepEqual(actions.result ?? [], [], JSON.stringify(actions))
  })
})

describe("a closed OWL Functional file", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), initializationOptions: { diagnostics: { workspace: true } } })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("has its errors in the workspace diagnostics", async () => {
    for (const uri of [w.ofn, w.owl]) {
      const d = await s.diagnosticsFor(uri, 15000)
      assert.deepEqual(shown(d), ERRORS, uri)
    }
  })
})
