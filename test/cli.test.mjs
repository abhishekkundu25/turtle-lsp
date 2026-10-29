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
import { pathToFileURL } from "node:url"
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
function run(ws, args, { cwd = ws, cache = path.join(ws, ".cache"), env = {} } = {}) {
  const r = spawnSync(serverPath, args, {
    cwd,
    encoding: "utf8",
    timeout: 60000,
    env: { ...process.env, TURTLE_LSP_CACHE_DIR: cache, NO_COLOR: "1", ...env },
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

test("check: every syntax error is reported, past the editor's 1,000 (text, --format json and the summary)", (t) => {
  let text = ""
  for (let i = 0; i < 1500; i++) text += `<http://e/s${i}> <http://e/p> <http://e/o${i}> . <http://e/t${i}> <http://e/p> .\n`
  const ws = workspace(t, { "broken.ttl": text })
  const r = run(ws, ["check"])
  assert.equal(r.code, 1)
  assert.equal(r.lines.length, 1500)
  assert.ok(r.lines.every((l, i) => l.startsWith(`broken.ttl:${i + 1}:`) && l.endsWith("error[syntax] Expected an object, found '.'")), r.lines.slice(0, 3).join("\n"))
  assert.equal(r.err.trim(), "1500 errors, 0 warnings in 1 file")
  const json = JSON.parse(run(ws, ["check", "--format", "json"]).out)
  const problems = json.files[0].diagnostics
  assert.equal(problems.length, 1500)
  assert.ok(problems.every((p) => p.code === "syntax" && !p.message.startsWith("More than")))
  assert.deepEqual(problems[1499].range.start.line, 1500)
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

// ---- robustness: what a CI gate and a write-back tool must survive ----

const writeBytes = (ws, name, bytes) => {
  fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
  fs.writeFileSync(path.join(ws, name), bytes)
}
// A process running as root can write a read-only file.
const root = process.platform === "win32" || process.getuid?.() === 0
const BROKEN = "ex:a ex:b ex:c .\nex:d ex:e ex:f\n"

test("check: a file that is not UTF-8 is an error of its own and the run goes on (exit 2)", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "a.ttl": CLEAN, "z-broken.ttl": BROKEN })
  writeBytes(ws, "m-binary.ttl", Buffer.from([0x65, 0x78, 0x3a, 0xff, 0xfe, 0x0a]))
  const r = run(ws, ["check"])
  assert.equal(r.code, 2, r.err)
  // The good files are still checked and reported, in path order.
  assert.deepEqual(r.lines.map((l) => l.split(" ")[0]), ["m-binary.ttl:1:1:", "z-broken.ttl:2:11:"])
  assert.match(r.lines[0], /^m-binary\.ttl:1:1: error\[encoding\] .*not valid UTF-8/)
  assert.match(r.err, /2 errors, 0 warnings in 2 files/)
  // In the other formats too.
  const j = JSON.parse(run(ws, ["check", "--format", "json"]).out)
  assert.deepEqual(j.files.map((f) => [f.path, f.diagnostics[0].code]), [["m-binary.ttl", "encoding"], ["z-broken.ttl", "syntax"]])
  assert.equal(j.summary.filesChecked, 4)
  // An unreadable file the user names is reported the same way; --ignore hides
  // the line, not the failure.
  const named = run(ws, ["check", "m-binary.ttl", "--ignore", "encoding"])
  assert.equal(named.code, 2)
  assert.equal(named.out, "")
})

test("check: a name that is a folder or cannot be read is an io error, not an abort", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "a.ttl": CLEAN })
  fs.mkdirSync(path.join(ws, "dir.ttl"))
  const r = run(ws, ["check", "a.ttl", "dir.ttl"])
  // A folder with a file name is walked, not read: nothing to report there.
  assert.equal(r.code, 0, r.err)
  if (!root && process.platform !== "win32") {
    writeBytes(ws, "locked.ttl", CLEAN)
    fs.chmodSync(path.join(ws, "locked.ttl"), 0o000)
    const l = run(ws, ["check"])
    fs.chmodSync(path.join(ws, "locked.ttl"), 0o644)
    assert.equal(l.code, 2, l.err)
    assert.match(l.lines.join("\n"), /^locked\.ttl:1:1: error\[io\] /m)
  }
})

test("query: 200 nested REGEX( in a FILTER is a syntax error at once", (t) => {
  // Each level doubled spargebra's parse before its grammar was left-factored.
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "a.ttl": CLEAN })
  const started = Date.now()
  const r = run(ws, ["query", "-e", `SELECT * WHERE { ?x ?p ?o FILTER(${"REGEX(".repeat(200)}?x) }`])
  assert.equal(r.code, 2, r.err)
  assert.equal(r.out, "")
  assert.match(r.err, /^-e:1:\d+: .*syntax error/)
  assert.ok(Date.now() - started < 10000, `${Date.now() - started} ms`)
})

test("format: a file that is not UTF-8 is not formatted, the others are (exit 2)", (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED })
  writeBytes(ws, "bad.ttl", Buffer.from([0xc3, 0x28, 0x0a]))
  const r = run(ws, ["format"])
  assert.equal(r.code, 2)
  assert.match(r.err, /^bad\.ttl: not formatted: not valid UTF-8/m)
  assert.notEqual(read(ws, "a.ttl"), UNFORMATTED)
  assert.deepEqual([...fs.readFileSync(path.join(ws, "bad.ttl"))], [0xc3, 0x28, 0x0a])
})

test("format: a read-only file is reported and left alone; the run finishes and exits 2", { skip: root }, (t) => {
  const ws = workspace(t, { "a.ttl": UNFORMATTED, "b-readonly.ttl": UNFORMATTED, "c.ttl": UNFORMATTED })
  fs.chmodSync(path.join(ws, "b-readonly.ttl"), 0o444)
  const r = run(ws, ["format"])
  assert.equal(r.code, 2)
  assert.match(r.err, /^b-readonly\.ttl: not formatted: is read-only/m)
  assert.match(r.err, /^2 files formatted$/m)
  assert.match(r.err, /^1 file could not be written$/m)
  assert.equal(read(ws, "b-readonly.ttl"), UNFORMATTED)
  assert.notEqual(read(ws, "a.ttl"), UNFORMATTED)
  assert.notEqual(read(ws, "c.ttl"), UNFORMATTED, "the files after the failure are formatted")
  assert.equal(fs.statSync(path.join(ws, "b-readonly.ttl")).mode & 0o777, 0o444)
})

test("check --fix: a file that cannot be written does not stop the others; exit 2", { skip: root }, (t) => {
  const text = "<http://x/a> rdfs:label \"a\" .\n"
  const ws = workspace(t, { "a.ttl": text, "b-readonly.ttl": text, "c.ttl": text })
  fs.chmodSync(path.join(ws, "b-readonly.ttl"), 0o444)
  const r = run(ws, ["check", "--fix"])
  assert.equal(r.code, 2, r.err)
  assert.match(r.err, /^b-readonly\.ttl: not fixed: is read-only/m)
  assert.match(r.err, /^Fixed 2 files; 1 file could not be written$/m)
  assert.equal(read(ws, "b-readonly.ttl"), text)
  for (const f of ["a.ttl", "c.ttl"]) assert.match(read(ws, f), /^@prefix rdfs:/)
  // What could not be fixed is still reported.
  assert.match(r.out, /^b-readonly\.ttl:1:14: error\[undeclared-prefix\]/m)
})

test("format: a symbolic link keeps pointing at its file, and the file's mode is kept", { skip: process.platform === "win32" }, (t) => {
  const ws = workspace(t, { "real/a.ttl": UNFORMATTED })
  fs.symlinkSync(path.join(ws, "real", "a.ttl"), path.join(ws, "link.ttl"))
  fs.chmodSync(path.join(ws, "real", "a.ttl"), 0o640)
  const r = run(ws, ["format", "link.ttl"])
  assert.equal(r.code, 0, r.err)
  assert.ok(fs.lstatSync(path.join(ws, "link.ttl")).isSymbolicLink())
  assert.notEqual(read(ws, "real/a.ttl"), UNFORMATTED)
  assert.equal(fs.statSync(path.join(ws, "real", "a.ttl")).mode & 0o777, 0o640)
  // No temporary file stays behind.
  assert.deepEqual(fs.readdirSync(path.join(ws, "real")), ["a.ttl"])
})

test("format and check --fix: CRLF files stay CRLF", (t) => {
  const crlf = UNFORMATTED.replace(/\n/g, "\r\n")
  const ws = workspace(t, { "a.ttl": crlf })
  assert.equal(run(ws, ["format"]).code, 0)
  const out = read(ws, "a.ttl")
  assert.notEqual(out, crlf)
  assert.ok(out.includes("\r\n") && !/[^\r]\n/.test(out), JSON.stringify(out))
})

test("check: a scan that stops at its cap says so and exits 2; what it checks is the first files in path order", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES })
  // The bad files are in the last folders: filesystem order must not matter.
  for (const n of ["a", "b", "c", "d"]) writeBytes(ws, `${n}/f.ttl`, n === "d" ? BROKEN : CLEAN)
  const r = run(ws, ["check"], { env: { TURTLE_LSP_MAX_FILES: "3" } })
  assert.equal(r.code, 2, r.err)
  assert.match(r.err, /warning: \.: more than 3 RDF and SPARQL files; the scan stopped at the first 3 in path order \(raise the limit with TURTLE_LSP_MAX_FILES\)/)
  assert.equal(r.out, "", "the first three in path order are a/f, b/f and c/f")
  // The same run is the same again, and a cap that fits is clean.
  assert.equal(run(ws, ["check"], { env: { TURTLE_LSP_MAX_FILES: "3" } }).err, r.err)
  const ok = run(ws, ["check"], { env: { TURTLE_LSP_MAX_FILES: "5" } })
  assert.equal(ok.code, 1, "now d/f.ttl is checked")
  assert.deepEqual(ok.lines.map((l) => l.split(":")[0]), ["d/f.ttl"])
  // Exactly at the cap (five files) is not a truncation.
  assert.equal(ok.err.includes("turtle-lsp: warning"), false)
  const bad = run(ws, ["check"], { env: { TURTLE_LSP_MAX_FILES: "many" } })
  assert.equal(bad.code, 2)
  assert.match(bad.err, /TURTLE_LSP_MAX_FILES needs a whole number above 0/)
  // format stops the same way, and writes only what it covers.
  const f = run(ws, ["format"], { env: { TURTLE_LSP_MAX_FILES: "1" } })
  assert.equal(f.code, 2)
  assert.match(f.err, /more than 1 RDF and SPARQL files/)
})

test("check: a file over 50 MB is skipped with a warning naming it (exit 2)", (t) => {
  const ws = workspace(t, { "prefixes.ttl": PREFIXES, "a.ttl": BROKEN })
  const big = path.join(ws, "sub", "big.ttl")
  fs.mkdirSync(path.dirname(big))
  fs.closeSync(fs.openSync(big, "w"))
  fs.truncateSync(big, 50 * 1024 * 1024 + 1) // sparse
  const r = run(ws, ["check"])
  assert.equal(r.code, 2, r.err)
  assert.match(r.err, /^turtle-lsp: warning: sub\/big\.ttl: larger than 50 MB, skipped$/m)
  assert.match(r.lines.join("\n"), /^a\.ttl:2:11: error\[syntax\]/)
})

test("check --format sarif: URIs are percent-encoded; a file outside the root is a file: URI without a base", (t) => {
  const ws = workspace(t, { ".git/HEAD": "", "repo/my data/a#b %c é.ttl": BROKEN })
  const outside = tempDir("turtle-cli-outside-")
  t.after(() => fs.rmSync(outside, { recursive: true, force: true }))
  writeBytes(outside, "out side#1.ttl", BROKEN)
  const repo = path.join(ws, "repo")
  const r = run(ws, ["check", "--root", repo, "--format", "sarif", path.join(outside, "out side#1.ttl"), repo], { cwd: repo })
  assert.equal(r.code, 1, r.err)
  const results = JSON.parse(r.out).runs[0].results
  const locs = results.map((x) => x.locations[0].physicalLocation.artifactLocation)
  assert.deepEqual(locs.find((l) => l.uriBaseId), { uri: "my%20data/a%23b%20%25c%20%C3%A9.ttl", uriBaseId: "%SRCROOT%" })
  const abs = locs.find((l) => !l.uriBaseId)
  assert.deepEqual(abs, { uri: pathToFileURL(path.join(outside, "out side#1.ttl")).href })
  assert.match(abs.uri, /out%20side%231\.ttl$/)
})

// How fast `check --statistics` counts them: latency.test.mjs.
test("check: 20,000 undeclared uses are counted; --format json still carries their fixes", (t) => {
  let text = ""
  for (let i = 0; i < 20000; i++) text += `<http://x/s${i}> foaf:name "n${i}" .\n`
  const ws = workspace(t, { "big.ttl": text })
  const stats = run(ws, ["check", "--statistics"])
  assert.equal(stats.code, 1)
  assert.deepEqual(stats.lines, ["  20000  undeclared-prefix", "  20000  total"])

  // Nine megabytes: past what the helper's pipe holds, so to a file.
  run(ws, ["check", "--format", "json", "--output-file", "report.json"])
  const report = JSON.parse(read(ws, "report.json"))
  const problems = report.files[0].diagnostics
  assert.equal(problems.length, 20000)
  assert.match(problems[19999].fixes[0].title, /^Add @prefix foaf:/)
  // The fix's edit is the one declaration, above the first use.
  assert.deepEqual(problems[0].fixes[0].edits[0].range, { start: { line: 1, column: 1 }, end: { line: 1, column: 1 } })
})

test("check --fix: CRLF text with a fix after its last line break keeps every line ending", (t) => {
  const ws = workspace(t, { "a.ttl": '@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\r\n<http://x/a> rdfs:labl "a" ;\r\n  foaf:name "b" .\r\n' })
  const r = run(ws, ["check", "--fix", "--unsafe-fixes"])
  assert.equal(r.code, 0, r.err)
  const fixed = read(ws, "a.ttl")
  assert.equal(fixed.split("\n").length - 1, fixed.split("\r\n").length - 1, JSON.stringify(fixed))
  assert.match(fixed, /rdfs:label "a"/)
  assert.match(fixed, /@prefix foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/> \.\r\n/)
})

test("check: --select and --ignore accept only codes turtle-lsp reports", (t) => {
  const ws = workspace(t, { "a.ttl": BROKEN })
  for (const flag of ["--select", "--ignore"]) {
    const r = run(ws, ["check", flag, "sintax"])
    assert.equal(r.code, 2)
    assert.equal(r.out, "")
    assert.match(r.err, new RegExp(`^turtle-lsp: ${flag}: unknown diagnostic code 'sintax' \\(known codes: syntax, `))
  }
  assert.equal(run(ws, ["check", "--select", "syntax"]).code, 1)
  assert.equal(run(ws, ["check", "--select", "unused-prefix"]).code, 0, "a code that does not occur selects nothing")
  assert.equal(run(ws, ["check", "--ignore", "syntax,undeclared-prefix"]).code, 0)
})

// A workspace whose a.ttl gets an unused-prefix hint (foaf:), an
// undescribed-term warning (ex:Persn: vocab.ttl makes ex: an own namespace)
// and an unknown-term warning (rdfs:labl); `settings` is its turtle-lsp.json.
function lintLists(t, settings) {
  const ws = workspace(t, {
    "vocab.ttl": `${PREFIXES}ex:Person a rdfs:Class .\nex:Place a rdfs:Class .\nex:name a rdf:Property .\nex:city a rdf:Property .\n`,
    "a.ttl": `@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix foaf: <http://xmlns.com/foaf/0.1/> .
ex:bob a ex:Persn ; rdfs:labl "Bob" .
`,
  })
  if (settings !== undefined) fs.writeFileSync(path.join(ws, "turtle-lsp.json"), JSON.stringify(settings))
  return ws
}
const lintCodes = (r) => r.lines.filter((l) => l.startsWith("a.ttl:")).map((l) => l.match(/\[([^\]]+)\]/)[1]).sort()

test("check: lint.ignore in turtle-lsp.json hides codes, --ignore adds to it", (t) => {
  // vocab.ttl's unused sh: prefix is a hint of its own; only a.ttl is compared.
  assert.deepEqual(lintCodes(run(lintLists(t), ["check"])), ["undescribed-term", "unknown-term", "unused-prefix"])
  const ws = lintLists(t, { lint: { ignore: ["unused-prefix"] } })
  assert.deepEqual(lintCodes(run(ws, ["check"])), ["undescribed-term", "unknown-term"])
  assert.deepEqual(lintCodes(run(ws, ["check", "--ignore", "unknown-term"])), ["undescribed-term"])
  // --select cannot bring back what the file ignores.
  assert.deepEqual(lintCodes(run(ws, ["check", "--select", "unused-prefix,unknown-term"])), ["unknown-term"])
  // The file's lint.undescribedTerms: false still works next to the list.
  fs.writeFileSync(path.join(ws, "turtle-lsp.json"), JSON.stringify({ lint: { ignore: ["unused-prefix"], undescribedTerms: false } }))
  const r = run(ws, ["check"])
  assert.deepEqual(lintCodes(r), ["unknown-term"])
  assert.equal(r.err.includes("turtle-lsp.json"), false, r.err)
})

test("check: warnings hidden by lint.ignore and --ignore do not count against --max-warnings", (t) => {
  const ws = lintLists(t, { lint: { ignore: ["undescribed-term"] } })
  assert.equal(run(ws, ["check"]).code, 0, "warnings alone pass")
  // One warning left (unknown-term): over a limit of 0, within 1.
  assert.equal(run(ws, ["check", "--max-warnings=0"]).code, 1)
  assert.equal(run(ws, ["check", "--max-warnings=1"]).code, 0)
  // --ignore hides the last one: no warnings, so a limit of 0 passes.
  const none = run(ws, ["check", "--max-warnings=0", "--ignore", "unknown-term"])
  assert.equal(none.code, 0, none.out)
  assert.deepEqual(lintCodes(none), ["unused-prefix"])
  // Without the file both warnings count.
  fs.rmSync(path.join(ws, "turtle-lsp.json"))
  assert.equal(run(ws, ["check", "--max-warnings=1"]).code, 1)
})

test("check: an off-by-default code is a known code; lint.enable with it or an unknown code changes nothing else", (t) => {
  const ws = lintLists(t)
  for (const flag of ["--select", "--ignore"]) {
    const r = run(ws, ["check", flag, "missing-ontology-license"])
    assert.equal(r.code, 0, `${flag}: ${r.err}`)
  }
  // Not enabled: --select of the code reports nothing.
  assert.deepEqual(run(ws, ["check", "--select", "missing-ontology-license"]).lines, [])
  fs.writeFileSync(
    path.join(ws, "turtle-lsp.json"),
    JSON.stringify({ lint: { enable: ["missing-ontology-license", "unused-prefix", "no-such-code"] } }),
  )
  const r = run(ws, ["check"])
  assert.equal(r.code, 0, r.err)
  assert.deepEqual(lintCodes(r), ["undescribed-term", "unknown-term", "unused-prefix"])
  assert.equal(r.err.includes("turtle-lsp.json"), false, r.err)
})

test("a word that is no command fails instead of starting the server", (t) => {
  const ws = workspace(t, {})
  for (const word of ["chek", "serve", "foo.ttl"]) {
    const r = spawnSync(serverPath, [word], { cwd: ws, encoding: "utf8", timeout: 10000, input: "" })
    assert.equal(r.status, 2, `${word}: ${r.stderr}`)
    assert.equal(r.stdout, "")
    assert.match(r.stderr, new RegExp(`^turtle-lsp: unknown command '${word}'\n\nUsage:`))
  }
})

test("--version prints to stdout; help COMMAND shows that command's options", () => {
  const v = spawnSync(serverPath, ["--version"], { encoding: "utf8", timeout: 10000 })
  assert.equal(v.status, 0)
  assert.match(v.stdout, /^turtle-lsp \d+\.\d+\.\d+\S*\n$/)
  assert.equal(v.stderr, "")
  const help = (...args) => spawnSync(serverPath, args, { encoding: "utf8", timeout: 10000 })
  assert.match(help("help", "check").stdout, /^Usage: turtle-lsp check /)
  assert.match(help("help", "format").stdout, /^Usage: turtle-lsp format /)
  assert.match(help("help", "query").stdout, /^Usage: turtle-lsp query /)
  assert.match(help("help").stdout, /^Usage:\n  turtle-lsp  /)
  const bad = help("help", "nothing")
  assert.equal(bad.status, 2)
  assert.match(bad.stderr, /no help for 'nothing'/)
})

test("check: --statistics cannot be combined with --format; '-' (standard input) is rejected", (t) => {
  const ws = workspace(t, { "a.ttl": CLEAN })
  const s = run(ws, ["check", "--statistics", "--format", "json"])
  assert.equal(s.code, 2)
  assert.match(s.err, /--statistics .*drop --format/)
  for (const args of [["check", "-"], ["format", "-"]]) {
    const r = run(ws, args)
    assert.equal(r.code, 2, args.join(" "))
    assert.match(r.err, /'-' \(standard input\) is not supported/)
  }
})

test("format --diff: a file outside the current folder is labelled a/abs/path, not a//abs/path", (t) => {
  const ws = workspace(t, { "work/.keep": "" })
  const file = path.join(ws, "elsewhere.ttl")
  fs.writeFileSync(file, UNFORMATTED)
  const r = run(ws, ["format", "--diff", file], { cwd: path.join(ws, "work") })
  assert.equal(r.code, 1, r.err)
  const label = file.replace(/\\/g, "/").replace(/^\/+/, "")
  const [a, b] = r.out.split("\n")
  assert.equal(a, `--- a/${label}`)
  assert.equal(b, `+++ b/${label}`)
  assert.ok(!r.out.includes("a//"))
})

test("the pre-commit hooks match every extension the tool reads, and format's help lists them", () => {
  const yaml = fs.readFileSync(new URL("../.pre-commit-hooks.yaml", import.meta.url), "utf8")
  const patterns = [...yaml.matchAll(/^ {2}files: (.+)$/gm)].map((m) => new RegExp(m[1]))
  assert.equal(patterns.length, 2)
  for (const re of patterns) {
    for (const ext of ["ttl", "turtle", "trig", "nt", "nq", "jsonld", "rq", "ru", "sparql"]) assert.ok(re.test(`dir/file.${ext}`), ext)
    assert.ok(!re.test("file.json") && !re.test("file.ttl.bak"))
  }
  const help = spawnSync(serverPath, ["format", "--help"], { encoding: "utf8" }).stdout
  assert.match(help, /\.turtle/)
  assert.match(help, /JSON-LD/)
})
