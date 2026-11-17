// `--format json` on the command line: a failure is a JSON error object on
// stdout, and `format --format json` lists what it did. Run as a user runs
// it, on folders written by each test. Only stdout, stderr, the exit code and
// file contents are asserted.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, tempDir } from "./lsp-client.mjs"

const PREFIXES = "@prefix ex: <http://example.org/> .\n"
const FORMATTED = `${PREFIXES}\nex:bob a ex:Person ;\n    ex:name "Bob" .\n`
const UNFORMATTED = `${PREFIXES}ex:bob   a ex:Person;ex:name "Bob".\n`
const BROKEN = `${PREFIXES}ex:bob a .\n`
const DATA = `${PREFIXES}ex:a ex:p ex:b .\nex:b ex:p ex:c .\nex:c ex:p ex:d .\nex:d ex:p ex:e .\n`

// A workspace with `files` ({relative path: text or bytes}) and a `.git`, so
// it is its own root; removed after the test.
function workspace(t, files) {
  const ws = tempDir("turtle-cli-json-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  fs.mkdirSync(path.join(ws, ".cache"))
  for (const [name, text] of Object.entries({ ".git/HEAD": "", ...files })) {
    fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
    fs.writeFileSync(path.join(ws, name), text)
  }
  return ws
}

// Runs the binary in the workspace with a private import cache.
function run(ws, args) {
  const r = spawnSync(serverPath, args, {
    cwd: ws,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1" },
  })
  return { code: r.status, out: r.stdout, err: r.stderr, lines: r.stdout.split("\n").filter(Boolean) }
}

const read = (ws, name) => fs.readFileSync(path.join(ws, name), "utf8")

// The failure in `r` as JSON: one line on stdout, the message on stderr,
// exit 2. `placed`: stderr carries no `turtle-lsp: ` (a FILE:LINE:COL error).
function jsonFailure(r, { placed = false } = {}) {
  assert.equal(r.code, 2, r.err)
  assert.equal(r.lines.length, 1, r.out)
  assert.ok(r.out.endsWith("\n"))
  const v = JSON.parse(r.out)
  assert.deepEqual(Object.keys(v).sort(), ["error", "version"])
  assert.equal(v.version, 1)
  assert.deepEqual(Object.keys(v.error), ["message"])
  assert.equal(typeof v.error.message, "string")
  const line = placed ? v.error.message : `turtle-lsp: ${v.error.message}`
  assert.equal(r.err, `${line}\n`)
  return v.error.message
}

// The failures of the ticket, and the ones a query finds once it runs, with
// `--format json` in `args`. `jsonOnly`: no failure without `--format json`.
const FAILURES = [
  { args: ["check", "--format", "json", "--bogus"], message: /^unknown option '--bogus'/ },
  { args: ["check", "--format", "json", "nope.ttl"], message: /^nope\.ttl: no such file/ },
  { args: ["check", "--format", "json", "--root", "nope"], message: /--root .*nope: not a directory$/ },
  { args: ["check", "--format", "json", "--statistics"], message: /drop --format/, jsonOnly: true },
  { args: ["format", "--format", "json", "nope.ttl"], message: /^nope\.ttl: no such file/ },
  { args: ["format", "--format", "json", "--diff"], message: /--diff/, jsonOnly: true },
  { args: ["format", "--format", "json", "-"], message: /standard input/, jsonOnly: true },
  { args: ["query", "--format", "json", "-e", "SELEC"], message: /^-e:1:1: .*SELEC/, placed: true },
  { args: ["query", "--format", "json", "-e", "INSERT DATA { <http://example.org/a> <http://example.org/p> <http://example.org/b> }"], message: /UPDATE is not supported/ },
  { args: ["query", "--format", "json", "-e", "CONSTRUCT WHERE { ?s ?p ?o }"], message: /does not fit a CONSTRUCT/, jsonOnly: true },
  {
    args: ["query", "--format", "json", "--max-buffered-rows", "2", "-e", "SELECT * WHERE { ?s ?p ?o } ORDER BY ?o"],
    message: /kept more than 2 rows in memory/,
  },
  { args: ["codes", "--format", "json", "--bogus"], message: /^unknown option '--bogus'/ },
  { args: ["explain", "--format", "json", "nope"], message: /^unknown diagnostic code 'nope'/ },
  { args: ["explain", "--format", "json"], message: /^explain needs a diagnostic code\n\nUsage:/ },
]

test("--format json: a failure prints one JSON error line, the same message on stderr, and exits 2", (t) => {
  const ws = workspace(t, { "data.ttl": DATA })
  for (const { args, message, placed } of FAILURES) {
    assert.match(jsonFailure(run(ws, args), { placed }), message, args.join(" "))
  }
})

test("--format=json works the same; without --format json, stdout stays empty", (t) => {
  const ws = workspace(t, { "data.ttl": DATA })
  for (const { args, placed, jsonOnly } of FAILURES) {
    const at = args.indexOf("--format")
    const joined = [...args.slice(0, at), "--format=json", ...args.slice(at + 2)]
    const r = run(ws, joined)
    jsonFailure(r, { placed })
    assert.equal(r.out, run(ws, args).out, joined.join(" "))
    if (jsonOnly) continue
    const without = run(ws, [...args.slice(0, at), ...args.slice(at + 2)])
    assert.equal(without.code, 2, args.join(" "))
    assert.equal(without.out, "", args.join(" "))
    assert.equal(without.err, r.err, args.join(" "))
  }
  // The last --format counts, as in the parsers; after --, it is a path.
  assert.equal(run(ws, ["check", "--format", "json", "--format", "concise", "--bogus"]).out, "")
  assert.equal(run(ws, ["check", "--bogus", "--", "--format", "json"]).out, "")
})

test("--format json: a refused turtle-lsp.json is a JSON error", (t) => {
  const ws = workspace(t, { "data.ttl": DATA, "turtle-lsp.json": '{"query": {"timeoutSeconds": 1e300}}' })
  const message = jsonFailure(run(ws, ["check", "--format", "json"]))
  assert.match(message, /^turtle-lsp\.json: query\.timeoutSeconds needs a number of seconds/)
})

// The invariant: with --format json, stdout is exactly one JSON value on
// every exit code, for every command.
test("--format json: stdout parses as one JSON value on every exit code", (t) => {
  const ws = workspace(t, {
    "clean.ttl": FORMATTED,
    "broken.ttl": BROKEN,
    "messy.ttl": UNFORMATTED,
  })
  // Apart: as context, it would fail every check of the other folder.
  const latin1 = workspace(t, { "latin1.ttl": Buffer.from([0xff, 0xfe]) })
  const rows = [
    [["check", "clean.ttl"], 0],
    [["check", "broken.ttl"], 1],
    [["check", "latin1.ttl"], 2, latin1], // a report: the file could not be checked
    [["check", "--bogus"], 2],
    [["format", "clean.ttl"], 0],
    [["format", "--check", "messy.ttl"], 1],
    [["format", "broken.ttl"], 2], // a report: the file is not formatted
    [["format", "nope.ttl"], 2],
    [["query", "-e", "PREFIX ex: <http://example.org/> ASK { ex:bob a ex:Person }", "clean.ttl"], 0],
    [["query", "-e", "PREFIX ex: <http://example.org/> ASK { ex:ann a ex:Person }", "clean.ttl"], 1],
    [["query", "-e", "SELECT ?s WHERE { ?s ?p ?o }", "clean.ttl"], 0],
    [["query", "-e", "ASK {", "clean.ttl"], 2],
    [["codes"], 0],
    [["codes", "extra"], 2],
    [["explain", "syntax"], 0],
    [["explain", "syntax", "unused-prefix"], 2],
  ]
  for (const [args, code, folder = ws] of rows) {
    const r = run(folder, [...args, "--format", "json"])
    assert.equal(r.code, code, `${args.join(" ")}: ${r.err}`)
    assert.doesNotThrow(() => JSON.parse(r.out), `${args.join(" ")}: ${r.out}`)
  }
  assert.equal(read(ws, "messy.ttl"), UNFORMATTED)
})

test("format --format json: formatted, not-formatted (a syntax error, not UTF-8) with its reason, and the counts", (t) => {
  const files = { "a.ttl": UNFORMATTED, "b.ttl": FORMATTED, "c.ttl": BROKEN, "d.ttl": Buffer.from([0xff, 0xfe]) }
  const ws = workspace(t, files)
  const check = run(ws, ["format", "--format", "json", "--check"])
  assert.equal(check.code, 2, check.err)
  assert.equal(check.lines.length, 1)
  const notFormatted = [
    { path: "c.ttl", status: "not-formatted", reason: "the file has 1 syntax error; fix it first" },
    { path: "d.ttl", status: "not-formatted", reason: "not valid UTF-8 (first bad byte at offset 0)" },
  ]
  assert.deepEqual(JSON.parse(check.out), {
    version: 1,
    files: [{ path: "a.ttl", status: "would-format" }, ...notFormatted],
    summary: { changed: 1, notFormatted: 2, filesChecked: 4 },
  })
  assert.equal(read(ws, "a.ttl"), UNFORMATTED)

  const r = run(ws, ["format", "--format=json"])
  assert.equal(r.code, 2, r.err)
  assert.equal(r.lines.length, 1)
  assert.deepEqual(JSON.parse(r.out), {
    version: 1,
    files: [{ path: "a.ttl", status: "formatted" }, ...notFormatted],
    summary: { changed: 1, notFormatted: 2, filesChecked: 4 },
  })
  assert.equal(read(ws, "a.ttl"), FORMATTED)
  assert.equal(read(ws, "c.ttl"), BROKEN)

  // Nothing left to change: nothing listed; exit codes as in text.
  const again = run(ws, ["format", "--format", "json", "a.ttl", "b.ttl"])
  assert.equal(again.code, 0, again.err)
  assert.deepEqual(JSON.parse(again.out), {
    version: 1,
    files: [],
    summary: { changed: 0, notFormatted: 0, filesChecked: 2 },
  })
  fs.writeFileSync(path.join(ws, "a.ttl"), UNFORMATTED)
  const would = run(ws, ["format", "--format", "json", "--check", "a.ttl"])
  assert.equal(would.code, 1, would.err)
  assert.equal(would.code, run(ws, ["format", "--check", "a.ttl"]).code)
})

test("format --format json --diff: a JSON error, nothing changed", (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED })
  assert.match(jsonFailure(run(ws, ["format", "--format", "json", "--diff"])), /--diff/)
  assert.equal(read(ws, "a.ttl"), UNFORMATTED)
})

test("format --format: text is today's output; another value is an error", (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED })
  const text = run(ws, ["format", "--format", "text", "--check"])
  assert.deepEqual([text.code, text.out], [1, "a.ttl\n"])
  const bad = run(ws, ["format", "--format", "yaml"])
  assert.equal(bad.code, 2)
  assert.equal(bad.out, "")
  assert.match(bad.err, /--format must be text or json, not 'yaml'/)
  const help = run(ws, ["format", "--help"])
  assert.match(help.out, /--format F +text \(default\) or json/)
})

test("help: each command says a failure with --format json is a JSON error object", (t) => {
  const ws = workspace(t, {})
  for (const command of ["check", "format", "query", "codes", "explain"]) {
    const r = run(ws, ["help", command])
    assert.equal(r.code, 0)
    assert.match(r.out, /With --format json, a failure .*\n\{"version": 1, "error": \{"message": M\}\}/, command)
  }
})
