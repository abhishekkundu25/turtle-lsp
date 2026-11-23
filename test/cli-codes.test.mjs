// The command line: `turtle-lsp codes` and `turtle-lsp explain`, run as a
// user runs them. Only stdout, stderr and the exit code are asserted.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import { serverPath } from "./lsp-client.mjs"

function run(args) {
  const r = spawnSync(serverPath, args, { encoding: "utf8", timeout: 60000, env: { ...process.env, NO_COLOR: "1" } })
  return { code: r.status, out: r.stdout, err: r.stderr, lines: r.stdout.split("\n").filter(Boolean) }
}

// The off-by-default codes: the house-style checks.
const OPT_IN = ["missing-label", "missing-definition", "missing-ontology-license"]

test("codes: one line per code, with on or off, the same codes as --format json", () => {
  const r = run(["codes"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.err, "")
  const rows = r.lines.map((l) => l.split(/\s+/))
  for (const [code, onOff] of rows) assert.ok(onOff === "on" || onOff === "off", `${code} ${onOff}`)
  const byCode = Object.fromEntries(rows.map(([code, onOff]) => [code, onOff]))
  assert.equal(byCode["syntax"], "on")
  assert.equal(byCode["undeclared-prefix"], "on")
  for (const c of OPT_IN) assert.equal(byCode[c], "off", c)
  assert.match(r.lines.find((l) => l.startsWith("undeclared-prefix ")), /A prefix is used but not declared$/)

  const j = run(["codes", "--format", "json"])
  assert.equal(j.code, 0, j.err)
  assert.equal(j.lines.length, 1)
  const v = JSON.parse(j.out)
  assert.equal(v.version, 1)
  assert.deepEqual(v.codes.map((c) => c.code), rows.map(([code]) => code))
  for (const c of v.codes) {
    assert.deepEqual(Object.keys(c).sort(), ["code", "default", "description"])
    assert.equal(c.default, byCode[c.code])
    assert.ok(c.description.length > 0)
  }
  // Every code `codes` lists is one `check --select` takes.
  const all = v.codes.map((c) => c.code).join(",")
  assert.equal(run(["check", "--select", all, "--help"]).code, 0)

  assert.equal(run(["codes", "--format=json"]).out, j.out)
  assert.equal(run(["codes", "--format=text"]).out, r.out)
})

test("explain: an on-by-default code, its description and how to turn it off", () => {
  const r = run(["explain", "undeclared-prefix"])
  assert.equal(r.code, 0, r.err)
  assert.match(r.out, /^undeclared-prefix: A prefix is used but not declared$/m)
  assert.match(r.out, /^Default: on$/m)
  assert.match(r.out, /^Turn off: .*"lint\.ignore".*--ignore undeclared-prefix/m)
  assert.match(r.out, /^Show only it: turtle-lsp check --select undeclared-prefix$/m)
})

test("explain: an off-by-default code says so and how to turn it on with lint.enable", () => {
  const r = run(["explain", "missing-label"])
  assert.equal(r.code, 0, r.err)
  assert.match(r.out, /^Default: off$/m)
  assert.match(r.out, /^Turn on: Add "missing-label" to "lint\.enable" in turtle-lsp\.json at the workspace root: /m)
  // The whole file, so an agent can write it as it is.
  assert.ok(r.out.includes('{"lint": {"enable": ["missing-label"]}}'), r.out)
})

test("explain --format json: one codes entry plus enable and disable", () => {
  const codes = JSON.parse(run(["codes", "--format", "json"]).out).codes
  for (const code of ["undeclared-prefix", "missing-label"]) {
    const r = run(["explain", code, "--format=json"])
    assert.equal(r.code, 0, r.err)
    assert.equal(r.lines.length, 1)
    const v = JSON.parse(r.out)
    const { enable, disable, ...entry } = v
    assert.deepEqual(entry, codes.find((c) => c.code === code))
    assert.equal(typeof enable, "string")
    assert.equal(typeof disable, "string")
    assert.match(disable, /lint\.ignore/)
  }
  const v = JSON.parse(run(["explain", "--format", "json", "missing-label"]).out)
  assert.match(v.enable, /lint\.enable/)
})

test("explain: a misspelt code exits 2 and names the closest known code", () => {
  const r = run(["explain", "undeclard-prefix"])
  assert.equal(r.code, 2)
  assert.equal(r.out, "")
  assert.match(r.err, /unknown diagnostic code 'undeclard-prefix'; did you mean 'undeclared-prefix'\?/)
})

test("explain with no code, a second code, and bad options exit 2", () => {
  const none = run(["explain"])
  assert.equal(none.code, 2)
  assert.match(none.err, /explain needs a diagnostic code/)
  assert.match(none.err, /Usage: turtle-lsp explain/)
  assert.equal(run(["explain", "syntax", "io"]).code, 2)
  for (const args of [["codes", "--format", "xml"], ["explain", "syntax", "--format", "xml"], ["codes", "--fix"], ["codes", "syntax"]]) {
    const r = run(args)
    assert.equal(r.code, 2, args.join(" "))
    assert.equal(r.out, "", args.join(" "))
    assert.match(r.err, /^turtle-lsp: /, args.join(" "))
  }
})

test("help lists codes and explain; each prints its usage", () => {
  const help = run(["help"])
  assert.equal(help.code, 0)
  assert.match(help.out, /^ {2}turtle-lsp codes /m)
  assert.match(help.out, /^ {2}turtle-lsp explain CODE /m)
  for (const [command, args] of [["codes", ["help", "codes"]], ["codes", ["codes", "--help"]], ["explain", ["help", "explain"]], ["explain", ["explain", "--help"]], ["explain", ["explain", "syntax", "-h"]]]) {
    const r = run(args)
    assert.equal(r.code, 0, args.join(" "))
    assert.match(r.out, new RegExp(`^Usage: turtle-lsp ${command} `), args.join(" "))
    assert.match(r.out, /--format F/, args.join(" "))
  }
})

test("explain: the RDF/XML, OWL Functional and OWL/XML codes are known codes, on by default, in codes, explain and --format json", () => {
  const listed = run(["codes"]).lines
  const codes = JSON.parse(run(["codes", "--format", "json"]).out).codes
  for (const [code, description] of [
    ["xml-syntax", /^xml-syntax: The XML is malformed/m],
    ["rdfxml-syntax", /^rdfxml-syntax: The XML is not valid RDF\/XML$/m],
    ["ofn-syntax", /^ofn-syntax: The OWL Functional syntax is malformed, or breaks a rule of the OWL 2 structural specification; a warning for a form read as OWL API reads it \(an rdf:PlainLiteral with no @\)$/m],
    ["owx-syntax", /^owx-syntax: The OWL\/XML is malformed, or breaks a rule of the OWL 2 XML Serialization or the structural specification$/m],
    ["unsupported-syntax", /^unsupported-syntax: The file is in an OWL syntax that is not read \(Manchester\): an error when an import is read from it, else a warning$/m],
  ]) {
    const r = run(["explain", code])
    assert.equal(r.code, 0, r.err)
    assert.match(r.out, description)
    assert.match(r.out, /^Default: on$/m)
    assert.match(listed.find((l) => l.startsWith(`${code} `)) ?? "", /^\S+\s+on\s/, code)
    const entry = codes.find((c) => c.code === code)
    assert.equal(entry?.default, "on", code)
    const j = run(["explain", code, "--format", "json"])
    assert.equal(j.code, 0, j.err)
    const { enable, disable, ...rest } = JSON.parse(j.out)
    assert.deepEqual(rest, entry, code)
    assert.match(disable, /lint\.ignore/, code)
  }
})
