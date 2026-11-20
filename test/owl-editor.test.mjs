// An RDF/XML file (.owl, .rdf) open in an editor: the client attaches to it
// with XML's language id. It reads like any RDF document (hover, definition,
// references, symbols, diagnostics), and only rename writes it: a rename of
// an IRI it uses edits it too, and one can start in it (`rename.rdfXml` off:
// neither). A client that opts in (`registerOutsideRdfXml` in
// initializationOptions, as VS Code does) and registers formatting, rename,
// completion and code actions itself gets formatting, completion and code
// actions for the other documents only, and rename for these files too, so
// the editor offers no other writer in them. Every other client gets the
// static capabilities, as before, and empty answers here but for rename. A
// client that takes change annotations gets the RDF/XML edits marked to be
// confirmed.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, positionOf, tempDir } from "./lsp-client.mjs"
import { applyEdits } from "./jsonld-fixtures.mjs"

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
// What VS Code's client declares for workspace edits.
const ANNOTATIONS = { workspaceEdit: { documentChanges: true, changeAnnotationSupport: { groupsOnLabel: true } } }
// The two files after renaming ex:Widget to ex:Thing.
const ONTO_RENAMED = ONTO.replaceAll("http://example.org/Widget", "http://example.org/Thing")
const DATA_RENAMED = DATA.replace("ex:Widget", "ex:Thing")

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
    const capabilities = { ...DYNAMIC, workspace: { ...DYNAMIC.workspace, ...ANNOTATIONS } }
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities, initializationOptions: OPT_IN })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("formatting, rename, completion and code actions are registered for the documents the server writes, not announced; rename for RDF/XML too", async () => {
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
      const rename = r.method === "textDocument/rename"
      assert.deepEqual(
        selector.map((f) => f.language ?? f.pattern),
        ["turtle", "trig", "ntriples", "nquads", "sparql", "jsonld", "**/*.jsonld", ...(rename ? ["**/*.{owl,rdf}"] : [])],
        r.method,
      )
      if (!rename) assert.doesNotMatch(JSON.stringify(selector), /owl|rdf|xml/, r.method)
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

  test("the server formats nothing in it, asked anyway; a rename edits it, each RDF/XML edit to be confirmed", async () => {
    const at = { textDocument: { uri: w.onto }, position: positionOf(ONTO, 'about="http://example.org/Widg') }
    const formatted = await attempt(s, "textDocument/formatting", { textDocument: { uri: w.onto }, options: { tabSize: 2, insertSpaces: true } })
    assert.deepEqual(formatted.result ?? [], [], JSON.stringify(formatted))
    // The whole value, from inside the .owl.
    const prepared = await s.request("textDocument/prepareRename", at)
    const line = ONTO.split("\n").findIndex((l) => l.includes("http://example.org/Widget"))
    const start = ONTO.split("\n")[line].indexOf("http://")
    assert.deepEqual(prepared, { start: { line, character: start }, end: { line, character: start + "http://example.org/Widget".length } })
    for (const [from, where] of [[w.onto, at], [w.data, { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:Widg") }]]) {
      const edit = await until(
        () => attempt(s, "textDocument/rename", { ...where, newName: from === w.onto ? "Thing" : "ex:Thing" }),
        (r) => r.result?.documentChanges?.length === 2,
      )
      const { documentChanges, changeAnnotations, changes } = edit.result ?? {}
      assert.equal(changes, undefined, JSON.stringify(edit))
      const [id] = Object.keys(changeAnnotations ?? {})
      assert.deepEqual(changeAnnotations[id], {
        label: "Rename in RDF/XML",
        needsConfirmation: true,
        description: "Edits RDF/XML documents (.owl, .rdf): check them before they are applied",
      })
      const byUri = Object.fromEntries(documentChanges.map((d) => [d.textDocument.uri, d.edits]))
      assert.ok(byUri[w.onto].every((e) => e.annotationId === id), JSON.stringify(byUri[w.onto]))
      assert.ok(byUri[w.data].every((e) => e.annotationId === undefined), JSON.stringify(byUri[w.data]))
      assert.equal(applyEdits(ONTO, byUri[w.onto]), ONTO_RENAMED, from)
      assert.equal(applyEdits(DATA, byUri[w.data]), DATA_RENAMED, from)
    }
    // A broken RDF/XML file that uses the IRI refuses the whole rename,
    // with its file and line.
    const broken = { textDocument: { uri: w.broken }, position: positionOf(BROKEN, 'about="http://example.org/') }
    const refused = await attempt(s, "textDocument/rename", { ...broken, newName: "y" })
    assert.match(refused.error ?? "", /^can't rename in .*broken\.rdf: line \d+: the document has a syntax error/, JSON.stringify(refused))
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

  test("gets the static capabilities, no registration, and nothing written in a .owl but by a rename, with plain edits", async () => {
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
    assert.ok(prepared.result?.start, JSON.stringify(prepared))
    // No change annotations without the client's support.
    const renamed = await attempt(s, "textDocument/rename", { ...at, newName: "<http://example.org/Thing>" })
    assert.equal(renamed.result?.documentChanges, undefined, JSON.stringify(renamed))
    assert.equal(renamed.result?.changeAnnotations, undefined, JSON.stringify(renamed))
    assert.equal(applyEdits(ONTO, renamed.result.changes[w.onto]), ONTO_RENAMED)
    const actions = await attempt(s, "textDocument/codeAction", { ...at, range: { start: at.position, end: at.position }, context: { diagnostics: [] } })
    const edits = (actions.result ?? []).filter((a) => a.edit)
    assert.deepEqual(edits, [], JSON.stringify(actions))
    const completion = await attempt(s, "textDocument/completion", at)
    const items = Array.isArray(completion.result) ? completion.result : (completion.result?.items ?? [])
    assert.deepEqual(items, [], JSON.stringify(completion).slice(0, 300))
  })
})

describe("a .owl that is not open", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), capabilities: { workspace: ANNOTATIONS } })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("is edited from the index by a rename in a Turtle file, with its edits to be confirmed", async () => {
    s.open(w.data, DATA)
    const inData = { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:Widg") }
    const edit = await until(
      () => attempt(s, "textDocument/rename", { ...inData, newName: "ex:Thing" }),
      (r) => r.result?.documentChanges?.length === 2,
    )
    const byUri = Object.fromEntries((edit.result?.documentChanges ?? []).map((d) => [d.textDocument.uri, d.edits]))
    assert.ok(byUri[w.onto]?.every((e) => e.annotationId), JSON.stringify(edit))
    assert.equal(applyEdits(ONTO, byUri[w.onto]), ONTO_RENAMED)
    assert.equal(applyEdits(DATA, byUri[w.data]), DATA_RENAMED)
  })
})

describe("with rename.rdfXml off", () => {
  let w, s
  before(async () => {
    w = workspace()
    s = await startServer({ cwd: w.dir, rootUri: uriOf(w.dir), initializationOptions: { "rename.rdfXml": false } })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(w.dir, { recursive: true, force: true })
  })

  test("nothing is renamed in a .owl, nor in a Turtle file that shares the IRI with one", async () => {
    s.open(w.onto, ONTO, "xml")
    s.open(w.data, DATA)
    const at = { textDocument: { uri: w.onto }, position: positionOf(ONTO, 'about="http://example.org/Widg') }
    await until(() => s.request("textDocument/hover", at), (h) => contents(h).includes("A thing with parts."))
    // Nothing to rename here; rename itself says why.
    assert.deepEqual(await attempt(s, "textDocument/prepareRename", at), { result: null })
    const renamed = await attempt(s, "textDocument/rename", { ...at, newName: "Thing" })
    assert.match(renamed.error ?? "", /RDF\/XML document is not edited/, JSON.stringify(renamed))
    const inData = { textDocument: { uri: w.data }, position: positionOf(DATA, "ex:Widg") }
    const fromData = await until(
      () => attempt(s, "textDocument/rename", { ...inData, newName: "ex:Thing" }),
      (r) => Boolean(r.error),
    )
    assert.match(fromData.error ?? "", /onto\.owl: an RDF\/XML document is not edited/, JSON.stringify(fromData))
  })
})
