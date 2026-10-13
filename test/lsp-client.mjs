import { spawn, spawnSync } from "node:child_process"
import { Transform } from "node:stream"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import { createRequire } from "node:module"
// require() resolves "vscode-jsonrpc/node" in both v8 (node.js file) and v9 (exports map).
const rpc = createRequire(import.meta.url)("vscode-jsonrpc/node")

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
// The server binary under test; override with TURTLE_LSP_BIN.
export const serverPath =
  process.env.TURTLE_LSP_BIN ?? path.join(root, "target", "release", process.platform === "win32" ? "turtle-lsp.exe" : "turtle-lsp")
export const fixtures = path.join(root, "test", "fixtures")

export function uriOf(file) {
  return pathToFileURL(file).toString()
}

// Starts the server as an editor would (stdio, cwd = workspace) and collects
// every notification so tests can wait on diagnostics.
// macOS scans a freshly built binary on first launch; do that outside any timing.
let warmed = false
function warmUp() {
  if (warmed) return
  warmed = true
  spawnSync(serverPath, ["--version"])
}

// `capabilities` and `initializationOptions` go into the initialize request;
// `onRequest(method, params)` answers requests from the server (default null).
export async function startServer({
  cwd = path.join(fixtures, "workspace"),
  rootUri = uriOf(cwd),
  capabilities = {},
  initializationOptions,
  onRequest = () => null,
} = {}) {
  warmUp()
  const proc = spawn(serverPath, [], { cwd, stdio: ["pipe", "pipe", "pipe"] })
  const exited = new Promise((r) => proc.once("exit", () => r(true)))
  // The server may exit before our last write (e.g. after shutdown/exit).
  proc.stdin.on("error", () => {})
  let stderr = ""
  proc.stderr.on("data", (d) => (stderr += d))
  // Strip (and record) any bytes written to stdout outside LSP frames, so one
  // stray console.log can be asserted on instead of hanging every test.
  const stray = []
  // Responses to requests sent with sendRaw (string ids), by id.
  const rawResponses = new Map()
  const frames = framer(stray, rawResponses)
  proc.stdout.pipe(frames)
  const conn = rpc.createMessageConnection(
    new rpc.StreamMessageReader(frames),
    new rpc.StreamMessageWriter(proc.stdin)
  )
  const diagnostics = new Map()
  const waiters = []
  const logs = []
  const published = []
  const serverRequests = []
  const versions = new Map()
  conn.onNotification("textDocument/publishDiagnostics", (p) => {
    published.push(p.uri)
    // A report for an older version than the one last sent (e.g. a republish
    // after the initial index lands) is stale: the next one is the answer.
    if (p.version != null && versions.has(p.uri) && p.version < versions.get(p.uri)) return
    diagnostics.set(p.uri, p.diagnostics)
    for (const w of waiters.splice(0)) w()
  })
  conn.onNotification("window/logMessage", (p) => logs.push(p.message))
  conn.onRequest((method, params) => {
    serverRequests.push({ method, params })
    return onRequest(method, params)
  })
  conn.listen()

  const t0 = performance.now()
  const init = await conn.sendRequest("initialize", {
    processId: process.pid,
    rootUri,
    workspaceFolders: [{ uri: rootUri, name: "ws" }],
    capabilities,
    initializationOptions,
  })
  const initMs = performance.now() - t0
  await conn.sendNotification("initialized", {})

  const api = {
    proc,
    conn,
    init,
    initMs,
    logs,
    stray,
    published,
    serverRequests,
    get stderr() {
      return stderr
    },
    request: (method, params) => conn.sendRequest(method, params),
    notify: (method, params) => conn.sendNotification(method, params),
    // Writes the messages in one chunk, so the server reads them together.
    // Requests need string ids; their responses land in rawResponses.
    sendRaw(messages) {
      proc.stdin.write(messages.map((m) => frame({ jsonrpc: "2.0", ...m })).join(""))
    },
    async rawResponse(id, timeoutMs = 5000) {
      const end = Date.now() + timeoutMs
      while (!rawResponses.has(id)) {
        if (Date.now() > end) throw new Error(`no response to ${id} within ${timeoutMs}ms`)
        await new Promise((r) => setTimeout(r, 10))
      }
      return rawResponses.get(id)
    },
    open(uri, text) {
      versions.set(uri, 1)
      diagnostics.delete(uri)
      conn.sendNotification("textDocument/didOpen", {
        textDocument: { uri, languageId: "turtle", version: 1, text },
      })
    },
    change(uri, text) {
      const v = (versions.get(uri) ?? 1) + 1
      versions.set(uri, v)
      diagnostics.delete(uri)
      conn.sendNotification("textDocument/didChange", {
        textDocument: { uri, version: v },
        contentChanges: [{ text }],
      })
    },
    close(uri) {
      conn.sendNotification("textDocument/didClose", { textDocument: { uri } })
    },
    async diagnosticsFor(uri, timeoutMs = 5000) {
      const end = Date.now() + timeoutMs
      while (!diagnostics.has(uri)) {
        if (Date.now() > end) throw new Error(`no diagnostics for ${uri} within ${timeoutMs}ms`)
        await new Promise((r) => {
          waiters.push(r)
          setTimeout(r, 50)
        })
      }
      return diagnostics.get(uri)
    },
    async stop() {
      try {
        await Promise.race([conn.sendRequest("shutdown"), new Promise((r) => setTimeout(r, 500))])
        await conn.sendNotification("exit").catch(() => {})
      } catch {}
      // Every test session must end the process; a server that lingers after
      // shutdown/exit is a leak, not something to paper over with kill().
      const gone = await Promise.race([exited, new Promise((r) => setTimeout(() => r(false), 3000))])
      conn.dispose()
      if (!gone) {
        proc.kill()
        throw new Error("turtle-lsp did not exit within 3 s of shutdown/exit")
      }
    },
  }
  return api
}

export function positionOf(text, needle, offsetInNeedle = needle.length) {
  const idx = text.indexOf(needle)
  if (idx < 0) throw new Error(`needle not found: ${needle}`)
  const before = text.slice(0, idx + offsetInNeedle).split("\n")
  return { line: before.length - 1, character: before[before.length - 1].length }
}

function frame(message) {
  const body = JSON.stringify(message)
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`
}

function framer(stray, rawResponses) {
  let buf = Buffer.alloc(0)
  return new Transform({
    transform(chunk, _enc, done) {
      buf = Buffer.concat([buf, chunk])
      for (;;) {
        const h = buf.indexOf("Content-Length:")
        if (h < 0) break
        if (h > 0) {
          stray.push(buf.subarray(0, h).toString())
          buf = buf.subarray(h)
        }
        const end = buf.indexOf("\r\n\r\n")
        if (end < 0) break
        const len = Number(/Content-Length:\s*(\d+)/.exec(buf.subarray(0, end).toString())[1])
        const total = end + 4 + len
        if (buf.length < total) break
        const body = buf.subarray(end + 4, total).toString()
        if (body.includes('"id":"raw-')) {
          const msg = JSON.parse(body)
          if (typeof msg.id === "string" && msg.method === undefined) rawResponses.set(msg.id, msg)
        }
        this.push(buf.subarray(0, total))
        buf = buf.subarray(total)
      }
      done()
    },
  })
}
