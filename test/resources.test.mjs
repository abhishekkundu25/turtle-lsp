// Resource hygiene: the server must cost nothing while idle and must never
// outlive its editor, however the session ends.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
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
function rawServer() {
  const proc = spawn(serverPath, [], { cwd: ws, stdio: ["pipe", "ignore", "ignore"] })
  proc.stdin.on("error", () => {})
  const exited = new Promise((r) => proc.on("exit", (code) => r(code)))
  const send = (o) => proc.stdin.write(frame(o))
  send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { capabilities: {}, rootUri: uriOf(ws) } })
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
