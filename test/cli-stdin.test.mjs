// `turtle-lsp check -` and `turtle-lsp format -`: standard input, named by
// `--stdin-filename`, run as a user runs them on folders written by each
// test. Only stdout, stderr, the exit code and file contents are asserted.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { serverPath, tempDir } from "./lsp-client.mjs"

const PREFIXES = `@prefix ex: <http://example.org/> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
const SHAPES = `ex:PersonShape a sh:NodeShape ;
    sh:targetClass ex:Person ;
    sh:property [ sh:path ex:name ; sh:minCount 1 ] .

ex:Person a rdfs:Class .

ex:name a rdf:Property .
`
const CLEAN = `ex:bob a ex:Person ;
    ex:name "Bob" .
`
// Clean but for the shape: no ex:name.
const NAMELESS = "ex:ann a ex:Person .\n"
const UNFORMATTED = "ex:bob   a ex:Person;ex:name \"Bob\".\n"

// A workspace with `files` ({relative path: text}); removed after the test.
function workspace(t, files) {
  const ws = tempDir("turtle-cli-stdin-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  fs.mkdirSync(path.join(ws, ".cache"))
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
    fs.writeFileSync(path.join(ws, name), text)
  }
  return ws
}

// Runs the binary in `cwd` (default: the workspace) with `input` on stdin.
function run(ws, args, input = "", { cwd = ws } = {}) {
  const r = spawnSync(serverPath, args, {
    cwd,
    input,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), NO_COLOR: "1" },
  })
  return { code: r.status, out: r.stdout, err: r.stderr, lines: r.stdout.split("\n").filter(Boolean) }
}

const read = (ws, name) => fs.readFileSync(path.join(ws, name), "utf8")

// ---- check ----

test("check -: problems are reported under stdin.ttl, or the --stdin-filename path", (t) => {
  const ws = workspace(t, { "a.ttl": CLEAN })
  const r = run(ws, ["check", "-"], "ex:a ex:b ex:c .\n")
  assert.equal(r.code, 1, r.err)
  assert.deepEqual(r.lines.map((l) => l.split(": ")[0]), ["stdin.ttl:1:1", "stdin.ttl:1:6", "stdin.ttl:1:11"])
  assert.ok(r.lines.every((l) => l.includes("error[undeclared-prefix]")), r.out)
  const named = run(ws, ["check", "-", "--stdin-filename", "data/x.ttl"], "ex:a ex:b ex:c .\n")
  assert.equal(named.code, 1, named.err)
  assert.equal(named.lines.length, 3)
  assert.ok(named.lines.every((l) => l.startsWith("data/x.ttl:1:")), named.out)
  const json = run(ws, ["check", "-", "--format", "json"], "ex:a ex:b ex:c .\n")
  assert.deepEqual(JSON.parse(json.out).files.map((f) => f.path), ["stdin.ttl"])
  const clean = run(ws, ["check", "-"], `@prefix ex: <http://example.org/> .\n${CLEAN}`)
  assert.equal(clean.code, 0, clean.err)
  assert.equal(clean.out, "")
  assert.equal(clean.err, "")
})

test("check -: the workspace is context, not reported: its prefixes.ttl and its shapes apply", (t) => {
  // A broken file elsewhere is not reported: only the text is.
  const ws = workspace(t, {
    ".git/HEAD": "",
    "prefixes.ttl": PREFIXES,
    "shapes.ttl": SHAPES,
    "other/broken.ttl": "ex:a ex:b\n",
  })
  const ok = run(ws, ["check", "-"], CLEAN)
  assert.equal(ok.code, 0, ok.out + ok.err)
  assert.equal(ok.out, "")
  const violation = run(ws, ["check", "-", "--stdin-filename", "data/people.ttl"], NAMELESS)
  assert.equal(violation.code, 1, violation.err)
  assert.equal(violation.lines.length, 1, violation.out)
  assert.match(violation.lines[0], /^data\/people\.ttl:1:1: error\[shacl-violation\] /)
  // Run from a subfolder, the name is taken from there; the root is the folder with the .git.
  const sub = run(ws, ["check", "-", "--stdin-filename", "x.ttl"], NAMELESS, { cwd: path.join(ws, "other") })
  assert.equal(sub.code, 1, sub.err)
  assert.match(sub.lines[0], /^x\.ttl:1:1: error\[shacl-violation\] /)
})

test("check -: the text replaces a file at its path, which is neither context nor reported", (t) => {
  // On disk the file breaks the shape; the text does not.
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "shapes.ttl": SHAPES, "data/people.ttl": NAMELESS })
  assert.equal(run(ws, ["check"]).code, 1)
  const r = run(ws, ["check", "-", "--stdin-filename", "data/people.ttl"], CLEAN)
  assert.equal(r.code, 0, r.out + r.err)
  assert.equal(r.out, "")
  // The same file named through a symbolic link to its folder (macOS: /var is /private/var).
  const viaLink = path.join(os.tmpdir(), path.basename(ws), "data", "people.ttl")
  const linked = run(ws, ["check", "-", "--stdin-filename", viaLink], CLEAN)
  assert.equal(linked.code, 0, linked.out + linked.err)
  // The text with the problem: reported once, under the path.
  const json = run(ws, ["check", "-", "--stdin-filename", "data/people.ttl", "--format", "json"], NAMELESS)
  assert.equal(json.code, 1, json.err)
  const report = JSON.parse(json.out)
  assert.deepEqual(report.files.map((f) => f.path), ["data/people.ttl"])
  assert.equal(report.summary.filesChecked, 1)
  assert.equal(read(ws, "data/people.ttl"), NAMELESS)
})

test("check -: the root is found from the --stdin-filename folder; --root wins", (t) => {
  // `repo` has a .git and a prefixes.ttl; the current folder has neither.
  const ws = workspace(t, { "repo/.git/HEAD": "", "repo/prefixes.ttl": PREFIXES, "work/.keep": "" })
  const cwd = path.join(ws, "work")
  const inRepo = run(ws, ["check", "-", "--stdin-filename", "../repo/data/x.ttl"], CLEAN, { cwd })
  assert.equal(inRepo.code, 0, inRepo.out + inRepo.err)
  const outside = run(ws, ["check", "-", "--stdin-filename", "x.ttl"], CLEAN, { cwd })
  assert.equal(outside.code, 1, outside.err)
  assert.ok(outside.lines.every((l) => l.includes("undeclared-prefix")), outside.out)
  // With the current folder as the root, the file is outside it: repo's prefixes.ttl does not apply.
  const root = run(ws, ["check", "-", "--stdin-filename", "../repo/data/x.ttl", "--root", "."], CLEAN, { cwd })
  assert.equal(root.code, 1, root.err)
  // Outside the current folder, the path is shown in full, as for a file argument.
  const shown = `${path.join(ws, "repo", "data", "x.ttl").replace(/\\/g, "/")}:`
  assert.ok(root.lines.every((l) => l.startsWith(shown)), root.out)
  assert.ok(root.lines.every((l) => l.includes("undeclared-prefix")), root.out)
})

test("check -: the syntax comes from the --stdin-filename extension", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES })
  const sparql = run(ws, ["check", "-", "--stdin-filename", "q.rq"], "SELECT * WHERE { ?s ?p }\n")
  assert.equal(sparql.code, 1, sparql.err)
  assert.match(sparql.lines[0], /^q\.rq:1:\d+: error\[syntax\] SPARQL syntax error/)
  assert.equal(run(ws, ["check", "-", "--stdin-filename", "q.rq"], "SELECT * WHERE { ?s ?p ?o }\n").code, 0)
  const jsonld = run(ws, ["check", "-", "--stdin-filename", "d.jsonld"], '{"@id": "http://example.org/a",\n')
  assert.equal(jsonld.code, 1, jsonld.err)
  assert.match(jsonld.lines[0], /^d\.jsonld:\d+:\d+: error\[json-syntax\] /)
  const okJsonld = '{"@id": "http://example.org/a", "http://example.org/p": "x"}\n'
  assert.equal(run(ws, ["check", "-", "--stdin-filename", "d.jsonld"], okJsonld).code, 0)
})

test("check -: text that is not UTF-8 is the encoding problem", (t) => {
  const ws = workspace(t, {})
  const r = run(ws, ["check", "-"], Buffer.from([0x3c, 0x61, 0x3e, 0xff, 0x0a]))
  assert.equal(r.code, 2, r.err)
  assert.match(r.out, /^stdin\.ttl:1:1: error\[encoding\] .*offset 3/)
})

// ---- format ----

test("format -: prints the formatted text, and formatted text unchanged", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES })
  const r = run(ws, ["format", "-"], UNFORMATTED)
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, CLEAN)
  assert.equal(r.err, "")
  const again = run(ws, ["format", "-"], r.out)
  assert.equal(again.code, 0, again.err)
  assert.equal(again.out, CLEAN)
  assert.equal(again.err, "")
  const sparql = run(ws, ["format", "-", "--stdin-filename", "q.rq"], "SELECT * {?s ?p ?o}")
  assert.equal(sparql.code, 0, sparql.err)
  assert.equal(sparql.out, "SELECT * {\n    ?s ?p ?o\n}\n")
})

test("format -: a file at the --stdin-filename path is not written", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "data/people.ttl": UNFORMATTED })
  const before = fs.statSync(path.join(ws, "data/people.ttl")).mtimeMs
  const r = run(ws, ["format", "-", "--stdin-filename", "data/people.ttl"], "ex:a   ex:b ex:c.\n")
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, "ex:a ex:b ex:c .\n")
  assert.equal(read(ws, "data/people.ttl"), UNFORMATTED)
  assert.equal(fs.statSync(path.join(ws, "data/people.ttl")).mtimeMs, before)
})

test("format - --check and --diff: nothing written, a diff labelled with the path", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES })
  const would = run(ws, ["format", "-", "--check"], UNFORMATTED)
  assert.equal(would.code, 1, would.err)
  assert.equal(would.out, "")
  const done = run(ws, ["format", "-", "--check"], CLEAN)
  assert.equal(done.code, 0, done.err)
  assert.equal(done.out, "")
  const diff = run(ws, ["format", "-", "--diff", "--stdin-filename", "data/x.ttl"], UNFORMATTED)
  assert.equal(diff.code, 1, diff.err)
  const lines = diff.out.split("\n")
  assert.deepEqual(lines.slice(0, 2), ["--- a/data/x.ttl", "+++ b/data/x.ttl"])
  assert.ok(lines.includes(`-${UNFORMATTED.trimEnd()}`), diff.out)
  assert.ok(lines.includes("+ex:bob a ex:Person ;"), diff.out)
  const same = run(ws, ["format", "-", "--diff"], CLEAN)
  assert.equal(same.code, 0, same.err)
  assert.equal(same.out, "")
})

test("format -: a syntax error or text that is not UTF-8 gives exit 2 and nothing on stdout", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES })
  const broken = run(ws, ["format", "-"], "ex:a ex:b\n")
  assert.equal(broken.code, 2)
  assert.equal(broken.out, "")
  assert.match(broken.err, /^stdin\.ttl: not formatted: .*syntax error/)
  const bytes = run(ws, ["format", "-"], Buffer.from([0x3c, 0x61, 0x3e, 0xff, 0x0a]))
  assert.equal(bytes.code, 2)
  assert.equal(bytes.out, "")
  assert.match(bytes.err, /^stdin\.ttl: not formatted: not valid UTF-8/)
})

// ---- what is refused ----

test("standard input: alone, without --fix, named only with -, and never for query", (t) => {
  const ws = workspace(t, { "a.ttl": CLEAN })
  const refused = [
    [["check", "-", "a.ttl"], /cannot be combined with other paths/],
    [["format", "-", "a.ttl"], /cannot be combined with other paths/],
    [["check", "-", "--fix"], /--fix .*standard input/],
    [["check", "--stdin-filename", "x.ttl"], /--stdin-filename works only with '-'/],
    [["format", "-", "--stdin-filename"], /--stdin-filename needs a value/],
    [["check", "-", "--stdin-filename", "notes.txt"], /not the name of an RDF or SPARQL file/],
    [["query", "-e", "ASK {}", "-"], /'-' \(standard input\) is not supported/],
  ]
  for (const [args, message] of refused) {
    const r = run(ws, args, CLEAN)
    assert.equal(r.code, 2, args.join(" "))
    assert.equal(r.out, "", args.join(" "))
    assert.match(r.err, message, args.join(" "))
  }
  assert.equal(read(ws, "a.ttl"), CLEAN)
})

test("check and format --help describe - and --stdin-filename", () => {
  for (const command of ["check", "format"]) {
    const help = spawnSync(serverPath, [command, "--help"], { encoding: "utf8" }).stdout
    assert.match(help, /^ {2}- {2,}as the only path: /m, command)
    assert.match(help, /^ {2}--stdin-filename PATH /m, command)
  }
})
