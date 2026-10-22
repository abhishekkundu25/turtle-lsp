// The command line: `turtle-lsp check` and `turtle-lsp format`, run as a user
// runs them, on folders written by each test. Only stdout, stderr, the exit
// code and file contents are asserted.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import crypto from "node:crypto"
import http from "node:http"
import fs from "node:fs"
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

// A workspace with `files` ({relative path: text}); removed after the test.
function workspace(t, files) {
  const ws = tempDir("turtle-cli-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  fs.mkdirSync(path.join(ws, ".cache"))
  for (const [name, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
    fs.writeFileSync(path.join(ws, name), text)
  }
  return ws
}

// Runs the binary in `cwd` (default: the workspace) with a private import cache.
function run(ws, args, { cwd = ws, cache = path.join(ws, ".cache") } = {}) {
  const r = spawnSync(serverPath, args, {
    cwd,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: cache, NO_COLOR: "1" },
  })
  return { code: r.status, out: r.stdout, err: r.stderr, lines: r.stdout.split("\n").filter(Boolean) }
}

const read = (ws, name) => fs.readFileSync(path.join(ws, name), "utf8")

// ---- check ----

test("check: a clean folder exits 0 with no output", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "shapes.ttl": SHAPES, "data/people.ttl": CLEAN, "q.rq": "SELECT * WHERE { ?s ?p ?o }\n" })
  const r = run(ws, ["check"])
  assert.equal(r.code, 0, r.err)
  assert.equal(r.out, "")
  assert.equal(r.err, "")
})

test("check: a syntax error exits 1 at its line and column, summary on stderr", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "data/broken.ttl": "ex:a ex:b ex:c .\nex:d ex:e ex:f\n" })
  const r = run(ws, ["check"])
  assert.equal(r.code, 1)
  assert.deepEqual(r.lines, ["data/broken.ttl:2:11: error[syntax] Expected '.' to end the statement"])
  assert.equal(r.err.trim(), "1 error, 0 warnings in 1 file")
})

test("check: an undeclared prefix suggests its declaration", (t) => {
  const ws = workspace(t, { "a.ttl": "<http://x/a> foaf:name \"A\" .\n" })
  const r = run(ws, ["check"])
  assert.equal(r.code, 1)
  assert.equal(r.lines.length, 1)
  assert.match(r.lines[0], /^a\.ttl:1:14: error\[undeclared-prefix\] .*@prefix foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/>/)
})

test("check: a SHACL violation is reported at the triple's line", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "shapes.ttl": SHAPES, "data/people.ttl": `${CLEAN}\nex:alice a ex:Person .\n` })
  const r = run(ws, ["check"])
  assert.equal(r.code, 1)
  assert.equal(r.lines.length, 1, r.out)
  assert.match(r.lines[0], /^data\/people\.ttl:4:1: error\[shacl-violation\] .*ex:alice/)
  // sh:Warning keeps its severity: a warning, so the check passes.
  fs.writeFileSync(path.join(ws, "shapes.ttl"), SHAPES.replace("sh:minCount 1", "sh:minCount 1 ; sh:severity sh:Warning"))
  const w = run(ws, ["check"])
  assert.equal(w.code, 0)
  assert.match(w.lines[0], /^data\/people\.ttl:4:1: warning\[shacl-violation\]/)
})

test("check: a TriG dataset that mentions a violating node does not repeat its violations", (t) => {
  const ex = "@prefix ex: <http://example.org/> .\n"
  const sparql = `ex:PersonShape sh:sparql [ sh:message "no ex:bad" ; sh:select "SELECT $this ?value WHERE { $this <http://example.org/bad> ?value }" ] .\n`
  const ws = workspace(t, {
    "prefixes.ttl": PREFIXES,
    "shapes.ttl": SHAPES + sparql,
    "data/people.ttl": "ex:alice a ex:Person ;\n    ex:bad 1 .\n",
    "unrelated.trig": `${ex}ex:g { ex:bob ex:name "Bob" . }\n`,
    "mentions.trig": `${ex}ex:g { ex:carol ex:knows ex:alice . ex:alice ex:nick "Al" . }\n`,
  })
  const r = run(ws, ["check"])
  const shacl = r.lines.filter((l) => l.includes("shacl-violation")).sort()
  assert.deepEqual(shacl, [
    "data/people.ttl:1:1: error[shacl-violation] ex:name of ex:alice: expected at least 1 value(s)",
    "data/people.ttl:1:1: error[shacl-violation] no ex:bad",
  ], r.out)
  // A violation the dataset adds is reported in it.
  fs.writeFileSync(path.join(ws, "mentions.trig"), `${ex}ex:g { ex:alice ex:bad 2 . }\n`)
  const added = run(ws, ["check"]).lines.filter((l) => l.startsWith("mentions.trig") && l.includes("shacl-violation"))
  assert.deepEqual(added, ["mentions.trig:2:8: error[shacl-violation] no ex:bad"])
})

test("check: one file is checked with the workspace's prefixes and shapes as context", (t) => {
  const ws = workspace(t, {
    ".git/HEAD": "ref: refs/heads/main\n",
    "prefixes.ttl": PREFIXES,
    "shapes/person.ttl": SHAPES,
    "data/people.ttl": `${CLEAN}\nex:alice a ex:Person .\n`,
    "data/other.ttl": "ex:x ex:y foaf:z .\n",
  })
  // From a subfolder: the root is the folder with .git; only the named file is reported.
  const r = run(ws, ["check", "people.ttl"], { cwd: path.join(ws, "data") })
  assert.equal(r.code, 1)
  assert.equal(r.lines.length, 1, r.out)
  assert.match(r.lines[0], /^people\.ttl:4:1: error\[shacl-violation\]/)
  // --root names the root explicitly.
  const s = run(ws, ["check", "--root", ws, "data/people.ttl"], { cwd: ws })
  assert.match(s.lines[0], /^data\/people\.ttl:4:1: error\[shacl-violation\]/)
})

test("check: a folder check skips hidden folders, as the editor's indexing does; a file named in one is checked", (t) => {
  const ws = workspace(t, {
    "prefixes.ttl": PREFIXES,
    "shapes.ttl": SHAPES,
    "data/people.ttl": CLEAN,
    ".data/store.ttl": "ex:alice a ex:Person .\n",
  })
  const r = run(ws, ["check"])
  assert.equal(r.code, 0, r.out)
  const named = run(ws, ["check", ".data/store.ttl"])
  assert.equal(named.code, 1)
  assert.equal(named.lines.length, 1, named.out)
  assert.match(named.lines[0], /^\.data\/store\.ttl:1:1: error\[shacl-violation\] .*ex:alice/)
  assert.deepEqual(run(ws, ["check", ".data"]).lines, named.lines)
})

test("check: --max-warnings and --exit-zero", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "a.ttl": "<http://x/a> rdfs:labl \"a\" .\n" })
  const plain = run(ws, ["check"])
  assert.equal(plain.code, 0, "warnings alone pass")
  assert.match(plain.lines[0], /^a\.ttl:1:14: warning\[unknown-term\] .*Did you mean 'label'/)
  assert.equal(run(ws, ["check", "--max-warnings", "1"]).code, 0)
  assert.equal(run(ws, ["check", "--max-warnings=0"]).code, 1)
  fs.writeFileSync(path.join(ws, "b.ttl"), "ex:a ex:b\n")
  assert.equal(run(ws, ["check"]).code, 1)
  const z = run(ws, ["check", "--exit-zero"])
  assert.equal(z.code, 0)
  assert.equal(z.lines.length, 2)
})

test("check: a bad flag or a missing path exits 2 with a message on stderr", (t) => {
  const ws = workspace(t, { "a.ttl": CLEAN })
  for (const args of [["check", "--bogus"], ["check", "--format", "xml"], ["check", "--max-warnings", "x"], ["check", "missing.ttl"], ["format", "--fix"]]) {
    const r = run(ws, args)
    assert.equal(r.code, 2, args.join(" "))
    assert.equal(r.out, "")
    assert.match(r.err, /^turtle-lsp: /)
  }
  const h = run(ws, ["check", "--help"])
  assert.equal(h.code, 0)
  assert.match(h.out, /--max-warnings N/)
})

test("check: --fix applies the safe fixes, --unsafe-fixes the renames too", (t) => {
  const text = "<http://x/a> rdfs:labl \"a\" .\n"
  const ws = workspace(t, { "a.ttl": text })
  const r = run(ws, ["check", "--fix"])
  const fixed = read(ws, "a.ttl")
  assert.match(fixed, /^@prefix rdfs: <http:\/\/www\.w3\.org\/2000\/01\/rdf-schema#> \.\n/)
  assert.match(fixed, /rdfs:labl "a"/)
  assert.equal(r.code, 0, "the rename is left as a warning")
  assert.equal(r.lines.length, 1)
  assert.match(r.lines[0], /warning\[unknown-term\]/)
  assert.match(r.err, /Applied 1 fix in 1 file/)
  const u = run(ws, ["check", "--fix", "--unsafe-fixes"])
  assert.equal(u.code, 0)
  assert.equal(u.out, "")
  assert.match(read(ws, "a.ttl"), /rdfs:label "a"/)
  // Context files are never written.
  const ws2 = workspace(t, { "a.ttl": text, "b.ttl": text })
  run(ws2, ["check", "--fix", "a.ttl"])
  assert.equal(read(ws2, "b.ttl"), text)
  assert.notEqual(read(ws2, "a.ttl"), text)
})

test("check: columns count code points", (t) => {
  // "é" is 2 UTF-8 bytes and 1 UTF-16 unit; "😀" 4 bytes and 2 units.
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "a.ttl": "<http://x/é😀> rdfs:labl \"a\" .\n" })
  const r = run(ws, ["check"])
  assert.match(r.lines[0], /^a\.ttl:1:15: warning\[unknown-term\]/)
})

test("check: --select, --ignore and --no-shacl", (t) => {
  const ws = workspace(t, {
    "prefixes.ttl": PREFIXES,
    "shapes.ttl": SHAPES,
    "data.ttl": "ex:alice a ex:Person ; rdfs:labl \"A\" ; foaf:knows ex:alice .\n",
  })
  const codes = (r) => r.lines.map((l) => l.match(/\[([^\]]+)\]/)[1]).sort()
  assert.deepEqual(codes(run(ws, ["check"])), ["shacl-violation", "undeclared-prefix", "unknown-term"])
  assert.deepEqual(codes(run(ws, ["check", "--select", "unknown-term,shacl-violation"])), ["shacl-violation", "unknown-term"])
  const ignored = run(ws, ["check", "--ignore", "undeclared-prefix", "--ignore=shacl-violation"])
  assert.deepEqual(codes(ignored), ["unknown-term"])
  assert.equal(ignored.code, 0, "ignored errors do not fail the check")
  assert.deepEqual(codes(run(ws, ["check", "--no-shacl"])), ["undeclared-prefix", "unknown-term"])
})

test("check: --max-diagnostics, --statistics and --format json", (t) => {
  const ws = workspace(t, { "a.ttl": "<http://x/a> foaf:a 1 .\n<http://x/b> foaf:b 2 .\n<http://x/c> foaf:c 3 .\n" })
  const capped = run(ws, ["check", "--max-diagnostics", "1"])
  assert.equal(capped.lines.length, 1)
  assert.match(capped.err, /3 errors/, "the summary counts everything")
  const stats = run(ws, ["check", "--statistics"])
  assert.match(stats.out, /3\s+undeclared-prefix/)
  const json = JSON.parse(run(ws, ["check", "--format", "json"]).out)
  assert.equal(json.version, 1)
  assert.equal(json.files[0].path, "a.ttl")
  assert.equal(json.files[0].diagnostics.length, 3)
  assert.deepEqual(json.files[0].diagnostics[0].range.start, { line: 1, column: 14 })
  const file = path.join(ws, "report.json")
  const o = run(ws, ["check", "--format=json", "--output-file", "report.json"])
  assert.equal(o.out, "")
  assert.equal(JSON.parse(fs.readFileSync(file, "utf8")).version, 1)
})

// SHA-256 of the URL, as hash_name in src/fetch.rs names cache files; a cache
// file starts with a line that names its URL.
function cacheName(url) {
  return crypto.createHash("sha256").update(url).digest("hex")
}
const cacheText = (url, text) => `# turtle-lsp cache for ${url}\n${text}`

test("check: owl:imports resolve from the import cache, offline", (t) => {
  const ws = workspace(t, {
    "d.ttl": `@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://ex.com/d> a owl:Ontology ; owl:imports <http://ex.com/onto> .
`,
  })
  const missing = run(ws, ["check"])
  assert.match(missing.out, /^d\.ttl:2:48: info\[unresolved-import\]/)
  const cache = path.join(ws, ".cache", "imports")
  fs.mkdirSync(cache, { recursive: true })
  fs.writeFileSync(
    path.join(cache, `${cacheName("http://ex.com/onto")}.ttl`),
    cacheText("http://ex.com/onto", "<http://ex.com/onto> a <http://www.w3.org/2002/07/owl#Ontology> .\n"),
  )
  const found = run(ws, ["check"])
  assert.equal(found.out, "")
  assert.equal(found.code, 0)
})

test("check: imports.unresolved in turtle-lsp.json keeps SHACL results at full severity", (t) => {
  const ws = workspace(t, {
    "d.ttl": `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix sh: <http://www.w3.org/ns/shacl#> .
@prefix ex: <http://example.org/> .
<http://ex.com/d> a owl:Ontology ; owl:imports <http://ex.com/onto> .
ex:S a sh:NodeShape ; sh:targetClass ex:P ;
    sh:property [ sh:path ex:kind ; sh:class ex:Kind ] .
ex:a a ex:P ; ex:kind ex:k .
`,
  })
  const shacl = (r) => r.lines.filter((l) => l.includes("shacl-violation"))
  const soft = run(ws, ["check"])
  assert.equal(shacl(soft).length, 1, soft.out)
  assert.match(shacl(soft)[0], /: warning\[shacl-violation\] .*\(may change once these imports resolve: http:\/\/ex\.com\/onto\)$/)
  fs.writeFileSync(path.join(ws, "turtle-lsp.json"), JSON.stringify({ imports: { unresolved: "ignore" } }))
  const full = run(ws, ["check"])
  assert.equal(full.code, 1, full.err)
  assert.equal(shacl(full).length, 1, full.out)
  assert.match(shacl(full)[0], /: error\[shacl-violation\] /)
  assert.doesNotMatch(shacl(full)[0], /imports/)
  // The import is still reported.
  assert.ok(full.lines.some((l) => /^d\.ttl:4:\d+: info\[unresolved-import\]/.test(l)), full.out)
})

// ---- format ----

const UNFORMATTED = `@prefix ex: <http://example.org/> .
ex:bob a ex:Person ; ex:name "Bob" .
`

test("format --check: exit 1, the files listed, nothing changed", (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED, "q.rq": "select * where { ?s ?p ?o }\n" })
  const r = run(ws, ["format", "--check"])
  assert.equal(r.code, 1)
  assert.deepEqual(r.lines, ["a.ttl", "q.rq"])
  assert.equal(read(ws, "a.ttl"), UNFORMATTED)
})

test("format: writes the files, and a second run changes nothing", (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED })
  const r = run(ws, ["format"])
  assert.equal(r.code, 0, r.err)
  const once = read(ws, "a.ttl")
  assert.notEqual(once, UNFORMATTED)
  assert.equal(run(ws, ["format"]).code, 0)
  assert.equal(read(ws, "a.ttl"), once)
  const c = run(ws, ["format", "--check"])
  assert.equal(c.code, 0)
  assert.equal(c.out, "")
})

test("format --diff: a unified diff, nothing changed", (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED })
  const r = run(ws, ["format", "--diff"])
  assert.equal(r.code, 1)
  assert.equal(read(ws, "a.ttl"), UNFORMATTED)
  assert.match(r.out, /^--- a\/a\.ttl\n\+\+\+ b\/a\.ttl\n@@ -1,2 \+1,\d+ @@\n/)
  assert.match(r.out, /\n-ex:bob a ex:Person ; ex:name "Bob" \.\n/)
})

test("format: a file with syntax errors is left alone and reported (exit 2)", (t) => {
  const broken = "ex:a ex:b\n"
  const ws = workspace(t, { "prefixes.ttl": "@prefix ex: <http://example.org/> .\n", "a.ttl": UNFORMATTED, "b.ttl": broken })
  for (const args of [["format", "--check"], ["format"]]) {
    const r = run(ws, args)
    assert.equal(r.code, 2, args.join(" "))
    assert.match(r.err, /^b\.ttl: not formatted: the file has 1 syntax error/m)
    assert.equal(read(ws, "b.ttl"), broken)
  }
  assert.notEqual(read(ws, "a.ttl"), UNFORMATTED, "the other files are formatted")
})

// ---- turtle-lsp.json is untrusted ----

// Like `run`, for a test whose own process must answer requests meanwhile.
function runAsync(ws, args) {
  return new Promise((resolve) => {
    const p = spawn(serverPath, args, {
      cwd: ws,
      env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache"), TURTLE_LSP_ALLOW_PRIVATE_HOSTS: "1", NO_COLOR: "1", NO_PROXY: "127.0.0.1,localhost" },
    })
    let out = ""
    let err = ""
    p.stdout.on("data", (d) => (out += d))
    p.stderr.on("data", (d) => (err += d))
    p.on("close", (code) => resolve({ code, out, err }))
  })
}

test("check: turtle-lsp.json cannot turn on the network; --network can", async (t) => {
  const hits = []
  const web = http.createServer((req, res) => {
    hits.push(req.url)
    res.writeHead(404)
    res.end()
  })
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  t.after(() => web.close())
  const ws = workspace(t, {
    "turtle-lsp.json": JSON.stringify({ imports: { network: true } }),
    "d.ttl": `@prefix owl: <http://www.w3.org/2002/07/owl#> .
<http://ex.com/d> a owl:Ontology ; owl:imports <http://127.0.0.1:${web.address().port}/x> .
`,
  })
  const off = await runAsync(ws, ["check", "--root", ws])
  assert.deepEqual(hits, [])
  assert.match(off.out, /info\[unresolved-import\]/)
  assert.match(off.err, /warning: turtle-lsp\.json: ignored imports\.network/)
  const on = await runAsync(ws, ["check", "--network"])
  assert.equal(hits.length, 1, JSON.stringify(hits))
  assert.equal(on.err.includes("ignored imports.network"), true, "the file still cannot do it; the flag did")
})

test("check: a timeout in turtle-lsp.json that no duration can hold exits 2 with a message, not a panic", (t) => {
  const ws = workspace(t, { "a.ttl": "<http://x/a> <http://x/b> <http://x/c> .\n", "turtle-lsp.json": '{"query":{"timeoutSeconds":1e300}}' })
  const r = run(ws, ["check"])
  assert.equal(r.code, 2, r.err)
  assert.match(r.err, /^turtle-lsp: turtle-lsp\.json: query\.timeoutSeconds needs a number of seconds, not 1e\+300$/m)
  // The other commands ignore the value, with a warning.
  const f = run(ws, ["format", "--check"])
  assert.notEqual(f.code, 101, f.err)
  assert.match(f.err, /query\.timeoutSeconds needs a number of seconds/)
})

test("check: a limit in turtle-lsp.json above the defaults is ignored, with one warning naming the keys", (t) => {
  const ws = workspace(t, { "a.ttl": "<http://x/a> <http://x/b> <http://x/c> .\n", "turtle-lsp.json": '{"query":{"maxRows":0,"timeoutSeconds":0},"shacl":{"maxTriples":999999999}}' })
  const r = run(ws, ["check"])
  assert.equal(r.code, 0, r.err)
  const warnings = r.err.split("\n").filter((l) => l.includes("warning"))
  assert.equal(warnings.length, 1, r.err)
  for (const k of ["query.maxRows", "query.timeoutSeconds", "shacl.maxTriples"]) assert.ok(warnings[0].includes(k), warnings[0])
})
