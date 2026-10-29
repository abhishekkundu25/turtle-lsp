import { spawn, spawnSync } from "node:child_process"
import { Transform } from "node:stream"
import { after } from "node:test"
import fs from "node:fs"
import os from "node:os"
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

// A new temporary directory under its long name: Windows' temp path can be an
// 8.3 short name (`RUNNER~1`), whose `~` clients and the server spell differently.
export function tempDir(prefix) {
  return fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), prefix)))
}

// Every server gets a private import cache (TURTLE_LSP_CACHE_DIR) unless the
// caller's environment sets one, so the suite never touches the user's real
// cache. The caches live under one temp dir, removed when the test process
// exits or is stopped by SIGINT or SIGTERM (see `cleanup`).
let cacheRoot
let caches = 0
//
// The test servers listen on loopback, which the server does not fetch from
// unless the environment says so (TURTLE_LSP_ALLOW_PRIVATE_HOSTS=1): the
// suite says so, and a test of the refusal sets it to 0 for one server.
export function serverEnv() {
  const env = { TURTLE_LSP_ALLOW_PRIVATE_HOSTS: "1", ...process.env }
  if (env.TURTLE_LSP_CACHE_DIR) return env
  cacheRoot ??= tempDir("turtle-lsp-caches-")
  return { ...env, TURTLE_LSP_CACHE_DIR: path.join(cacheRoot, String(++caches)) }
}

// The cache directory the server uses when TURTLE_LSP_CACHE_DIR is not set
// (`cache_dir()` in src/imports.rs), or undefined if it has none.
export function defaultCacheDir() {
  const abs = (k) => (process.env[k] && path.isAbsolute(process.env[k]) ? process.env[k] : undefined)
  const home = abs(process.platform === "win32" ? "USERPROFILE" : "HOME")
  const base =
    process.platform === "darwin"
      ? home && path.join(home, "Library", "Caches")
      : process.platform === "win32"
        ? abs("LOCALAPPDATA")
        : abs("XDG_CACHE_HOME") ?? (home && path.join(home, ".cache"))
  return base && path.join(base, "turtle-lsp")
}

// Files in a cache dir that are not a finished import (`imports/<64 hex>.<ext>`
// for ttl, trig, nt, nq), JSON-LD document (`imports/<64 hex>.jsonld`, with the
// `.url` file that names its URL beside it) or query results
// (`query-results/<name>.md`), e.g. a temp file left by an interrupted write.
// Paths relative to `dir`.
export function strayCacheFiles(dir) {
  if (!fs.existsSync(dir)) return []
  const files = fs
    .readdirSync(dir, { recursive: true, withFileTypes: true })
    .filter((e) => !e.isDirectory())
    .map((e) => path.relative(dir, path.join(e.parentPath ?? e.path, e.name)))
  const kept = new Set(files)
  return files.filter((f) => {
    if (/^imports[\\/][0-9a-f]{64}\.(ttl|trig|nt|nq|jsonld)$/.test(f)) return false
    // The results of "Run query", one per query file name.
    if (/^query-results[\\/][^\\/]+\.md$/.test(f)) return false
    // The URL file of a JSON-LD document, only beside the document it belongs to.
    return !(/^imports[\\/][0-9a-f]{64}\.url$/.test(f) && kept.has(f.replace(/\.url$/, ".jsonld")))
  })
}

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

// Servers not yet stopped, with where each was started. One a test forgets to
// stop would hold its file's process open, so the file's last hook kills it and
// fails the file, naming the test's line; the process exit does the same for
// a client used outside the test runner.
const running = new Map()

// Kills the servers still running and returns where each was started.
function killRunning() {
  const leaked = [...running.values()]
  for (const proc of running.keys()) proc.kill()
  running.clear()
  return leaked
}

const neverStopped = (where) => `turtle-lsp started ${where} was never stopped`

function removeCaches() {
  if (cacheRoot) fs.rmSync(cacheRoot, { recursive: true, force: true })
  cacheRoot = undefined
}

process.once("exit", () => {
  for (const where of killRunning()) {
    console.error(neverStopped(where))
    process.exitCode = 1
  }
  removeCaches()
})
// Ctrl-C or a runner timeout: the exit handler does not run on a signal, so
// clean up here and re-raise it, ending the process as it would have ended.
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, () => {
    killRunning()
    removeCaches()
    process.kill(process.pid, signal)
  })
}
// Under the test runner (no --test-force-exit), a leaked server would keep the
// file's process alive: kill it after the file's other hooks, which stop the
// servers they own, and fail the file.
if (process.env.NODE_TEST_CONTEXT) {
  setImmediate(() =>
    after(() => {
      const leaked = killRunning()
      if (leaked.length) throw new Error(leaked.map(neverStopped).join("\n"))
    }),
  )
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
  const env = serverEnv()
  const proc = spawn(serverPath, [], { cwd, env, stdio: ["pipe", "pipe", "pipe"] })
  // The first frame outside this file is the caller.
  const where = new Error().stack.split("\n").slice(2).find((l) => !l.includes("lsp-client.mjs"))?.trim() ?? "by a test"
  running.set(proc, where)
  const exited = new Promise((r) =>
    proc.once("exit", () => {
      running.delete(proc)
      r(true)
    }),
  )
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
  // `window/showMessage` notifications ({ type, message }), in order.
  const messages = []
  const published = []
  // Every publication in order, with its diagnostics.
  const publications = []
  // `$/progress` notifications ({ token, value }), in order.
  const progress = []
  const serverRequests = []
  const versions = new Map()
  conn.onNotification("textDocument/publishDiagnostics", (p) => {
    published.push(p.uri)
    publications.push({ at: performance.now(), ...p })
    // A report for an older version than the one last sent (e.g. a republish
    // after the initial index lands) is stale: the next one is the answer.
    if (p.version != null && versions.has(p.uri) && p.version < versions.get(p.uri)) return
    diagnostics.set(p.uri, p.diagnostics)
    for (const w of waiters.splice(0)) w()
  })
  conn.onNotification("window/logMessage", (p) => logs.push(p.message))
  conn.onNotification("window/showMessage", (p) => messages.push(p))
  conn.onNotification("$/progress", (p) => progress.push(p))
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
    // The server's TURTLE_LSP_CACHE_DIR.
    cacheDir: env.TURTLE_LSP_CACHE_DIR,
    init,
    initMs,
    logs,
    messages,
    stray,
    published,
    publications,
    progress,
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
    // Forgets the diagnostics last published for `uri`, so diagnosticsFor
    // waits for the ones about this text.
    open(uri, text, languageId = "turtle") {
      versions.set(uri, 1)
      diagnostics.delete(uri)
      conn.sendNotification("textDocument/didOpen", {
        textDocument: { uri, languageId, version: 1, text },
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
    // One ranged change, as editors send them (the server syncs incrementally):
    // `range` (LSP positions in the text before) is replaced by `text`.
    edit(uri, range, text) {
      const v = (versions.get(uri) ?? 1) + 1
      versions.set(uri, v)
      diagnostics.delete(uri)
      conn.sendNotification("textDocument/didChange", {
        textDocument: { uri, version: v },
        contentChanges: [{ range, text }],
      })
    },
    // Incremental sync: `changes` ({ range, text }, UTF-16 positions) in one
    // didChange, applied in order, each to the text the one before left.
    changeRange(uri, changes) {
      const v = (versions.get(uri) ?? 1) + 1
      versions.set(uri, v)
      diagnostics.delete(uri)
      conn.sendNotification("textDocument/didChange", {
        textDocument: { uri, version: v },
        contentChanges: changes.map(({ range, text }) => ({ range, text })),
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
      // Each step is bounded: a write that never completes must fail the
      // hook, not hold the test file open.
      const late = (ms, v) => new Promise((r) => setTimeout(() => r(v), ms).unref())
      try {
        await Promise.race([conn.sendRequest("shutdown"), late(500)])
        await Promise.race([conn.sendNotification("exit").catch(() => {}), late(500)])
      } catch {}
      // Every test session must end the process; a server that lingers after
      // shutdown/exit is a leak, not something to paper over with kill().
      const gone = await Promise.race([exited, late(3000, false)])
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
