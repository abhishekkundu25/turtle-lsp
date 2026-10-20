// Ill-typed literals (`invalid-literal`): a literal whose lexical form is not
// in its datatype's lexical space, in the editor and in `turtle-lsp check`,
// for every RDF syntax.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, startServer, tempDir, uriOf } from "./lsp-client.mjs"

const XSD = "http://www.w3.org/2001/XMLSchema#"
const invalid = (diags) => diags.filter((d) => d.code === "invalid-literal")
const at = (line, start, end) => ({ start: { line, character: start }, end: { line, character: end } })

const TTL = `@prefix ex: <http://example.org/> .
@prefix xsd: <${XSD}> .
ex:a ex:age "twelve"^^xsd:integer ;
    ex:born "2020-13-45"^^xsd:date ;
    ex:ok "12"^^xsd:integer, 12, "2020-02-29"^^xsd:date .
`
const NT = `<http://example.org/a> <http://example.org/age> "twelve"^^<${XSD}integer> .
`
const NQ = `<http://example.org/a> <http://example.org/age> "twelve"^^<${XSD}integer> <http://example.org/g> .
`
const TRIG = `@prefix ex: <http://example.org/> .
@prefix xsd: <${XSD}> .
ex:g { ex:a ex:age "twelve"^^xsd:integer }
`
const JSONLD = `{
  "@context": {
    "xsd": "${XSD}",
    "ex": "http://example.org/",
    "age": { "@id": "ex:age", "@type": "xsd:integer" }
  },
  "@id": "ex:a",
  "age": "twelve",
  "ex:born": { "@value": "2020-13-45", "@type": "xsd:date" }
}
`
const NT_RANGE = at(0, 48, 48 + `"twelve"^^<${XSD}integer>`.length)

// Each case: file name, language id, text, expected [message, range].
const CASES = [
  [
    "a.ttl",
    "turtle",
    TTL,
    [
      ["'twelve' is not a valid xsd:integer", at(2, 12, 33)],
      ["'2020-13-45' is not a valid xsd:date", at(3, 12, 34)],
    ],
  ],
  ["a.trig", "trig", TRIG, [["'twelve' is not a valid xsd:integer", at(2, 19, 40)]]],
  ["a.nt", "ntriples", NT, [["'twelve' is not a valid xsd:integer", NT_RANGE]]],
  ["a.nq", "nquads", NQ, [["'twelve' is not a valid xsd:integer", NT_RANGE]]],
  [
    "a.jsonld",
    "jsonld",
    JSONLD,
    [
      ["'twelve' is not a valid xsd:integer", at(7, 9, 17)],
      ["'2020-13-45' is not a valid xsd:date", at(8, 13, 60)],
    ],
  ],
]

describe("editor", () => {
  let ws, s
  before(async () => {
    ws = tempDir("turtle-invalid-literal-")
    s = await startServer({ cwd: ws })
  })
  after(async () => {
    try {
      await s?.stop()
    } finally {
      fs.rmSync(ws, { recursive: true, force: true })
    }
  })

  for (const [name, lang, text, expected] of CASES) {
    test(`${lang}: a warning at each ill-typed literal, naming its datatype`, async () => {
      const uri = uriOf(path.join(ws, name))
      s.open(uri, text, lang)
      const diags = invalid(await s.diagnosticsFor(uri))
      assert.deepEqual(
        diags.map((d) => [d.message, d.range]),
        expected,
        JSON.stringify(diags),
      )
      for (const d of diags) assert.equal(d.severity, 2)
      s.close(uri)
    })
  }

  test("a fixed literal clears its warning", async () => {
    const uri = uriOf(path.join(ws, "fix.ttl"))
    s.open(uri, TTL)
    assert.equal(invalid(await s.diagnosticsFor(uri)).length, 2)
    s.change(uri, TTL.replace('"twelve"', '"12"').replace("2020-13-45", "2020-12-25"))
    assert.deepEqual(invalid(await s.diagnosticsFor(uri)), [])
    s.close(uri)
  })

  test("rdf:XMLLiteral that is not well-balanced XML", { todo: "invalid-literal:xml-literal" }, async () => {
    const uri = uriOf(path.join(ws, "xml.ttl"))
    s.open(
      uri,
      `@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .\n<http://example.org/a> <http://example.org/p> "<b>bold</i>"^^rdf:XMLLiteral .\n`,
    )
    assert.equal(invalid(await s.diagnosticsFor(uri)).length, 1)
    s.close(uri)
  })
})

test("check: reports each ill-typed literal with its code and range, as a warning", (t) => {
  const ws = tempDir("turtle-invalid-literal-cli-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  fs.mkdirSync(path.join(ws, ".cache"))
  for (const [name, , text] of CASES) fs.writeFileSync(path.join(ws, name), text)
  const r = spawnSync(serverPath, ["check", "--format", "json"], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1" },
  })
  // Warnings only: the check passes.
  assert.equal(r.status, 0, r.stderr)
  const files = JSON.parse(r.stdout).files
  for (const [name, , , expected] of CASES) {
    const file = files.find((f) => f.path === name)
    assert.ok(file, `${name} in ${r.stdout}`)
    const got = file.diagnostics.filter((d) => d.code === "invalid-literal")
    // `check` positions are 1-based lines and columns.
    assert.deepEqual(
      got.map((d) => [d.message, d.severity, d.range]),
      expected.map(([m, r]) => [
        m,
        "warning",
        { start: { line: r.start.line + 1, column: r.start.character + 1 }, end: { line: r.end.line + 1, column: r.end.character + 1 } },
      ]),
      name,
    )
  }
})
