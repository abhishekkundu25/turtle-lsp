// Which binary the extension runs, and how the language client is started and
// restarted. Both live in src/server.ts, which does not import `vscode`, so
// they run here with a stubbed file system and a fake client.
import { test } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
import { findServer, ServerHandle } from "../src/server.ts"

/** A file system holding exactly `files`. */
const fsWith = (...files) => ({ existsSync: (p) => files.includes(p) })

const base = {
  exe: "turtle-lsp",
  candidates: ["/ext/server/turtle-lsp", "/ext/../../target/release/turtle-lsp"],
  pathEnv: "",
  trusted: true,
  workspaceFolders: ["/work/project"],
  path: path.posix,
}
const find = (over, ...files) => findServer({ ...base, fs: fsWith(...files), ...over })

test("a configured path that exists is used", () => {
  assert.deepEqual(find({ configured: "/opt/tl/turtle-lsp" }, "/opt/tl/turtle-lsp", "/ext/server/turtle-lsp"), { command: "/opt/tl/turtle-lsp" })
})

test("a configured path that is missing is an error, not a fall-back to another binary", () => {
  const r = find({ configured: "/opt/tl/turtle-lsp" }, "/ext/server/turtle-lsp")
  assert.match(r.error, /does not exist: \/opt\/tl\/turtle-lsp/)
})

test("a relative configured path is an error", () => {
  const r = find({ configured: "bin/turtle-lsp" }, "/work/project/bin/turtle-lsp")
  assert.match(r.error, /must be an absolute path/)
})

test("an empty configured path means the bundled binary, then PATH", () => {
  assert.deepEqual(find({ configured: "  " }, "/ext/server/turtle-lsp"), { command: "/ext/server/turtle-lsp" })
  assert.deepEqual(find({ configured: "", pathEnv: "/usr/local/bin" }, "/usr/local/bin/turtle-lsp"), { command: "/usr/local/bin/turtle-lsp" })
})

test("relative and empty PATH entries are never searched (they mean the workspace)", () => {
  // `:bin::.` is: empty, `bin`, empty, `.`; the binary sits in the workspace under each.
  const files = ["/work/project/bin/turtle-lsp", "/work/project/turtle-lsp", "/work/project/./turtle-lsp"]
  const stub = { existsSync: (p) => files.includes(p) || p === "bin/turtle-lsp" || p === "turtle-lsp" || p === "./turtle-lsp" }
  const r = findServer({ ...base, fs: stub, pathEnv: ":bin::." })
  assert.match(r.error, /not found/)
  // An absolute entry after them still works.
  assert.deepEqual(findServer({ ...base, fs: { existsSync: (p) => stub.existsSync(p) || p === "/usr/bin/turtle-lsp" }, pathEnv: ":bin::.:/usr/bin" }), {
    command: "/usr/bin/turtle-lsp",
  })
})

test("Windows PATH entries are split on ; and must be absolute there too", () => {
  const win = {
    ...base,
    path: path.win32,
    exe: "turtle-lsp.exe",
    candidates: [],
    workspaceFolders: ["C:\\work"],
    pathEnv: "bin;D:\\tools\\tl",
    fs: { existsSync: (p) => p === "D:\\tools\\tl\\turtle-lsp.exe" || p === "bin\\turtle-lsp.exe" },
  }
  assert.deepEqual(findServer(win), { command: "D:\\tools\\tl\\turtle-lsp.exe" })
  assert.match(findServer({ ...win, pathEnv: "bin" }).error, /not found/)
})

test("untrusted: a candidate inside a workspace folder is skipped, the next one wins", () => {
  const inside = "/work/project/node_modules/.bin/turtle-lsp"
  const r = find({ trusted: false, candidates: [inside, "/ext/server/turtle-lsp"] }, inside, "/ext/server/turtle-lsp")
  assert.deepEqual(r, { command: "/ext/server/turtle-lsp" })
  // Trusted, the first one is fine.
  assert.deepEqual(find({ trusted: true, candidates: [inside, "/ext/server/turtle-lsp"] }, inside, "/ext/server/turtle-lsp"), { command: inside })
})

test("untrusted: an absolute PATH entry inside the workspace is skipped", () => {
  const r = find({ trusted: false, pathEnv: "/work/project/bin:/usr/bin" }, "/work/project/bin/turtle-lsp", "/usr/bin/turtle-lsp")
  assert.deepEqual(r, { command: "/usr/bin/turtle-lsp" })
  assert.match(find({ trusted: false, pathEnv: "/work/project/bin" }, "/work/project/bin/turtle-lsp").error, /not found/)
})

test("untrusted: a configured path inside the workspace is refused; one outside is used", () => {
  assert.match(find({ trusted: false, configured: "/work/project/turtle-lsp" }, "/work/project/turtle-lsp").error, /not trusted/)
  assert.deepEqual(find({ trusted: false, configured: "/opt/turtle-lsp" }, "/opt/turtle-lsp"), { command: "/opt/turtle-lsp" })
})

test("a folder whose name only starts like a workspace folder is outside it", () => {
  const r = find({ trusted: false, candidates: ["/work/project-tools/turtle-lsp"] }, "/work/project-tools/turtle-lsp")
  assert.deepEqual(r, { command: "/work/project-tools/turtle-lsp" })
})

test("untrusted: a symlink into the workspace counts as inside it", () => {
  const link = "/usr/local/bin/turtle-lsp"
  const fs = { existsSync: (p) => p === link, realpathSync: (p) => (p === link ? "/work/project/target/turtle-lsp" : p) }
  assert.match(findServer({ ...base, fs, trusted: false, candidates: [], pathEnv: "/usr/local/bin" }).error, /not found/)
  assert.deepEqual(findServer({ ...base, fs, trusted: true, candidates: [], pathEnv: "/usr/local/bin" }), { command: link })
})

// ---------------------------------------------------------------------------

/** A client that records what is done to it, and can fail to start. */
function fakeClients() {
  const log = []
  let running = 0
  let overlap = false
  let id = 0
  const make = (failStart) => {
    const n = ++id
    const c = {
      n,
      state: "initial",
      async start() {
        log.push(`start ${n}`)
        if (running++ > 0) overlap = true
        await new Promise((r) => setTimeout(r, 5))
        running--
        if (failStart) {
          c.state = "start-failed"
          throw new Error("spawn ENOENT")
        }
        c.state = "running"
      },
      async stop() {
        log.push(`stop ${n}`)
        // vscode-languageclient 10.x: stop() throws unless the client is running.
        if (c.state !== "running") throw new Error(`Client is not running and can't be stopped. It's current state is: ${c.state}`)
        if (running++ > 0) overlap = true
        await new Promise((r) => setTimeout(r, 5))
        running--
        c.state = "stopped"
      },
    }
    return c
  }
  return { log, make, overlapped: () => overlap }
}

test("restart after a failed start builds a new client and ends running", async () => {
  const { log, make } = fakeClients()
  let fail = true
  const handle = new ServerHandle(() => make(fail))
  await assert.rejects(handle.ensureStarted(), /spawn ENOENT/)
  assert.equal(handle.client(), undefined, "a client that failed to start is dropped")
  fail = false
  const client = await handle.restart()
  assert.equal(client.state, "running")
  assert.equal(handle.client(), client)
  assert.deepEqual(log.filter((l) => l.startsWith("start")), ["start 1", "start 2"])
})

test("restart with no server to run (create returns undefined) leaves no client and does not throw", async () => {
  const handle = new ServerHandle(() => undefined)
  assert.equal(await handle.restart(), undefined)
  assert.equal(await handle.ensureStarted(), undefined)
  assert.equal(handle.client(), undefined)
})

test("restart stops the running client, then starts a new one", async () => {
  const { log, make } = fakeClients()
  const handle = new ServerHandle(() => make(false))
  const first = await handle.ensureStarted()
  const second = await handle.restart()
  assert.notEqual(first, second)
  assert.equal(first.state, "stopped")
  assert.equal(second.state, "running")
  assert.deepEqual(log, ["start 1", "stop 1", "start 2"])
})

test("ensureStarted twice starts one client", async () => {
  const { log, make } = fakeClients()
  const handle = new ServerHandle(() => make(false))
  const [a, b] = await Promise.all([handle.ensureStarted(), handle.ensureStarted()])
  assert.equal(a, b)
  assert.deepEqual(log, ["start 1"])
})

test("starts, restarts and stops asked together run one at a time, in order", async () => {
  const { log, make, overlapped } = fakeClients()
  const handle = new ServerHandle(() => make(false))
  await Promise.all([handle.ensureStarted(), handle.restart(), handle.restart(), handle.ensureStarted()])
  assert.deepEqual(log, ["start 1", "stop 1", "start 2", "stop 2", "start 3"])
  await handle.dispose()
  assert.equal(overlapped(), false, log.join(", "))
  assert.deepEqual(log.slice(-1), ["stop 3"])
  assert.equal(handle.client(), undefined)
})

test("dispose cancels restarts still waiting, and stops what is running", async () => {
  const { log, make } = fakeClients()
  const handle = new ServerHandle(() => make(false))
  await handle.ensureStarted()
  await Promise.all([handle.restart(), handle.restart(), handle.dispose()])
  assert.deepEqual(log, ["start 1", "stop 1"])
  assert.equal(handle.client(), undefined)
})

test("a failed start does not stop later requests from running", async () => {
  const { make } = fakeClients()
  let calls = 0
  const handle = new ServerHandle(() => make(++calls === 1))
  const first = handle.ensureStarted()
  const second = handle.restart()
  await assert.rejects(first, /spawn ENOENT/)
  assert.equal((await second).state, "running")
})

test("dispose stops the client; later requests start nothing", async () => {
  const { log, make } = fakeClients()
  const handle = new ServerHandle(() => make(false))
  const client = await handle.ensureStarted()
  await handle.dispose()
  assert.equal(client.state, "stopped")
  assert.equal(await handle.restart(), undefined)
  assert.equal(await handle.ensureStarted(), undefined)
  assert.deepEqual(log, ["start 1", "stop 1"])
})
