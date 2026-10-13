// Resource hygiene: the server must cost nothing while idle and must never
// outlive its editor, however the session ends.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { spawn } from "node:child_process"
import { startServer, uriOf, fixtures, serverPath } from "./lsp-client.mjs"
import { cpuSeconds } from "./procs.mjs"

const ws = path.join(fixtures, "workspace")
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const frame = (o) => {
  const body = JSON.stringify(o)
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`
}

// A server driven by raw frames, so a test can end the session any way it likes.
function rawServer(cwd = ws) {
  const proc = spawn(serverPath, [], { cwd, stdio: ["pipe", "ignore", "ignore"] })
  proc.stdin.on("error", () => {})
  const exited = new Promise((r) => proc.on("exit", (code) => r(code)))
  const send = (o) => proc.stdin.write(frame(o))
  send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { capabilities: {}, rootUri: uriOf(cwd) } })
  send({ jsonrpc: "2.0", method: "initialized", params: {} })
  const exitCode = async (ms = 5000) => {
    const code = await Promise.race([exited, sleep(ms).then(() => "running")])
    if (code === "running") proc.kill()
    return code
  }
  return { proc, send, exitCode }
}

test("an idle server uses no CPU", async () => {
  const s = await startServer()
  try {
    const uri = uriOf(path.join(ws, "people.ttl"))
    s.open(uri, fs.readFileSync(path.join(ws, "people.ttl"), "utf8"))
    await s.diagnosticsFor(uri)
    // The first request waits for the workspace index, so it is built by now.
    await s.request("textDocument/hover", { textDocument: { uri }, position: { line: 0, character: 0 } })
    await sleep(300)
    const before = cpuSeconds(s.proc.pid)
    await sleep(2000)
    const used = cpuSeconds(s.proc.pid) - before
    assert.ok(used < 0.1, `idle server used ${used.toFixed(2)} s of CPU in 2 s`)
  } finally {
    await s.stop()
  }
})

test("shutdown then exit ends the process with code 0", async () => {
  const s = rawServer()
  await sleep(200)
  s.send({ jsonrpc: "2.0", id: 2, method: "shutdown" })
  s.send({ jsonrpc: "2.0", method: "exit" })
  assert.equal(await s.exitCode(), 0)
})

test("exit without shutdown ends the process with code 1", async () => {
  const s = rawServer()
  await sleep(200)
  s.send({ jsonrpc: "2.0", method: "exit" })
  assert.equal(await s.exitCode(), 1)
})

test("the server exits when the editor goes away without shutdown", async () => {
  // A crashed or killed editor just closes the pipe.
  const s = rawServer()
  await sleep(300)
  s.proc.stdin.end()
  assert.notEqual(await s.exitCode(), "running", "server still running 5 s after stdin closed")
})

test("the server exits when the editor goes away mid-request", async () => {
  const s = rawServer()
  const uri = uriOf(path.join(ws, "__resources-big.ttl"))
  let text = "@prefix ex: <http://example.org/> .\n"
  for (let i = 0; i < 20000; i++) text += `ex:s${i} ex:p ex:o${i} .\n`
  s.send({ jsonrpc: "2.0", method: "textDocument/didOpen", params: { textDocument: { uri, languageId: "turtle", version: 1, text } } })
  s.send({ jsonrpc: "2.0", id: 2, method: "textDocument/documentSymbol", params: { textDocument: { uri } } })
  s.proc.stdin.end()
  assert.notEqual(await s.exitCode(), "running", "server still running 5 s after stdin closed")
})

// owl:imports are resolved on a background thread: it must block while idle,
// and a fetch in flight must not keep the process alive.
test("the imports thread costs nothing while idle, and a fetch in flight does not delay exit", async () => {
  const http = await import("node:http")
  const os = await import("node:os")
  // Answers nothing: a fetch hangs until its timeout.
  const pending = []
  const web = http.createServer((req, res) => pending.push(res))
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  const cache = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-resources-cache-"))
  process.env.TURTLE_LSP_CACHE_DIR = cache
  process.env.NO_PROXY = "127.0.0.1"
  try {
    const iri = `http://127.0.0.1:${web.address().port}/slow`
    const text = `<http://example.com/ont/a> <http://www.w3.org/2002/07/owl#imports> <http://example.com/ont/missing> .\n`
    const s = await startServer()
    try {
      const uri = uriOf(path.join(ws, "__resources-imports.ttl"))
      s.open(uri, text)
      await s.request("workspace/symbol", { query: "" })
      // The unresolved import is reported once the thread has answered.
      const unresolved = async () => (await s.diagnosticsFor(uri)).some((x) => x.code === "unresolved-import")
      for (let i = 0; i < 100 && !(await unresolved()); i++) await sleep(50)
      assert.ok(await unresolved(), "no unresolved-import diagnostic")
      await sleep(300)
      const before = cpuSeconds(s.proc.pid)
      await sleep(2000)
      const used = cpuSeconds(s.proc.pid) - before
      assert.ok(used < 0.1, `idle server with imports used ${used.toFixed(2)} s of CPU in 2 s`)
    } finally {
      await s.stop()
    }

    const s2 = await startServer({ initializationOptions: { imports: { network: true } } })
    const uri = uriOf(path.join(ws, "__resources-slow.ttl"))
    s2.open(uri, `<http://example.com/ont/a> <http://www.w3.org/2002/07/owl#imports> <${iri}> .\n`)
    await s2.request("workspace/symbol", { query: "" })
    for (let i = 0; i < 100 && pending.length === 0; i++) await sleep(20)
    assert.equal(pending.length, 1, "the import was not requested")
    // stop() fails if the process outlives shutdown/exit by 3 s; the fetch would take 10 s.
    await s2.stop()
  } finally {
    pending.forEach((r) => r.destroy())
    web.close()
    fs.rmSync(cache, { recursive: true, force: true })
  }
})

test("an idle server uses no CPU with shapes present, after a validation run", async () => {
  const project = path.join(fixtures, "project")
  const s = await startServer({ cwd: project })
  try {
    // Not on disk: an open document with an asset that violates the project shapes.
    const uri = uriOf(path.join(project, "data", "__resources-shacl.ttl"))
    s.open(uri, 'obj:x a sd:Asset ;\n    sd:designation "bad" .\n')
    s.notify("textDocument/didSave", { textDocument: { uri } })
    const end = Date.now() + 5000
    while (!(await s.diagnosticsFor(uri)).some((d) => d.code === "shacl-violation")) {
      assert.ok(Date.now() < end, "no SHACL result within 5 s")
      await sleep(25)
    }
    await sleep(300)
    const before = cpuSeconds(s.proc.pid)
    await sleep(2000)
    const used = cpuSeconds(s.proc.pid) - before
    assert.ok(used < 0.1, `idle server used ${used.toFixed(2)} s of CPU in 2 s`)
  } finally {
    await s.stop()
  }
})

test("the server exits when the editor goes away during a validation", async () => {
  // Validating a 10,000-node chain with an ex:next* path takes seconds.
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-shacl-exit-"))
  let chain = "@prefix ex: <http://example.com/chain/> .\n@prefix sh: <http://www.w3.org/ns/shacl#> .\n"
  chain += "ex:S sh:targetSubjectsOf ex:next ; sh:property [ sh:path [ sh:zeroOrMorePath ex:next ] ; sh:minCount 1 ] .\n"
  for (let i = 0; i < 10000; i++) chain += `ex:n${i} ex:next ex:n${i + 1} .\n`
  fs.writeFileSync(path.join(dir, "chain.ttl"), chain)
  try {
    const s = rawServer(dir)
    // The validation starts once the initial index is merged.
    s.send({ jsonrpc: "2.0", id: 2, method: "workspace/symbol", params: { query: "" } })
    await sleep(500)
    s.proc.stdin.end()
    assert.notEqual(await s.exitCode(), "running", "server still running 5 s after stdin closed")
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test("deeply nested input is an error, not a crash (open file and indexed file)", async () => {
  const deep = (n) => `@prefix ex: <http://example.org/> .\nex:s ex:p ${"[ ex:p ".repeat(n)}ex:o${" ]".repeat(n)} .\nex:t ex:p ( ${"( ".repeat(n)}${")".repeat(n)} ) .\n`
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-deep-"))
  // The workspace index parses this one on a worker thread with a small stack.
  fs.writeFileSync(path.join(dir, "deep.ttl"), deep(20000))
  try {
    const s = await startServer({ cwd: dir })
    try {
      await s.request("workspace/symbol", { query: "" })
      const uri = uriOf(path.join(dir, "__open-deep.ttl"))
      s.open(uri, deep(20000))
      const diags = await s.diagnosticsFor(uri, 20000)
      assert.ok(diags.some((d) => /Nested more than/.test(d.message)), JSON.stringify(diags.slice(0, 3)))
      const edits = await s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } })
      assert.deepEqual(edits, [])
    } finally {
      await s.stop()
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test("a long chain of sh:node references validates without crashing", async () => {
  let shapes = "@prefix ex: <http://example.com/chain/> .\n@prefix sh: <http://www.w3.org/ns/shacl#> .\n"
  shapes += "ex:S0 a sh:NodeShape ; sh:targetClass ex:C .\nex:x a ex:C .\n"
  for (let i = 0; i < 5000; i++) shapes += `ex:S${i} sh:node ex:S${i + 1} .\n`
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-chain-"))
  fs.writeFileSync(path.join(dir, "shapes.ttl"), shapes)
  try {
    const s = await startServer({ cwd: dir })
    try {
      const uri = uriOf(path.join(dir, "shapes.ttl"))
      s.open(uri, shapes)
      s.notify("textDocument/didSave", { textDocument: { uri } })
      await sleep(1500)
      // Still answering: the SHACL thread did not take the process down.
      assert.ok(await s.request("workspace/symbol", { query: "S1" }))
    } finally {
      await s.stop()
    }
  } finally {
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
