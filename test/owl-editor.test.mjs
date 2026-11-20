// An RDF/XML file (.owl, .rdf) open in an editor: the client attaches to it
// with XML's language id. It reads like any RDF document (hover, definition,
// references, symbols, diagnostics), and nothing writes it. A client that
// opts in (`registerOutsideRdfXml` in initializationOptions, as VS Code does)
// and registers formatting, rename, completion and code actions itself gets
// them for the documents the server writes only, so the editor offers none
// of them in these files. Every other client gets the static capabilities,
// as before, and empty answers here.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"

const ONTO = `<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:owl="http://www.w3.org/2002/07/owl#">
  <owl:Class rdf:about="http://example.org/Widget">
    <rdfs:label>Widget</rdfs:label>
    <rdfs:comment>A thing with parts.</rdfs:comment>
  </owl:Class>
  <owl:Class rdf:about="http://example.org/Gadget">
    <rdfs:subClassOf rdf:resource="http://example.org/Widget"/>
  </owl:Class>
</rdf:RDF>
`
const DATA = `@prefix ex: <http://example.org/> .

ex:w1 a ex:Widget .
`
const BROKEN = `<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="http://example.org/x">
</rdf:RDF>
`
const FEATURES = ["textDocument/formatting", "textDocument/rename", "textDocument/completion", "textDocument/codeAction"]
const PROVIDERS = ["documentFormattingProvider", "renameProvider", "completionProvider", "codeActionProvider"]
const dynamic = { dynamicRegistration: true }
const DYNAMIC = {
  textDocument: { formatting: dynamic, rename: dynamic, completion: dynamic, codeAction: dynamic },
  workspace: { didChangeWatchedFiles: dynamic },
}
const OPT_IN = { registerOutsideRdfXml: true }

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const contents = (h) => JSON.stringify(h?.contents ?? null)
const targets = (r) => [r ?? []].flat().map((l) => l.targetUri ?? l.uri)

/** A workspace with the ontology, a Turtle file that uses it, and a broken RDF/XML file. */
function workspace() {
  const dir = tempDir("turtle-owl-editor-")
  fs.writeFileSync(path.join(dir, "onto.owl"), ONTO)
  fs.writeFileSync(path.join(dir, "data.ttl"), DATA)
  fs.writeFileSync(path.join(dir, "broken.rdf"), BROKEN)
  return {
    dir,
    onto: uriOf(path.join(dir, "onto.owl")),
    data: uriOf(path.join(dir, "data.ttl")),
    broken: uriOf(path.join(dir, "broken.rdf")),
  }
}

/** Asks until `done(answer)` holds: the workspace index lands in the background. */
async function until(ask, done, ms = 10000) {
  const end = Date.now() + ms
  for (;;) {
    const answer = await ask()
    if (done(answer) || Date.now() > end) return answer
    await sleep(50)
  }
}

/** The answer to a request, or its error message. */
const attempt = (s, method, params) => s.request(method, params).then((result) => ({ result }), (e) => ({ error: e.message }))

describe("a client that opts in and registers features dynamically", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities: DYNAMIC, initializationOptions: OPT_IN })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("formatting, rename, completion and code actions are registered for the documents the server writes, not announced", async () => {
    for (const p of PROVIDERS) assert.equal(s.init.capabilities[p], undefined, p)
    // What reads stays static, and applies to .owl too.
    for (const p of ["hoverProvider", "definitionProvider", "referencesProvider", "documentSymbolProvider", "foldingRangeProvider"]) {
      assert.ok(s.init.capabilities[p], p)
    }
    const registrations = await until(
      async () => s.serverRequests.filter((r) => r.method === "client/registerCapability").flatMap((r) => r.params.registrations),
      (regs) => regs.length >= 5,
    )
    // The watchers have a request of their own, and watch .owl and .rdf files.
    const watch = registrations.find((r) => r.method === "workspace/didChangeWatchedFiles")
    const glob = watch.registerOptions.watchers[0].globPattern
    for (const ext of ["owl", "rdf"]) assert.ok(glob.includes(`*.${ext}`), glob)
    const features = registrations.filter((r) => r.method !== "workspace/didChangeWatchedFiles")
    assert.deepEqual(features.map((r) => r.method).sort(), [...FEATURES].sort())
    for (const r of features) {
      const selector = r.registerOptions.documentSelector
      assert.deepEqual(
        selector.map((f) => f.language ?? f.pattern),
        ["turtle", "trig", "ntriples", "nquads", "sparql", "jsonld", "**/*.jsonld"],
        r.method,
      )
      assert.doesNotMatch(JSON.stringify(selector), /owl|rdf|xml/, r.method)
    }
    const options = Object.fromEntries(features.map((r) => [r.method, r.registerOptions]))
    assert.equal(options["textDocument/rename"].prepareProvider, true)
    assert.equal(options["textDocument/completion"].resolveProvider, true)
    assert.deepEqual(options["textDocument/completion"].triggerCharacters, [":", "@", "?", "$"])
    assert.equal(options["textDocument/codeAction"].resolveProvider, true)
    assert.ok(options["textDocument/codeAction"].codeActionKinds.includes("quickfix"))
  })

  test("an open .owl (language id xml): hover, definition, references, symbols and diagnostics", async () => {
    s.open(w.onto, ONTO, "xml")
    assert.ok(Array.isArray(await s.diagnosticsFor(w.onto, 10000)))
    // Hover on the class's IRI: its comment.
    const onWidget = { textDocument: { uri: w.onto }, position: positionOf(ONTO, 'about="http://example.org/Widg') }
    const hover = await until(() => s.request("textDocument/hover", onWidget), (h) => contents(h).includes("A thing with parts."))
    assert.match(contents(hover), /A thing with parts\./)
    // From the Turtle file, the definition is in the .owl.
    const inData = { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:Widg") }
    s.open(w.data, DATA)
    const definition = await until(() => s.request("textDocument/definition", inData), (r) => targets(r).includes(w.onto))
    assert.deepEqual(targets(definition), [w.onto])
    // The references of ex:Widget reach both files.
    const references = await until(
      () => s.request("textDocument/references", { ...onWidget, context: { includeDeclaration: true } }),
      (r) => new Set(targets(r)).size === 2,
    )
    assert.deepEqual([...new Set(targets(references))].sort(), [w.data, w.onto].sort())
    // The outline names both classes.
    const symbols = await s.request("textDocument/documentSymbol", { textDocument: { uri: w.onto } })
    const names = JSON.stringify(symbols)
    for (const name of ["Widget", "Gadget"]) assert.ok(names.includes(name), names)
    // No semantic tokens: VS Code's XML grammar keeps the colours.
    const tokens = await s.request("textDocument/semanticTokens/full", { textDocument: { uri: w.onto } })
    assert.deepEqual(tokens?.data ?? [], [])
    // Broken XML is a problem of the open file.
    s.open(w.broken, BROKEN, "xml")
    const errors = (await s.diagnosticsFor(w.broken, 10000)).filter((d) => d.severity === 1)
    assert.ok(errors.length > 0, "no error for broken XML")
  })

  test("the server writes nothing in it, asked anyway", async () => {
    const at = { textDocument: { uri: w.onto }, position: positionOf(ONTO, 'about="http://example.org/Widg') }
    const formatted = await attempt(s, "textDocument/formatting", { textDocument: { uri: w.onto }, options: { tabSize: 2, insertSpaces: true } })
    assert.deepEqual(formatted.result ?? [], [], JSON.stringify(formatted))
    const renamed = await attempt(s, "textDocument/rename", { ...at, newName: "ex:Thing" })
    assert.match(renamed.error ?? "", /RDF\/XML document is not edited/, JSON.stringify(renamed))
  })
})

describe("a client that registers features dynamically but does not opt in", () => {
  let w, s, plain
  before(async () => {
    w = workspace()
    // Its settings in initializationOptions, without the opt-in.
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities: DYNAMIC, initializationOptions: { "inlayHints.labels": "all" } })
    plain = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir) })
  })
  after(async () => {
    await s.stop()
    await plain.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("gets the static capabilities of a client that cannot, and no registration of them", async () => {
    assert.deepEqual(s.init.capabilities, plain.init.capabilities)
    assert.equal(s.init.capabilities.documentFormattingProvider, true)
    assert.deepEqual(s.init.capabilities.renameProvider, { prepareProvider: true })
    assert.deepEqual(s.init.capabilities.completionProvider, { triggerCharacters: [":", "@", "?", "$"], resolveProvider: true })
    assert.equal(s.init.capabilities.codeActionProvider.resolveProvider, true)
    assert.deepEqual([...s.init.capabilities.codeActionProvider.codeActionKinds].sort(), ["quickfix", "refactor.rewrite", "source.organizeImports"])
    // The watchers are registered; then, after a round trip, nothing else is.
    const registrations = () => s.serverRequests.filter((r) => r.method === "client/registerCapability").flatMap((r) => r.params.registrations)
    await until(async () => registrations(), (regs) => regs.length >= 1)
    s.open(w.data, DATA)
    await s.request("textDocument/hover", { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:Widg") })
    assert.deepEqual(registrations().map((r) => r.method), ["workspace/didChangeWatchedFiles"])
  })
})

describe("a client that does not register features dynamically", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir) })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("gets the static capabilities, no registration, and nothing written in a .owl", async () => {
    for (const p of PROVIDERS) assert.ok(s.init.capabilities[p], p)
    s.open(w.onto, ONTO, "xml")
    await s.diagnosticsFor(w.onto, 10000)
    assert.deepEqual(s.serverRequests.filter((r) => r.method === "client/registerCapability"), [])
    const at = { textDocument: { uri: w.onto }, position: positionOf(ONTO, 'about="http://example.org/Widg') }
    // Read first, so that what follows is the gate at work, not a document not yet analysed.
    await until(() => s.request("textDocument/hover", at), (h) => contents(h).includes("A thing with parts."))
    const formatted = await attempt(s, "textDocument/formatting", { textDocument: { uri: w.onto }, options: { tabSize: 2, insertSpaces: true } })
    assert.deepEqual(formatted.result ?? [], [], JSON.stringify(formatted))
    const prepared = await attempt(s, "textDocument/prepareRename", at)
    // Nothing to rename here; rename itself says why.
    assert.deepEqual(prepared, { result: null })
    const renamed = await attempt(s, "textDocument/rename", { ...at, newName: "ex:Thing" })
    assert.match(renamed.error ?? "", /RDF\/XML document is not edited/, JSON.stringify(renamed))
    const actions = await attempt(s, "textDocument/codeAction", { ...at, range: { start: at.position, end: at.position }, context: { diagnostics: [] } })
    const edits = (actions.result ?? []).filter((a) => a.edit)
    assert.deepEqual(edits, [], JSON.stringify(actions))
    const completion = await attempt(s, "textDocument/completion", at)
    const items = Array.isArray(completion.result) ? completion.result : (completion.result?.items ?? [])
    assert.deepEqual(items, [], JSON.stringify(completion).slice(0, 300))
  })
})
