// An OWL Functional file (.ofn, or a .owl whose text is Functional) open in
// an editor, over stdio: the server reports its errors at their spans, with
// the code ofn-syntax, as it publishes, as it is pulled and after an edit,
// and reads nothing else in it (no hover, symbols or tokens; its axioms are
// not read into the workspace yet). Nothing writes it: a client that opts
// in to the dynamic registrations gets none of the writing features for
// it, and a client that does not gets empty answers.
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
  fs.writeFileSync(path.join(dir, "data.ttl"), "@prefix ex: <http://example.org/o#> .\nex:a a ex:A .\n")
  return { dir, ofn: uriOf(path.join(dir, "onto.ofn")), owl: uriOf(path.join(dir, "functional.owl")) }
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

  test("nothing in it is read: no hover, symbols, tokens or folding", async () => {
    const at = { textDocument: { uri: w.ofn }, position: positionOf(ONTO, "Class(:A", 7) }
    assert.equal(await s.request("textDocument/hover", at), null)
    const symbols = await s.request("textDocument/documentSymbol", { textDocument: { uri: w.ofn } })
    assert.deepEqual(symbols ?? [], [])
    const tokens = await s.request("textDocument/semanticTokens/full", { textDocument: { uri: w.ofn } })
    assert.deepEqual(tokens?.data ?? [], [])
    const folds = await s.request("textDocument/foldingRange", { textDocument: { uri: w.ofn } })
    assert.deepEqual(folds ?? [], [])
    const definition = await s.request("textDocument/definition", at)
    assert.deepEqual([definition ?? []].flat(), [])
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
