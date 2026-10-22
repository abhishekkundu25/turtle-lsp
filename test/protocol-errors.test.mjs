// Protocol error paths: what the server answers to input an editor should
// never send, and that it stays usable (or ends cleanly) afterwards. Each case
// drives a server by raw frames, as `rawServer` in resources.test.mjs does,
// but keeps what the server writes.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { execFileSync, spawn } from "node:child_process"
import fs from "node:fs"
import { serverPath, serverEnv, tempDir, uriOf } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const frame = (o) => {
  const body = typeof o === "string" ? o : JSON.stringify(o)
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`
}

const ParseError = -32700
const InvalidRequest = -32600
const MethodNotFound = -32601
const InvalidParams = -32602
const ServerNotInitialized = -32002

let cwd
before(() => (cwd = tempDir("turtle-protocol-")))
after(() => fs.rmSync(cwd, { recursive: true, force: true }))

const hoverParams = { textDocument: { uri: "file:///protocol/none.ttl" }, position: { line: 0, character: 0 } }

// A server fed raw bytes. `responses` holds what it has answered, by id.
// Servers still alive when a test ends are killed by `after`.
const started = []
after(() => started.forEach((s) => s.proc.kill()))

function session({ initialize = true } = {}) {
  const proc = spawn(serverPath, [], { cwd, env: serverEnv(), stdio: ["pipe", "pipe", "pipe"] })
  proc.stdin.on("error", () => {})
  let buf = Buffer.alloc(0)
  let stderr = ""
  const responses = new Map()
  const messages = []
  proc.stderr.on("data", (d) => (stderr += d))
  proc.stdout.on("data", (d) => {
    buf = Buffer.concat([buf, d])
    for (;;) {
      const end = buf.indexOf("\r\n\r\n")
      if (end < 0) return
      const len = Number(/Content-Length:\s*(\d+)/i.exec(buf.subarray(0, end).toString())[1])
      if (buf.length < end + 4 + len) return
      const msg = JSON.parse(buf.subarray(end + 4, end + 4 + len).toString())
      buf = buf.subarray(end + 4 + len)
      messages.push(msg)
      if (msg.method === undefined && msg.id !== undefined) responses.set(msg.id, msg)
    }
  })
  const exited = new Promise((r) => proc.once("exit", (code, signal) => r({ code, signal })))
  const s = {
    proc,
    messages,
    get stderr() {
      return stderr
    },
    write: (chunk) => proc.stdin.write(chunk),
    send: (o) => proc.stdin.write(frame(o)),
    request: (id, method, params) => s.send({ jsonrpc: "2.0", id, method, params }),
    notify: (method, params) => s.send({ jsonrpc: "2.0", method, params }),
    async response(id, ms = 5000) {
      const end = Date.now() + ms
      while (!responses.has(id)) {
        assert.ok(Date.now() < end, `no response to ${id} within ${ms} ms; got ${JSON.stringify([...responses.values()])}`)
        await sleep(10)
      }
      return responses.get(id)
    },
    async noResponse(id, ms = 300) {
      await sleep(ms)
      assert.ok(!responses.has(id), `unexpected response to ${id}: ${JSON.stringify(responses.get(id))}`)
    },
    init: () => s.request("init", "initialize", { capabilities: {}, rootUri: uriOf(cwd) }),
    async initialize() {
      s.init()
      assert.equal((await s.response("init")).error, undefined)
      s.notify("initialized", {})
    },
    // The server still answers: a hover on a document it has never seen is null.
    async answersHover(id = "hover") {
      s.request(id, "textDocument/hover", hoverParams)
      const r = await s.response(id)
      assert.equal(r.error, undefined, `hover failed: ${JSON.stringify(r.error)}`)
      assert.equal(r.result, null)
    },
    // The ends the protocol asks for: `shutdown` answered, `exit` ends the process with 0.
    async endsCleanly() {
      s.request("shutdown", "shutdown", null)
      assert.equal((await s.response("shutdown")).error, undefined)
      s.notify("exit", null)
      const end = await s.exit()
      assert.deepEqual(end, { code: 0, signal: null }, `stderr: ${stderr}`)
    },
    // How the process ended, within `ms`.
    async exit(ms = 5000) {
      const end = await Promise.race([exited, sleep(ms).then(() => "running")])
      assert.notEqual(end, "running", `the server was still running ${ms} ms later`)
      assert.ok(!/panicked/.test(stderr), `the server panicked:\n${stderr}`)
      return end
    },
  }
  started.push(s)
  return initialize ? s.initialize().then(() => s) : s
}

// Rejected requests: the right error, then the session carries on.
const rejected = [
  ["an unknown method", "foo/bar", {}, MethodNotFound],
  ["an unknown method under $/", "$/foo", {}, MethodNotFound],
  ["params of the wrong shape", "textDocument/hover", { nope: 1 }, InvalidParams],
  ["params of the wrong type", "textDocument/hover", [1, 2], InvalidParams],
  ["params missing", "textDocument/hover", undefined, InvalidParams],
  ["null params", "textDocument/hover", null, InvalidParams],
  ["a position of the wrong type", "textDocument/hover", { ...hoverParams, position: { line: "0", character: 0 } }, InvalidParams],
  ["a negative position", "textDocument/hover", { ...hoverParams, position: { line: -1, character: 0 } }, InvalidParams],
  ["a second initialize", "initialize", { capabilities: {}, rootUri: null }, undefined],
]
for (const [what, method, params, code] of rejected) {
  test(`${what} is answered with an error, and the session carries on`, async () => {
    const s = await session()
    s.request("bad", method, params)
    const r = await s.response("bad")
    assert.ok(r.error, `no error: ${JSON.stringify(r)}`)
    assert.equal(typeof r.error.message, "string")
    if (code !== undefined) assert.equal(r.error.code, code, r.error.message)
    await s.answersHover()
    await s.endsCleanly()
  })
}

test("an unknown notification is ignored", async () => {
  const s = await session()
  s.notify("foo/bar", { x: 1 })
  s.notify("$/foo", null)
  s.notify("textDocument/didOpen", { nope: true })
  s.notify("textDocument/didChange", { textDocument: { uri: "file:///protocol/none.ttl", version: 2 }, contentChanges: [{ text: "x" }] })
  await s.answersHover()
  await s.endsCleanly()
})

test("a response to a request the server never made is ignored", async () => {
  const s = await session()
  s.send({ jsonrpc: "2.0", id: 77, result: null })
  s.send({ jsonrpc: "2.0", id: "nope", error: { code: -32603, message: "x" } })
  await s.answersHover()
  await s.endsCleanly()
})

test("a request is answered under its own id, whether a string or a number", async () => {
  const s = await session()
  for (const id of ["a b", 0, -1, 2 ** 31 - 1]) {
    s.request(id, "foo/bar", {})
    assert.equal((await s.response(id)).error.code, MethodNotFound)
  }
  await s.answersHover()
  await s.endsCleanly()
})

test("requests before initialize get ServerNotInitialized; initialize then works", async () => {
  const s = await session({ initialize: false })
  s.request("early", "textDocument/hover", hoverParams)
  const r = await s.response("early")
  assert.equal(r.error?.code, ServerNotInitialized, JSON.stringify(r))
  s.notify("textDocument/didOpen", { nope: true })
  s.request("early2", "foo/bar", {})
  assert.equal((await s.response("early2")).error?.code, ServerNotInitialized)
  await s.initialize()
  await s.answersHover()
  await s.endsCleanly()
})

test("a request after shutdown is refused; exit still ends the process with 0", async () => {
  const s = await session()
  s.request("shutdown", "shutdown", null)
  assert.equal((await s.response("shutdown")).error, undefined)
  s.request("late", "textDocument/hover", hoverParams)
  assert.equal((await s.response("late")).error?.code, InvalidRequest)
  s.notify("exit", null)
  assert.deepEqual(await s.exit(), { code: 0, signal: null })
})

test("exit without shutdown ends the process with 1", async () => {
  const s = await session()
  s.notify("exit", null)
  assert.deepEqual(await s.exit(), { code: 1, signal: null })
})

test("closing stdin ends the process", async () => {
  const s = await session()
  await s.answersHover()
  s.proc.stdin.end()
  const end = await s.exit()
  assert.equal(end.signal, null)
})

// Input the transport cannot read. Today each ends the session (exit 1, the
// reason on stderr) without a response; the contract kept here is that it
// neither hangs nor panics, and that what follows is not a crash.
const garbage = [
  ["invalid JSON", frame("{not json"), true],
  ["JSON that is not a message", frame("[1,2]"), true],
  ["a body that is not UTF-8", Buffer.concat([Buffer.from("Content-Length: 4\r\n\r\n"), Buffer.from([0x7b, 0xff, 0xfe, 0x7d])]), true],
  ["a non-numeric Content-Length", "Content-Length: abc\r\n\r\n{}", false],
  ["a negative Content-Length", "Content-Length: -5\r\n\r\n{}", false],
  ["a header block without Content-Length", "X-Other: 1\r\n\r\n{}", false],
  ["a header line without CRLF", "Content-Length: 2\n\n{}", false],
]
for (const [what, bytes, framed] of garbage) {
  test(`${what}: no panic, no hang`, async () => {
    const s = await session()
    s.write(bytes)
    s.request("shutdown", "shutdown", null)
    s.notify("exit", null)
    s.proc.stdin.end()
    const end = await s.exit()
    assert.equal(end.signal, null, `ended by ${end.signal}`)
    assert.ok([0, 1].includes(end.code), `exit code ${end.code}`)
  })
  // What a robust server does with a well-framed body it cannot decode: say so
  // and keep serving. Header damage cannot be recovered from.
  if (framed) {
    test(`${what}: answered with an error, and the session carries on`, { todo: "protocol:undecodable-body" }, async () => {
      const s = await session()
      s.write(bytes)
      await sleep(200)
      const error = s.messages.find((m) => m.error)
      assert.ok(error, "no error response; the server ended the session")
      assert.ok([ParseError, InvalidRequest].includes(error.error.code), JSON.stringify(error))
      await s.answersHover()
      await s.endsCleanly()
    })
  }
}

test("a Content-Length far beyond any message: the client going away ends the process", async () => {
  const s = await session()
  s.write("Content-Length: 4000000\r\n\r\n")
  await sleep(100)
  s.proc.stdin.end()
  const end = await s.exit()
  assert.equal(end.signal, null)
})

// The length is believed and its buffer allocated and zeroed up front.
test("a Content-Length of 512 MiB is refused without allocating it", { todo: "protocol:content-length-limit", skip: process.platform === "win32" }, async () => {
  const s = await session()
  const rss = () => Number(execFileSync("ps", ["-o", "rss=", "-p", String(s.proc.pid)], { encoding: "utf8" })) / 1024
  const before = rss()
  s.write(`Content-Length: ${512 * 1024 * 1024}\r\n\r\n`)
  await sleep(500)
  const grew = rss() - before
  s.proc.kill()
  assert.ok(grew < 64, `the server grew by ${grew.toFixed(0)} MiB for one header`)
})
