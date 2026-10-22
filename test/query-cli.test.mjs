// The command line: `turtle-lsp query`, run as a user runs it, on folders
// written by each test. Only stdout, stderr and the exit code are asserted.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, tempDir } from "./lsp-client.mjs"

// The project's prefixes; the data files below declare none.
const PREFIXES = "@prefix ex: <http://example.org/> .\n"
const FILES = {
  ".git/HEAD": "",
  "prefixes.ttl": PREFIXES,
  "data/bob.ttl": 'ex:bob a ex:Person ;\n    ex:name "Bob" .\n',
  "alice.ttl": 'ex:alice a ex:Person ;\n    ex:name "Alice, A" .\n',
  "names.rq": "SELECT ?s ?name WHERE { ?s ex:name ?name } ORDER BY ?name\n",
}

// A workspace with `files` ({relative path: text}); removed after the test.
function workspace(t, files = FILES) {
  const ws = tempDir("turtle-query-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  fs.mkdirSync(path.join(ws, ".cache"))
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
    fs.writeFileSync(path.join(ws, name), text)
  }
  return ws
}

// Runs `turtle-lsp query ARGS` in the workspace with a private import cache.
function query(ws, args) {
  const r = spawnSync(serverPath, ["query", ...args], {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1" },
  })
  return { code: r.status, out: r.stdout, err: r.stderr }
}

test("query: SELECT prints an aligned table with the project's prefixes", (t) => {
  const r = query(workspace(t), ["names.rq"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.err, "")
  assert.equal(
    r.out,
    '?s       | ?name\n---------+-----------\nex:alice | "Alice, A"\nex:bob   | "Bob"\n',
  )
})

test("query: --limit cuts the table and says so", (t) => {
  const r = query(workspace(t), ["names.rq", "--limit", "1"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out.trimEnd().split("\n").at(-1), "showing 1 of 2 rows")
  assert.equal(r.out.split("\n").length, 5)
  const bad = query(workspace(t), ["names.rq", "--limit", "1", "--format", "csv"])
  assert.equal(bad.code, 2)
  assert.match(bad.err, /--limit works only with the table format/)
})

test("query: SELECT as SPARQL JSON", (t) => {
  const r = query(workspace(t), ["names.rq", "--format", "json"])
  assert.equal(r.code, 0, r.err)
  const j = JSON.parse(r.out)
  assert.deepEqual(j.head.vars, ["s", "name"])
  assert.deepEqual(j.results.bindings[0], {
    s: { type: "uri", value: "http://example.org/alice" },
    name: { type: "literal", value: "Alice, A" },
  })
  assert.equal(j.results.bindings.length, 2)
})

test("query: SELECT as CSV and TSV", (t) => {
  const ws = workspace(t)
  const csv = query(ws, ["names.rq", "--format=csv"])
  assert.equal(csv.code, 0, csv.err)
  assert.equal(csv.out, 's,name\r\nhttp://example.org/alice,"Alice, A"\r\nhttp://example.org/bob,Bob\r\n')
  const tsv = query(ws, ["names.rq", "--format", "tsv"])
  assert.equal(tsv.code, 0, tsv.err)
  assert.equal(tsv.out, '?s\t?name\n<http://example.org/alice>\t"Alice, A"\n<http://example.org/bob>\t"Bob"\n')
})

test("query: SELECT as Turtle is an error", (t) => {
  const r = query(workspace(t), ["names.rq", "--format", "turtle"])
  assert.equal(r.code, 2)
  assert.equal(r.out, "")
  assert.match(r.err, /--format turtle does not fit a SELECT query/)
})

test("query: ASK prints true (exit 0) or false (exit 1)", (t) => {
  const ws = workspace(t)
  const yes = query(ws, ["-e", "ASK { ex:bob a ex:Person }"])
  assert.deepEqual([yes.code, yes.out], [0, "true\n"], yes.err)
  const no = query(ws, ["-e", "ASK { ex:carol a ex:Person }"])
  assert.deepEqual([no.code, no.out], [1, "false\n"], no.err)
  const json = query(ws, ["-e", "ASK { ex:carol a ex:Person }", "--format", "json"])
  assert.equal(json.code, 1, json.err)
  assert.deepEqual(JSON.parse(json.out), { head: {}, boolean: false })
  const csv = query(ws, ["-e", "ASK {}", "--format", "csv"])
  assert.equal(csv.code, 2)
  assert.match(csv.err, /does not fit an ASK query/)
})

test("query: CONSTRUCT prints Turtle with the prefixes it uses", (t) => {
  const ws = workspace(t)
  const r = query(ws, ["-e", "CONSTRUCT { ?s ex:knows ex:carol } WHERE { ?s a ex:Person }"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, "@prefix ex: <http://example.org/> .\n\nex:alice ex:knows ex:carol .\n\nex:bob ex:knows ex:carol .\n")
  const table = query(ws, ["-e", "DESCRIBE ex:bob", "--format", "table"])
  assert.equal(table.code, 2)
  assert.match(table.err, /does not fit a CONSTRUCT or DESCRIBE query; use turtle/)
})

test("query: paths choose the data; the query file is not data", (t) => {
  const ws = workspace(t)
  const r = query(ws, ["names.rq", "data", "--format", "csv"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, "s,name\r\nhttp://example.org/bob,Bob\r\n")
  const one = query(ws, ["-e", "SELECT ?s WHERE { ?s a ex:Person }", "alice.ttl", "--format", "csv"])
  assert.equal(one.out, "s\r\nhttp://example.org/alice\r\n")
  const missing = query(ws, ["names.rq", "nope"])
  assert.equal(missing.code, 2)
  assert.match(missing.err, /nope: no such file or directory/)
})

test("query: prefixes.ttl applies to the query and to headerless data in a subfolder", (t) => {
  const ws = workspace(t, {
    ".git/HEAD": "",
    "prefixes.ttl": "@prefix ex: <http://example.org/> .\n@prefix my: <http://my.example/> .\n",
    "sub/deep/data.ttl": "my:x ex:p my:y .\n",
    "sub/q.rq": "SELECT ?o WHERE { my:x ex:p ?o }\n",
  })
  const r = query(ws, ["sub/q.rq", "--format", "csv"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, "o\r\nhttp://my.example/y\r\n")
  const table = query(ws, ["sub/q.rq"])
  assert.match(table.out, /^my:y$/m)
})

test("query: a syntax error prints file:line:col and exits 2", (t) => {
  const ws = workspace(t, { ...FILES, "bad.rq": "SELECT * WHERE {\n  ?s ?p\n}\n" })
  const r = query(ws, ["bad.rq"])
  assert.equal(r.code, 2)
  assert.equal(r.out, "")
  assert.match(r.err, /^bad\.rq:3:1: /)
  const inline = query(ws, ["-e", "SELECT ?x WHERE { ?x ?y }"])
  assert.equal(inline.code, 2)
  assert.match(inline.err, /^-e:1:25: /)
})

test("query: an UPDATE is not run", (t) => {
  const r = query(workspace(t), ["-e", "INSERT DATA { ex:a ex:b ex:c }"])
  assert.equal(r.code, 2)
  assert.equal(r.out, "")
  assert.match(r.err, /SPARQL UPDATE is not supported/)
})

test("query: bad flags and help", (t) => {
  const ws = workspace(t)
  assert.equal(query(ws, []).code, 2)
  assert.equal(query(ws, ["names.rq", "--format", "xml"]).code, 2)
  assert.equal(query(ws, ["names.rq", "--timeout", "soon"]).code, 2)
  for (const huge of ["1e300", "inf", "NaN", "-1"]) {
    const r = query(ws, ["names.rq", "--timeout", huge])
    assert.equal(r.code, 2, `${huge}: ${r.err}`)
    assert.match(r.err, /--timeout needs a number of seconds/)
  }
  const help = query(ws, ["--help"])
  assert.equal(help.code, 0)
  assert.match(help.out, /^Usage: turtle-lsp query/)
  const usage = spawnSync(serverPath, ["help"], { encoding: "utf8" })
  assert.match(usage.stdout, /turtle-lsp query \(FILE\.rq \| -e QUERY\)/)
})
