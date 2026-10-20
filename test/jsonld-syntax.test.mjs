// JSON-LD as its own syntax: `.jsonld` files and the `jsonld` language id
// reach the JSON-LD frontend, never the Turtle parser. The frontend reads
// JSON only so far, so a valid JSON-LD document has no diagnostics and a
// broken one has JSON syntax errors; plain `.json` files are not JSON-LD.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, startServer, tempDir, uriOf } from "./lsp-client.mjs"

// Turtle would report the braces, colons and commas as syntax errors.
const DOC = `{
  "@context": { "ex": "http://example.org/" },
  "@id": "ex:bob",
  "ex:name": "Bob"
}
`
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.code} ${d.message}`).join("\n")

let ws, s
before(async () => {
  ws = tempDir("turtle-jsonld-")
  fs.writeFileSync(path.join(ws, "people.jsonld"), DOC)
  fs.writeFileSync(path.join(ws, "a.ttl"), "@prefix ex: <http://example.org/> .\nex:alice ex:knows ex:bob .\n")
  s = await startServer({ cwd: ws })
  await s.request("workspace/symbol", { query: "" })
}, { timeout: 30_000 })
// Hooks get no time limit from --test-timeout; this one hung on Windows CI.
after(async () => {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
}, { timeout: 30_000 })

for (const languageId of ["jsonld", "json", "turtle"]) {
  test(`a .jsonld file opened as '${languageId}' publishes empty diagnostics`, async () => {
    const uri = uriOf(path.join(ws, `opened-as-${languageId}.jsonld`))
    s.open(uri, DOC, languageId)
    const diags = await s.diagnosticsFor(uri)
    s.close(uri)
    assert.deepEqual(diags, [], fmt(diags))
  })
}

test("the 'jsonld' language id wins over the file name", async () => {
  const uri = uriOf(path.join(ws, "untitled.txt"))
  s.open(uri, DOC, "jsonld")
  const diags = await s.diagnosticsFor(uri)
  s.close(uri)
  assert.deepEqual(diags, [], fmt(diags))
})

const formatEdits = async (text, options) => {
  const uri = uriOf(path.join(ws, "fmt.jsonld"))
  s.open(uri, text, "jsonld")
  await s.diagnosticsFor(uri)
  const edits = await s.request("textDocument/formatting", { textDocument: { uri }, options })
  s.close(uri)
  return edits ?? []
}

test("a JSON-LD document is formatted like JSON.stringify with the editor's indent", async () => {
  const edits = await formatEdits(DOC, { tabSize: 2, insertSpaces: true })
  assert.equal(edits.length, 1)
  assert.equal(edits[0].newText, JSON.stringify(JSON.parse(DOC), null, 2) + "\n")
})

test("JSON-LD formatting honours tabs and the tab size", async () => {
  const tabs = await formatEdits(DOC, { tabSize: 4, insertSpaces: false })
  assert.equal(tabs[0].newText, JSON.stringify(JSON.parse(DOC), null, "\t") + "\n")
  const four = await formatEdits(DOC, { tabSize: 4, insertSpaces: true })
  assert.equal(four[0].newText, JSON.stringify(JSON.parse(DOC), null, 4) + "\n")
})

test("a JSON-LD document with a syntax error is not formatted", async () => {
  assert.deepEqual(await formatEdits('{ "@id": "ex:a" "ex:name": "A" }', { tabSize: 2, insertSpaces: true }), [])
})

// ---- the command line ----

function run(args) {
  const r = spawnSync(serverPath, args, {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1" },
  })
  return { code: r.status, out: r.stdout, err: r.stderr }
}

test("check: a .jsonld file is checked, with no problems yet", () => {
  const all = run(["check", "--format", "json"])
  assert.equal(all.code, 0, all.err)
  const report = JSON.parse(all.out)
  assert.deepEqual(report.files, [], all.out)
  assert.equal(report.summary.filesChecked, 2, "a.ttl and people.jsonld")

  const named = run(["check", "--format", "json", "people.jsonld"])
  assert.equal(named.code, 0, named.err)
  assert.equal(named.err, "", "not skipped as a non-RDF file")
  assert.equal(JSON.parse(named.out).summary.filesChecked, 1)
})

test("check: a plain .json file is not RDF", (t) => {
  fs.writeFileSync(path.join(ws, "package.json"), "{}\n")
  t.after(() => fs.rmSync(path.join(ws, "package.json")))
  const r = run(["check", "package.json"])
  assert.match(r.err, /package\.json: not an RDF or SPARQL file, skipped/)
})

test("format --check: a .jsonld file in another layout is reported and left alone", () => {
  const r = run(["format", "--check", "people.jsonld"])
  assert.equal(r.code, 1, r.err)
  assert.match(r.out + r.err, /people\.jsonld/)
  assert.equal(fs.readFileSync(path.join(ws, "people.jsonld"), "utf8"), DOC)
})

// ---- JSON syntax errors ----

// A missing comma: one error on the value before it, as Turtle marks the
// token before a missing '.'.
const BROKEN = `{
  "@id": "ex:a"
  "ex:name": "A"
}
`

test("a JSON syntax error is published at its exact range", async () => {
  const uri = uriOf(path.join(ws, "broken-open.jsonld"))
  s.open(uri, BROKEN, "jsonld")
  const diags = await s.diagnosticsFor(uri)
  s.close(uri)
  assert.deepEqual(
    // ex: is no prefix without a context (jsonld-undefined-prefix): only the syntax error is checked here.
    diags.filter((d) => d.code === "json-syntax").map((d) => ({ range: d.range, severity: d.severity, message: d.message })),
    [
      {
        range: { start: { line: 1, character: 9 }, end: { line: 1, character: 15 } },
        severity: 1,
        message: "Expected ',' between members",
      },
    ],
    fmt(diags),
  )
})

test("check: a JSON syntax error is reported at its exact range", (t) => {
  fs.writeFileSync(path.join(ws, "broken.jsonld"), BROKEN)
  t.after(() => fs.rmSync(path.join(ws, "broken.jsonld")))
  const r = run(["check", "--format", "json", "broken.jsonld"])
  assert.equal(r.code, 1, r.err)
  const [file] = JSON.parse(r.out).files
  assert.equal(file.path, "broken.jsonld")
  assert.deepEqual(
    file.diagnostics.filter((d) => d.code === "json-syntax").map((d) => ({ range: d.range, severity: d.severity, message: d.message })),
    [
      {
        range: { start: { line: 2, column: 10 }, end: { line: 2, column: 16 } },
        severity: "error",
        message: "Expected ',' between members",
      },
    ],
  )
})
