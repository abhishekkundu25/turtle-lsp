// `turtle-lsp check --min-severity` and `--max-problems`: what is printed,
// what is counted, the stderr line about the problems not shown, and the
// exit code. Only stdout, stderr and the exit code are asserted.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, tempDir } from "./lsp-client.mjs"

// A workspace with `files` ({relative path: text}); removed after the test.
function workspace(t, files) {
  const ws = tempDir("turtle-cli-budget-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  fs.mkdirSync(path.join(ws, ".cache"))
  for (const [name, text] of Object.entries(files)) {
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
  const lines = (s) => s.split("\n").filter(Boolean)
  return { code: r.status, out: r.stdout, err: r.stderr, lines: lines(r.stdout), errLines: lines(r.stderr) }
}

const PREFIXES = `@prefix ex: <http://example.org/> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
`
// A hint: a prefix declared and never used.
const UNUSED = "@prefix un: <http://example.org/unused#> .\n"
// A warning: a term the RDFS vocabulary does not define.
const WARNING = 'ex:a rdfs:labl "A" .\n'
// An error: an undeclared prefix.
const ERROR = "ex:b ex:p foo:c .\n"

const severities = (lines) => lines.map((l) => l.match(/: (error|warning|info|hint)[[:]/)[1])

// `n` undeclared-prefix errors, one a line.
function errors(n) {
  let text = ""
  for (let i = 0; i < n; i++) text += `<http://example.org/s${i}> foo:p "${i}" .\n`
  return text
}

test("check --min-severity: what is below it is not printed, counted or failed on", (t) => {
  const ws = workspace(t, { "a.ttl": PREFIXES + UNUSED + WARNING + ERROR })
  const every = run(ws, ["check"])
  assert.equal(every.code, 1, every.err)
  assert.deepEqual(severities(every.lines).sort(), ["error", "hint", "warning"])

  const errorsOnly = run(ws, ["check", "--min-severity", "error"])
  assert.equal(errorsOnly.code, 1)
  assert.deepEqual(severities(errorsOnly.lines), ["error"])
  assert.equal(errorsOnly.err.trim(), "1 error, 0 warnings in 1 file")

  const json = JSON.parse(run(ws, ["check", "--format", "json", "--min-severity=warning"]).out)
  assert.deepEqual(json.files[0].diagnostics.map((d) => d.severity).sort(), ["error", "warning"])
  assert.equal(json.summary.hints, 0)

  const stats = run(ws, ["check", "--statistics", "--min-severity", "error"])
  assert.deepEqual(stats.lines, ["  1  undeclared-prefix", "  1  total"])
})

test("check --min-severity warning: the warnings left still fail --max-warnings 0; hints alone pass", (t) => {
  const warned = workspace(t, { "a.ttl": PREFIXES + UNUSED + WARNING })
  const r = run(warned, ["check", "--min-severity", "warning", "--max-warnings", "0"])
  assert.equal(r.code, 1, r.err)
  assert.deepEqual(severities(r.lines), ["warning"])
  assert.equal(r.err.trim(), "0 errors, 1 warning in 1 file")

  const hinted = workspace(t, { "a.ttl": "@prefix ex: <http://example.org/> .\n" })
  assert.equal(run(hinted, ["check"]).lines.length, 1, "the unused prefix is a hint")
  const quiet = run(hinted, ["check", "--min-severity", "warning"])
  assert.equal(quiet.code, 0, quiet.err)
  assert.equal(quiet.out, "")
  assert.equal(quiet.err, "")
})

test("check --min-severity: an unknown severity is an error naming the four", (t) => {
  const ws = workspace(t, { "a.ttl": PREFIXES })
  const r = run(ws, ["check", "--min-severity", "fatal"])
  assert.equal(r.code, 2)
  assert.equal(r.out, "")
  assert.match(r.err, /--min-severity must be error, warning, info or hint, not 'fatal'/)
})

test("check: the concise format prints 50 problems by default, and says how to see the rest", (t) => {
  const ws = workspace(t, { "a.ttl": errors(150) })
  const r = run(ws, ["check"])
  assert.equal(r.code, 1)
  assert.equal(r.lines.length, 50)
  assert.ok(r.lines.every((l, i) => l.startsWith(`a.ttl:${i + 1}:`)), r.lines.slice(0, 3).join("\n"))
  assert.deepEqual(r.errLines, [
    "150 errors, 0 warnings in 1 file",
    "100 more problems not shown (--max-problems 50); narrow the run with a path, --select or --min-severity, or pass --max-problems 0",
  ])
  assert.equal(run(ws, ["check"]).out, r.out, "the same problems on every run")

  const all = run(ws, ["check", "--max-problems", "0"])
  assert.equal(all.code, 1)
  assert.equal(all.lines.length, 150)
  assert.deepEqual(all.errLines, ["150 errors, 0 warnings in 1 file"])

  const ten = run(ws, ["check", "--max-problems=10"])
  assert.equal(ten.code, 1)
  assert.deepEqual(ten.lines, r.lines.slice(0, 10))
  assert.match(ten.errLines.at(-1), /^140 more problems not shown \(--max-problems 10\);/)

  const stats = run(ws, ["check", "--statistics"])
  assert.deepEqual(stats.lines, ["  150  undeclared-prefix", "  150  total"])
  assert.deepEqual(stats.errLines, ["150 errors, 0 warnings in 1 file"])
  // The statistics count every problem: no cap applies to them.
  const statsTen = run(ws, ["check", "--statistics", "--max-problems", "10"])
  assert.deepEqual(statsTen.lines, ["  150  undeclared-prefix", "  150  total"])
  assert.deepEqual(statsTen.errLines, ["150 errors, 0 warnings in 1 file"])

  // The default cap is for stdout: a report written to a file is complete.
  const file = path.join(ws, "report.txt")
  const toFile = run(ws, ["check", "--output-file", file])
  assert.equal(toFile.code, 1)
  assert.equal(fs.readFileSync(file, "utf8").trim().split("\n").length, 150)
  assert.ok(!toFile.err.includes("not shown"), toFile.err)
})

test("check --format json: no cap by default; with --max-problems, summary.notShown counts the rest", (t) => {
  const ws = workspace(t, { "a.ttl": errors(150) })
  const r = run(ws, ["check", "--format", "json"])
  const all = JSON.parse(r.out)
  assert.equal(all.files[0].diagnostics.length, 150)
  assert.equal(all.summary.notShown, 0)
  assert.equal(r.err, "")

  const capped = run(ws, ["check", "--format", "json", "--max-problems", "10"])
  const ten = JSON.parse(capped.out)
  assert.equal(ten.files[0].diagnostics.length, 10)
  assert.equal(ten.summary.notShown, 140)
  assert.equal(ten.summary.errors, 150)
  assert.equal(capped.code, r.code)
  assert.equal(capped.code, 1)
  assert.match(capped.err, /^140 more problems not shown \(--max-problems 10\);/)
})

test("check: --max-problems caps the run after --max-diagnostics caps each file", (t) => {
  const ws = workspace(t, { "a.ttl": errors(4), "b.ttl": errors(4), "c.ttl": errors(4) })
  const r = run(ws, ["check", "--max-diagnostics", "2", "--max-problems", "5"])
  assert.equal(r.code, 1)
  assert.deepEqual(
    r.lines.map((l) => l.split(":").slice(0, 2).join(":")),
    ["a.ttl:1", "a.ttl:2", "b.ttl:1", "b.ttl:2", "c.ttl:1"],
  )
  assert.deepEqual(r.errLines, [
    "12 errors, 0 warnings in 3 files",
    "7 more problems not shown (--max-diagnostics 2, --max-problems 5); narrow the run with a path, --select or --min-severity, or drop --max-diagnostics and pass --max-problems 0",
  ])

  const perFile = run(ws, ["check", "--max-diagnostics", "2"])
  assert.equal(perFile.lines.length, 6)
  assert.equal(
    perFile.errLines.at(-1),
    "6 more problems not shown (--max-diagnostics 2); narrow the run with a path, --select or --min-severity, or drop --max-diagnostics",
  )
})

test("check: the cap keeps the most severe problems, so an error is never hidden behind hints", (t) => {
  // 120 unused prefixes (hints) in a.ttl come before z.ttl's one error.
  let a = "@prefix ex: <http://example.org/> .\n"
  for (let i = 1; i <= 120; i++) a += `@prefix u${i}: <http://example.org/u${i}/> .\n`
  a += "ex:a ex:p ex:b .\n"
  const ws = workspace(t, { "a.ttl": a, "z.ttl": "@prefix ex: <http://example.org/> .\nex:z ex:p bad:x .\n" })
  const r = run(ws, ["check"])
  assert.equal(r.code, 1)
  assert.equal(r.lines.length, 50)
  // The error is printed, last, in its usual place in the report.
  assert.match(r.lines.at(-1), /^z\.ttl:2:\d+: error\[undeclared-prefix\]/)
  assert.ok(r.lines.slice(0, -1).every((l) => l.startsWith("a.ttl:")), r.lines.slice(0, 3).join("\n"))
  // Per file too: --max-diagnostics keeps a file's errors before its hints.
  const one = run(ws, ["check", "--max-diagnostics", "1", "z.ttl", "a.ttl"])
  assert.equal(one.lines.filter((l) => l.startsWith("z.ttl:")).length, 1)
  assert.match(one.lines.find((l) => l.startsWith("z.ttl:")), /error\[undeclared-prefix\]/)
})
