// JSON-LD as its own syntax: `.jsonld` files and the `jsonld` language id
// reach the JSON-LD frontend, never the Turtle parser. The frontend is still
// empty, so a JSON-LD document has no diagnostics yet; plain `.json` files
// are not JSON-LD.
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
})
after(async () => {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

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

test("a JSON-LD document is not formatted", async () => {
  const uri = uriOf(path.join(ws, "people.jsonld"))
  s.open(uri, DOC, "jsonld")
  await s.diagnosticsFor(uri)
  const edits = await s.request("textDocument/formatting", {
    textDocument: { uri },
    options: { tabSize: 2, insertSpaces: true },
  })
  s.close(uri)
  assert.deepEqual(edits ?? [], [])
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

test("format --check: a .jsonld file is left alone", () => {
  const r = run(["format", "--check", "people.jsonld"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, "")
  assert.equal(fs.readFileSync(path.join(ws, "people.jsonld"), "utf8"), DOC)
})
