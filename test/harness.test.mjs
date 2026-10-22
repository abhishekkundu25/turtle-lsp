// The test harness itself: the pending reporter, the leaked-server and signal
// cleanup of lsp-client.mjs, and the checks nvim.test.mjs makes of its run.
// Each case runs a small fixture (test/fixtures/harness, test/fixtures/pending-reporter)
// in its own `node` process, so a failure there is an assertion here.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { serverPath, tempDir } from "./lsp-client.mjs"
import { processesOf } from "./procs.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const posix = process.platform !== "win32"
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

// An environment in which a nested `node --test` is a run of its own.
function cleanEnv(extra = {}) {
  const env = { ...process.env, ...extra }
  for (const k of Object.keys(env)) if (k.startsWith("NODE_TEST")) delete env[k]
  return env
}

let work
let server
before(() => {
  work = tempDir("turtle-harness-")
  // A private copy, so the process checks see only the servers started here.
  server = path.join(work, path.basename(serverPath))
  fs.copyFileSync(serverPath, server)
  fs.chmodSync(server, 0o755)
})
after(() => fs.rmSync(work, { recursive: true, force: true }))

function nodeTest(args, extra) {
  return spawnSync(process.execPath, ["--test", "--test-timeout=30000", ...args], {
    cwd: path.join(here, ".."),
    env: cleanEnv({ TURTLE_LSP_BIN: server, ...extra }),
    encoding: "utf8",
    timeout: 60000,
  })
}

const reporters = [
  "--test-reporter=spec",
  "--test-reporter-destination=stdout",
  `--test-reporter=${path.join(here, "pending-reporter.mjs")}`,
  "--test-reporter-destination=stdout",
]

test("a pending test that passes is listed and fails the run", () => {
  const r = nodeTest([...reporters, path.join(here, "fixtures/pending-reporter/passing-todo.mjs")])
  assert.match(r.stdout, /pending tests by tag:/)
  assert.match(r.stdout, /fixture:pending: 1/)
  assert.match(r.stdout, /fixture:ready: 1 \(ticket 99: 1\), 1 passing/)
  assert.match(r.stdout, /pending tests that pass, to promote:\n {2}fixture:ready \(ticket 99\): ready to promote/)
  assert.equal(r.status, 1, r.stdout)
})

test("pending tests that all still fail do not fail the run", () => {
  const r = nodeTest([...reporters, path.join(here, "fixtures/pending-reporter/failing-todo.mjs")])
  assert.match(r.stdout, /pending tests by tag:\n {2}fixture:pending: 1\n/)
  assert.doesNotMatch(r.stdout, /to promote/)
  assert.equal(r.status, 0, r.stdout)
})

test("npm run test:node lets the pending reporter print its summary", () => {
  const script = JSON.parse(fs.readFileSync(path.join(here, "..", "package.json"), "utf8")).scripts["test:node"]
  // The process exits before the reporter's last output under --test-force-exit.
  assert.doesNotMatch(script, /--test-force-exit/)
  assert.match(script, /--test-reporter=\.\/test\/pending-reporter\.mjs/)
})

test("a server a test never stops fails its file instead of hanging it", () => {
  const started = Date.now()
  const r = nodeTest([path.join(here, "fixtures/harness/leaky.mjs")])
  assert.ok(Date.now() - started < 30000, "the file hung")
  assert.notEqual(r.status, 0, r.stdout)
  assert.match(r.stdout + r.stderr, /turtle-lsp started .*leaky\.mjs:\d+:\d+\)? was never stopped/)
  assert.deepEqual(processesOf(server), [])
})

test("a server stopped in a file-level hook is not a leak", () => {
  const r = nodeTest([path.join(here, "fixtures/harness/tidy.mjs")])
  assert.equal(r.status, 0, r.stdout + r.stderr)
  assert.deepEqual(processesOf(server), [])
})

for (const signal of ["SIGINT", "SIGTERM"]) {
  test(`${signal} removes the cache root, kills the server and re-raises`, { skip: !posix && "POSIX signals" }, async () => {
    const child = spawn(process.execPath, [path.join(here, "fixtures/harness/signal-client.mjs")], {
      env: cleanEnv({ TURTLE_LSP_BIN: server, TMPDIR: work }),
      stdio: ["ignore", "pipe", "inherit"],
    })
    const exited = new Promise((r) => child.once("exit", (code, sig) => r({ code, sig })))
    let out = ""
    const line = await new Promise((resolve, reject) => {
      child.stdout.on("data", (d) => {
        out += d
        if (out.includes("\n")) resolve(JSON.parse(out))
      })
      exited.then(() => reject(new Error(`the client ended early: ${out}`)))
    }).catch((e) => {
      child.kill("SIGKILL")
      throw e
    })
    assert.ok(fs.existsSync(line.cacheRoot), "no cache root while the server runs")
    assert.ok(line.cacheRoot.startsWith(work), `cache root ${line.cacheRoot} is not under the private TMPDIR`)
    child.kill(signal)
    const end = await Promise.race([exited, sleep(10000).then(() => "hung")])
    if (end === "hung") child.kill("SIGKILL")
    assert.notEqual(end, "hung", `the client ignored ${signal}`)
    assert.equal(end.sig, signal, `the client ended by ${end.sig ?? `exit ${end.code}`}, not ${signal}`)
    assert.ok(!fs.existsSync(line.cacheRoot), `${line.cacheRoot} was left behind`)
    for (let i = 0; processesOf(server).length && i < 100; i++) await sleep(50)
    assert.deepEqual(processesOf(server), [], "the server outlived its client")
  })
}

// nvim.test.mjs against a stand-in `nvim` whose run is scripted.
const spec = path.join(here, "nvim", "spec.lua")
// The count nvim.test.mjs expects: indented checks sit under `if` and run only
// with the local-only `real` fixtures.
const hasReal = fs.existsSync(path.join(here, "fixtures", "real"))
const checksInSpec = () =>
  fs.readFileSync(spec, "utf8").split("\n").filter((l) => (hasReal ? /^\s*check\(/ : /^check\(/).test(l)).length
function fakeNvim(mode) {
  const dir = path.join(work, `nvim-${mode}`)
  fs.mkdirSync(dir, { recursive: true })
  const bin = path.join(dir, "nvim")
  const n = checksInSpec()
  // The stand-in prints `count` ok lines, then ends as `finish` says.
  const script = {
    ok: { count: n, finish: "exit 0" },
    status: { count: n, finish: "exit 3" },
    signal: { count: n, finish: 'kill -9 "$$"' },
    short: { count: 2, finish: "exit 0" },
    plan: { count: n, finish: `echo "1..${n + 1}"; exit 0` },
  }[mode]
  fs.writeFileSync(
    bin,
    `#!/bin/sh
[ "$1" = "--version" ] && { echo "NVIM fake"; exit 0; }
i=0
while [ "$i" -lt ${script.count} ]; do echo "ok - check $i ::"; i=$((i + 1)); done
${script.finish}
`,
  )
  fs.chmodSync(bin, 0o755)
  return dir
}
function nvimTest(mode) {
  const dir = fakeNvim(mode)
  return nodeTest([path.join(here, "nvim.test.mjs")], { PATH: `${dir}${path.delimiter}${process.env.PATH}` })
}

test("nvim.test.mjs passes when nvim reports every check and exits 0", { skip: !posix && "needs sh" }, () => {
  const r = nvimTest("ok")
  assert.equal(r.status, 0, r.stdout + r.stderr)
})
for (const [mode, failing] of [
  ["status", "nvim exits 0, not by a signal"],
  ["signal", "nvim exits 0, not by a signal"],
  ["short", "nvim reports every check"],
  ["plan", "nvim reports every check"],
]) {
  test(`nvim.test.mjs fails when nvim's run is: ${mode}`, { skip: !posix && "needs sh" }, () => {
    const r = nvimTest(mode)
    assert.notEqual(r.status, 0, r.stdout)
    assert.match(r.stdout, new RegExp(`✖ ${failing}`))
  })
}
