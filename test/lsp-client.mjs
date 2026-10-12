import { spawn, spawnSync } from "node:child_process"
import { Transform } from "node:stream"
import path from "node:path"
import { fileURLToPath, pathToFileURL } from "node:url"
import rpc from "vscode-jsonrpc/node.js"

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

export async function startServer({ cwd = path.join(fixtures, "workspace"), rootUri = uriOf(cwd) } = {}) {
  warmUp()
  const proc = spawn(serverPath, [], { cwd, stdio: ["pipe", "pipe", "pipe"] })
  // The server may exit before our last write (e.g. after shutdown/exit).
  proc.stdin.on("error", () => {})
  let stderr = ""
  proc.stderr.on("data", (d) => (stderr += d))
  // Strip (and record) any bytes written to stdout outside LSP frames, so one
  // stray console.log can be asserted on instead of hanging every test.
  const stray = []
  const frames = framer(stray)
  proc.stdout.pipe(frames)
  const conn = rpc.createMessageConnection(
    new rpc.StreamMessageReader(frames),
    new rpc.StreamMessageWriter(proc.stdin)
  )
  const diagnostics = new Map()
  const waiters = []
  const logs = []
  conn.onNotification("textDocument/publishDiagnostics", (p) => {
    diagnostics.set(p.uri, p.diagnostics)
    for (const w of waiters.splice(0)) w()
  })
  conn.onNotification("window/logMessage", (p) => logs.push(p.message))
  conn.onRequest(() => null)
  conn.listen()

  const t0 = performance.now()
  const init = await conn.sendRequest("initialize", {
    processId: process.pid,
    rootUri,
    workspaceFolders: [{ uri: rootUri, name: "ws" }],
    capabilities: {},
  })
  const initMs = performance.now() - t0
  conn.sendNotification("initialized", {})

  const versions = new Map()
  const api = {
    proc,
    conn,
    init,
    initMs,
    logs,
    stray,
    get stderr() {
      return stderr
    },
    request: (method, params) => conn.sendRequest(method, params),
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
      conn.dispose()
      proc.kill()
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

function framer(stray) {
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
        this.push(buf.subarray(0, total))
        buf = buf.subarray(total)
      }
      done()
    },
  })
}
