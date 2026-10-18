// Resource hygiene: the server must cost nothing while idle and must never
// outlive its editor, however the session ends.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { spawn } from "node:child_process"
import {
  startServer,
  tempDir,
  uriOf,
  fixtures,
  serverPath,
  serverEnv,
  defaultCacheDir,
  strayCacheFiles,
  positionOf,
} from "./lsp-client.mjs"
import { cpuSeconds } from "./procs.mjs"

const ws = path.join(fixtures, "workspace")
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const frame = (o) => {
  const body = JSON.stringify(o)
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`
}

// A server driven by raw frames, so a test can end the session any way it likes.
function rawServer(cwd = ws, initializationOptions) {
  const env = serverEnv()
  const proc = spawn(serverPath, [], { cwd, env, stdio: ["pipe", "ignore", "ignore"] })
  proc.stdin.on("error", () => {})
  const exited = new Promise((r) => proc.on("exit", (code) => r(code)))
  const send = (o) => proc.stdin.write(frame(o))
  send({ jsonrpc: "2.0", id: 1, method: "initialize", params: { capabilities: {}, rootUri: uriOf(cwd), initializationOptions } })
  send({ jsonrpc: "2.0", method: "initialized", params: {} })
  const exitCode = async (ms = 5000) => {
    const code = await Promise.race([exited, sleep(ms).then(() => "running")])
    if (code === "running") proc.kill()
    return code
  }
  return { proc, send, exitCode, cacheDir: env.TURTLE_LSP_CACHE_DIR }
}

// CPU the server uses over 2 s of doing nothing; under 0.1 s passes.
async function assertIdle(s, what) {
  await sleep(300)
  const before = cpuSeconds(s.proc.pid)
  await sleep(2000)
  const used = cpuSeconds(s.proc.pid) - before
  console.log(`# idle CPU, ${what}: ${used.toFixed(2)} s in 2 s`)
  assert.ok(used < 0.1, `idle server (${what}) used ${used.toFixed(2)} s of CPU in 2 s`)
}

// owl:imports are fetched from this server. /slow never answers; /partial
// sends half its body and then stalls.
const ONTO = `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
<> a owl:Ontology .
<#Remote> a owl:Class ; rdfs:label "Remote thing" .
`
process.env.NO_PROXY = "127.0.0.1"
let web, base
const hits = []
const pending = []
before(async () => {
  web = http.createServer((req, res) => {
    hits.push(req.url)
    if (req.url === "/onto.ttl") {
      res.writeHead(200, { "content-type": "text/turtle" })
      res.end(ONTO)
    } else if (req.url === "/partial") {
      res.writeHead(200, { "content-type": "text/turtle", "content-length": ONTO.length * 2 })
      res.write(ONTO)
      pending.push(res)
    } else if (req.url === "/slow") {
      pending.push(res)
    } else {
      res.writeHead(404)
      res.end()
    }
  })
  await new Promise((r) => web.listen(0, "127.0.0.1", r))
  base = `http://127.0.0.1:${web.address().port}`
})
after(() => {
  pending.forEach((r) => r.destroy())
  web.close()
})

const importing = (iri) => `@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix r: <${base}/onto.ttl#> .
<http://example.com/ont/app> owl:imports <${iri}> .
<http://example.com/ont/app#x> a r:Remote .
`

// Opens a document importing `iri` and waits until the imported term is known.
async function openAndResolve(s, iri = `${base}/onto.ttl`, fetch = async () => {}) {
  const uri = uriOf(path.join(ws, "__resources-fetch.ttl"))
  const text = importing(iri)
  s.open(uri, text)
  await s.request("workspace/symbol", { query: "" })
  await fetch(uri)
  const position = positionOf(text, "r:Remote", 3)
  const end = Date.now() + 10000
  let last
  for (;;) {
    last = await s.request("textDocument/hover", { textDocument: { uri }, position })
    if (last?.contents?.value?.includes("Remote thing")) return uri
    assert.ok(Date.now() < end, `the import never resolved; last hover: ${JSON.stringify(last)}`)
    await sleep(50)
  }
}

// Every entry under `dir` with its size and mtime; null if `dir` does not exist.
function snapshot(dir) {
  if (!dir || !fs.existsSync(dir)) return null
  const stat = (f) => {
    const st = fs.statSync(path.join(dir, f))
    return `${f} ${st.size} ${st.mtimeMs}`
  }
  return [stat(""), ...fs.readdirSync(dir, { recursive: true }).sort().map(stat)]
}

// The finished imports in a cache, by name, with their bytes.
function cachedImports(cacheDir) {
  const dir = path.join(cacheDir, "imports")
  if (!fs.existsSync(dir)) return {}
  return Object.fromEntries(fs.readdirSync(dir).map((f) => [f, fs.readFileSync(path.join(dir, f))]))
}

test("an idle server uses no CPU", async () => {
  const s = await startServer()
  try {
    const uri = uriOf(path.join(ws, "people.ttl"))
    s.open(uri, fs.readFileSync(path.join(ws, "people.ttl"), "utf8"))
    await s.diagnosticsFor(uri)
    // The first request waits for the workspace index, so it is built by now.
    await s.request("textDocument/hover", { textDocument: { uri }, position: { line: 0, character: 0 } })
    await assertIdle(s, "after a hover")
  } finally {
    await s.stop()
  }
})

test("an idle server uses no CPU after opening, editing, formatting and closing .nq and .rq files", async () => {
  const query = path.join(fixtures, "query")
  const s = await startServer({ cwd: query })
  try {
    for (const [name, languageId] of [
      ["data.nq", "nquads"],
      ["people.rq", "sparql"],
      ["update.ru", "sparql"],
    ]) {
      const uri = uriOf(path.join(query, name))
      const text = fs.readFileSync(path.join(query, name), "utf8")
      s.conn.sendNotification("textDocument/didOpen", { textDocument: { uri, languageId, version: 1, text } })
      await s.diagnosticsFor(uri)
      s.change(uri, text + "\n# edited\n")
      await s.diagnosticsFor(uri)
      await s.request("textDocument/formatting", { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } })
      await s.request("textDocument/completion", { textDocument: { uri }, position: { line: 1, character: 0 } })
      await s.request("textDocument/semanticTokens/full", { textDocument: { uri } })
      s.close(uri)
    }
    await assertIdle(s, "after N-Quads and SPARQL documents")
  } finally {
    await s.stop()
  }
})

test("the server exits when the editor goes away while a long query is open", async () => {
  const body = Array.from({ length: 5000 }, (_, i) => `?s${i} <https://e.org/p> ?o${i} .`).join("\n")
  const s = rawServer(path.join(fixtures, "query"))
  const uri = uriOf(path.join(fixtures, "query", "__long.rq"))
  s.send({ jsonrpc: "2.0", method: "textDocument/didOpen", params: { textDocument: { uri, languageId: "sparql", version: 1, text: `SELECT * WHERE {\n${body}\n}\n` } } })
  s.send({ jsonrpc: "2.0", id: 2, method: "textDocument/formatting", params: { textDocument: { uri }, options: { tabSize: 4, insertSpaces: true } } })
  s.proc.stdin.end()
  assert.notEqual(await s.exitCode(), "running", "server still running 5 s after stdin closed")
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
    await assertIdle(s, "imports thread")
  } finally {
    await s.stop()
  }

  const asked = pending.length
  const s2 = await startServer({ initializationOptions: { imports: { network: true } } })
  const uri = uriOf(path.join(ws, "__resources-slow.ttl"))
  s2.open(uri, `<http://example.com/ont/a> <http://www.w3.org/2002/07/owl#imports> <${base}/slow> .\n`)
  await s2.request("workspace/symbol", { query: "" })
  for (let i = 0; i < 100 && pending.length === asked; i++) await sleep(20)
  assert.equal(pending.length, asked + 1, "the import was not requested")
  // stop() fails if the process outlives shutdown/exit by 3 s; the fetch would take 10 s.
  await s2.stop()
  assert.deepEqual(strayCacheFiles(s2.cacheDir), [])
})

// The import cache: the suite never writes to the user's real cache, and a
// fetch leaves only finished `<16 hex>.ttl` files behind.
test("a fetched import lands in the test's own cache, and the real cache is not touched", async () => {
  const real = defaultCacheDir()
  const before = snapshot(real)
  const s = await startServer({ initializationOptions: { imports: { network: true } } })
  try {
    assert.ok(s.cacheDir && s.cacheDir !== real, `server cache: ${s.cacheDir}`)
    await openAndResolve(s)
  } finally {
    await s.stop()
  }
  assert.equal(Object.keys(cachedImports(s.cacheDir)).length, 1)
  assert.deepEqual(strayCacheFiles(s.cacheDir), [])
  assert.deepEqual(snapshot(real), before, `${real} changed`)
})

test("an idle server uses no CPU after a fetch has finished", async () => {
  const s = await startServer({ initializationOptions: { imports: { network: true } } })
  try {
    await openAndResolve(s)
    await assertIdle(s, "after a fetch")
    assert.deepEqual(strayCacheFiles(s.cacheDir), [])
  } finally {
    await s.stop()
  }
})

test("a server that goes away mid-fetch leaves no temp file in the cache", async () => {
  for (const end of ["stdin closed", "killed"]) {
    const asked = pending.length
    const s = rawServer(ws, { imports: { network: true } })
    const uri = uriOf(path.join(ws, "__resources-partial.ttl"))
    s.send({
      jsonrpc: "2.0",
      method: "textDocument/didOpen",
      params: { textDocument: { uri, languageId: "turtle", version: 1, text: importing(`${base}/partial`) } },
    })
    for (let i = 0; i < 250 && pending.length === asked; i++) await sleep(20)
    assert.equal(pending.length, asked + 1, `${end}: the import was not requested`)
    // Half the body has arrived; the rest never will.
    await sleep(100)
    if (end === "killed") s.proc.kill("SIGKILL")
    else s.proc.stdin.end()
    assert.notEqual(await s.exitCode(), "running", `${end}: server still running 5 s later`)
    assert.deepEqual(strayCacheFiles(s.cacheDir), [], end)
    assert.deepEqual(cachedImports(s.cacheDir), {}, end)
  }
})

test("opening and closing a cached import leaves the cache byte-identical", async () => {
  const s = await startServer({ initializationOptions: { imports: { network: true } } })
  try {
    await openAndResolve(s)
    const before = cachedImports(s.cacheDir)
    const times = snapshot(s.cacheDir)
    const [name] = Object.keys(before)
    const uri = uriOf(path.join(s.cacheDir, "imports", name))
    s.open(uri, before[name].toString("utf8"))
    await s.request("textDocument/documentSymbol", { textDocument: { uri } })
    s.notify("textDocument/didSave", { textDocument: { uri } })
    s.close(uri)
    await s.request("workspace/symbol", { query: "" })
    await sleep(300)
    assert.deepEqual(cachedImports(s.cacheDir), before)
    assert.deepEqual(snapshot(s.cacheDir), times)
  } finally {
    await s.stop()
  }
})

// Gated on the capability, so it runs once the server offers the command.
test("the fetch-import quick fix caches only finished files and leaves the server idle", async (t) => {
  const real = defaultCacheDir()
  const before = snapshot(real)
  const s = await startServer()
  try {
    const commands = s.init.capabilities.executeCommandProvider?.commands ?? []
    if (!commands.includes("turtle.fetchImport")) return t.skip("the server does not offer turtle.fetchImport")
    const iri = `${base}/onto.ttl`
    const asked = hits.length
    await openAndResolve(s, iri, async (uri) => {
      // The setting is off: nothing is fetched until the user asks.
      const end = Date.now() + 10000
      while (!(await s.diagnosticsFor(uri)).some((d) => d.code === "unresolved-import")) {
        assert.ok(Date.now() < end, "no unresolved-import diagnostic")
        await sleep(50)
      }
      assert.equal(hits.length, asked)
      await s.request("workspace/executeCommand", { command: "turtle.fetchImport", arguments: [iri] })
    })
    assert.deepEqual(hits.slice(asked), ["/onto.ttl"])
    assert.equal(Object.keys(cachedImports(s.cacheDir)).length, 1)
    assert.deepEqual(strayCacheFiles(s.cacheDir), [])
    await assertIdle(s, "after a quick-fix fetch")
  } finally {
    await s.stop()
  }
  assert.deepEqual(snapshot(real), before, `${real} changed`)
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
    await assertIdle(s, "after a validation run")
  } finally {
    await s.stop()
  }
})

test("the server exits when the editor goes away during a validation", async () => {
  // Validating a 10,000-node chain with an ex:next* path takes seconds.
  const dir = tempDir("turtle-shacl-exit-")
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
  const dir = tempDir("turtle-deep-")
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
  const dir = tempDir("turtle-chain-")
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
