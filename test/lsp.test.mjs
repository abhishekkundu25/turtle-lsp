import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { spawn } from "node:child_process"
import { createRequire } from "node:module"
import { startServer, uriOf, fixtures, positionOf, serverPath, serverEnv, tempDir } from "./lsp-client.mjs"

const rpc = createRequire(import.meta.url)("vscode-jsonrpc/node")

const ws = path.join(fixtures, "workspace")
const peoplePath = path.join(ws, "people.ttl")
const orgPath = path.join(ws, "org.ttl")
const people = fs.readFileSync(peoplePath, "utf8")
const org = fs.readFileSync(orgPath, "utf8")
const scratch = (name) => uriOf(path.join(ws, `__${name}.ttl`))

let s
before(async () => {
  s = await startServer()
})
after(async () => {
  await s.stop()
})

const problems = (diags) => diags.filter((d) => d.severity <= 2)
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.message}`).join("\n")

async function diagnose(name, text) {
  const uri = scratch(name)
  s.open(uri, text)
  return { uri, diags: await s.diagnosticsFor(uri) }
}

// ---------- protocol hygiene ----------

test("server writes nothing to stdout except LSP messages", async () => {
  assert.deepEqual(s.stray, [])
})

test("initialize answers quickly", async () => {
  assert.ok(s.initMs < 500, `initialize took ${s.initMs.toFixed(0)}ms`)
})

// ---------- diagnostics ----------

test("valid file has no warnings or errors", async () => {
  const { diags } = await diagnose("valid", people)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("object on the next line is not flagged as a missing terminator", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p
    ex:o .
`
  const { diags } = await diagnose("multiline-stmt", text)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("multi-line string literal content is not linted", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p """first line
  see also foo:bar and nothing ends here
last line""" .
`
  const { diags } = await diagnose("multiline-string", text)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("syntax error is reported on the offending line", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p ex:o .
ex:t ex:p "unterminated .
`
  const { diags } = await diagnose("syntax", text)
  const errors = diags.filter((d) => d.severity === 1)
  assert.ok(errors.length > 0, "expected a syntax error")
  assert.ok(errors.some((d) => d.range.start.line === 2), fmt(errors))
})

test("undeclared prefix is warned once, on the use", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:s ex:p nope:o .
`
  const { diags } = await diagnose("undeclared", text)
  const w = diags.filter((d) => /nope/.test(d.message))
  assert.equal(w.length, 1, fmt(diags))
  assert.equal(w[0].range.start.line, 1)
  assert.equal(w[0].range.start.character, 10)
})

test("unknown term in a well-known vocabulary is warned on first open", async () => {
  const text = `@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .
@prefix ex: <http://example.org/> .
ex:s rdfs:labell "x" .
`
  const { diags } = await diagnose("unknown-term", text)
  assert.ok(diags.some((d) => /labell/.test(d.message)), fmt(diags))
})

test("a well-known prefix name bound to a custom IRI is not checked against that vocabulary", async () => {
  const text = `@prefix foaf: <http://my.org/not-foaf#> .
foaf:thing foaf:whatever foaf:other .
`
  const uri = scratch("custom-iri")
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  // give the lazy vocab load a chance, then re-check
  await new Promise((r) => setTimeout(r, 1500))
  s.change(uri, text + "\n")
  const diags = await s.diagnosticsFor(uri)
  assert.deepEqual(problems(diags), [], fmt(diags))
})

test("prefix declaration ranges point at the declaration", async () => {
  const text = `@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix : <http://example.org/> .
:a rdf:type :B .
`
  const uri = scratch("empty-prefix")
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const syms = await s.request("textDocument/documentSymbol", { textDocument: { uri } })
  const decls = syms.flatMap((x) => x.children ?? [x])
  const empty = decls.find((x) => x.name === ":" || x.name.startsWith(": "))
  assert.ok(empty, JSON.stringify(syms))
  const range = empty.selectionRange ?? empty.location.range
  assert.equal(range.start.line, 1, JSON.stringify(empty))
})

// ---------- completion ----------

async function complete(uri, text, needle) {
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const res = await s.request("textDocument/completion", {
    textDocument: { uri },
    position: positionOf(text, needle),
  })
  return Array.isArray(res) ? res : res.items
}

test("completion after 'foaf:' offers foaf terms only", async () => {
  const text = `@prefix foaf: <http://xmlns.com/foaf/0.1/> .
@prefix ex: <http://example.org/> .
ex:a a foaf:
`
  const items = await complete(scratch("cmp-foaf"), text, "a foaf:")
  const labels = items.map((i) => i.label)
  assert.ok(labels.includes("foaf:Person"), labels.slice(0, 20).join(", "))
  const foreign = labels.filter((l) => /^[\w-]+:/.test(l) && !l.startsWith("foaf:"))
  assert.deepEqual(foreign, [])
})

test("completion at an empty position returns a bounded list", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a ex:p 
`
  const items = await complete(scratch("cmp-empty"), text, "ex:p ")
  assert.ok(items.length < 1000, `got ${items.length} items`)
})

test("completing a term from an undeclared well-known prefix adds the @prefix line", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a a skos:Conc
`
  const items = await complete(scratch("cmp-autoimport"), text, "skos:Conc")
  const concept = items.find((i) => i.label === "skos:Concept")
  assert.ok(concept, "skos:Concept offered")
  assert.ok(concept.additionalTextEdits?.some((e) => e.newText.includes("@prefix skos:")), JSON.stringify(concept))
})

test("hover on a vocabulary term shows its documentation", async () => {
  const uri = scratch("hover")
  s.open(uri, people)
  await s.diagnosticsFor(uri)
  const h = await s.request("textDocument/hover", {
    textDocument: { uri },
    position: positionOf(people, "a foaf:Pers"),
  })
  assert.ok(h && JSON.stringify(h.contents).length > 20, JSON.stringify(h))
})

test("completion without a prefix finds vocabulary terms by local name", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a a Pers
`
  const items = await complete(scratch("cmp-noprefix"), text, "a Pers")
  const person = items.find((i) => i.label === "foaf:Person")
  assert.ok(person, items.map((i) => i.label).slice(0, 20).join(", "))
  assert.ok(person.additionalTextEdits?.some((e) => e.newText.includes("@prefix foaf:")), "adds @prefix foaf")
})

test("'@' triggers completion of directives", async () => {
  assert.ok(s.init.capabilities.completionProvider.triggerCharacters.includes("@"))
  const text = `@
`
  const items = await complete(scratch("cmp-at"), text, "@")
  assert.ok(items.some((i) => i.label === "@prefix"), items.map((i) => i.label).join(", "))
})

test("nested [ ] and ( ) blocks fold", async () => {
  const text = `@prefix ex: <http://example.org/> .
ex:a ex:knows [
        ex:name "B" ;
        ex:age 3
    ] ;
    ex:list (
        ex:x
        ex:y
    ) .
`
  const uri = scratch("fold")
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const folds = (await s.request("textDocument/foldingRange", { textDocument: { uri } })).map((f) => `${f.startLine}-${f.endLine}`)
  assert.ok(folds.includes("1-3"), folds.join(" "))
  assert.ok(folds.includes("5-7"), folds.join(" "))
  assert.ok(folds.includes("1-8"), folds.join(" "))
})

// ---------- navigation (own server: only the two fixture files are indexed) ----------

describe("navigation", () => {
  let n
  before(async () => {
    n = await startServer()
  })
  after(async () => {
    await n.stop()
  })


  test("go to definition of a subject jumps to where it is described, not to every use", async () => {
    const uri = uriOf(orgPath)
    n.open(uri, org)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/definition", {
      textDocument: { uri },
      position: positionOf(org, "ex:alic"),
    })
    assert.equal(locs.length, 1, JSON.stringify(locs))
    assert.equal(locs[0].uri, uriOf(peoplePath))
    assert.equal(locs[0].range.start.line, 4)
  })

  test("references have no duplicates", async () => {
    const uri = uriOf(peoplePath)
    n.open(uri, people)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/references", {
      textDocument: { uri },
      position: positionOf(people, "ex:bo"),
      context: { includeDeclaration: true },
    })
    const keys = locs.map((l) => `${l.uri}#${l.range.start.line}:${l.range.start.character}`)
    assert.deepEqual(keys, [...new Set(keys)])
    assert.equal(keys.length, 2, keys.join("\n"))
  })

  test("rename produces non-overlapping edits", async () => {
    const uri = uriOf(peoplePath)
    n.open(uri, people)
    await n.diagnosticsFor(uri)
    const edit = await n.request("textDocument/rename", {
      textDocument: { uri },
      position: positionOf(people, "ex:alic"),
      newName: "ex:alicia",
    })
    for (const [u, edits] of Object.entries(edit.changes)) {
      const keys = edits.map((e) => `${e.range.start.line}:${e.range.start.character}`)
      assert.deepEqual(keys, [...new Set(keys)], `duplicate edits in ${u}`)
    }
    const total = Object.values(edit.changes).flat().length
    assert.equal(total, 2, JSON.stringify(edit.changes)) // 1 in people.ttl, 1 in org.ttl
  })

  test("closing a workspace file keeps it in the workspace index", async () => {
    const uri = uriOf(peoplePath)
    n.open(uri, people)
    await n.diagnosticsFor(uri)
    n.close(uri)
    const orgUri = uriOf(orgPath)
    n.open(orgUri, org)
    await n.diagnosticsFor(orgUri)
    const locs = await n.request("textDocument/definition", {
      textDocument: { uri: orgUri },
      position: positionOf(org, "ex:alic"),
    })
    assert.ok(locs.some((l) => l.uri === uri), JSON.stringify(locs))
  })

  test("type definition jumps to the resource's class", async () => {
    const path_ = path.join(ws, "classes.ttl")
    const text = fs.readFileSync(path_, "utf8")
    const uri = uriOf(path_)
    n.open(uri, text)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/typeDefinition", { textDocument: { uri }, position: positionOf(text, "ex:dav") })
    assert.deepEqual(locs.map((l) => l.range.start.line), [7], JSON.stringify(locs))
  })

  test("implementation lists instances and subclasses", async () => {
    const path_ = path.join(ws, "classes.ttl")
    const text = fs.readFileSync(path_, "utf8")
    const uri = uriOf(path_)
    n.open(uri, text)
    await n.diagnosticsFor(uri)
    const locs = await n.request("textDocument/implementation", { textDocument: { uri }, position: positionOf(text, "ex:Employe") })
    assert.deepEqual(locs.map((l) => l.range.start.line).sort(), [11, 7].sort(), JSON.stringify(locs))
  })
})

test("workspace root comes from the client's rootUri, not the process cwd", async () => {
  const other = await startServer({ cwd: os.tmpdir(), rootUri: uriOf(ws) })
  try {
    const orgUri = uriOf(orgPath)
    other.open(orgUri, org)
    await other.diagnosticsFor(orgUri)
    const locs = await other.request("textDocument/definition", {
      textDocument: { uri: orgUri },
      position: positionOf(org, "ex:alic"),
    })
    assert.ok(locs.some((l) => l.uri === uriOf(peoplePath)), JSON.stringify(locs))
  } finally {
    await other.stop()
  }
})

test("diagnostics come back under the client's spelling of the URI", async () => {
  // `%7E` is `~` and `%2D` is `-`: the same file, spelled the way Node and
  // VS Code spell some paths. A client matching URIs as strings needs its own.
  const uri = scratch("spelled-uri").replace("spelled-uri", "spelled%2Duri%7E")
  s.open(uri, "@prefix ex: <http://example.com/> .\nex:a ex:b ex:c ex:d .\n")
  const diags = await s.diagnosticsFor(uri)
  assert.ok(problems(diags).length > 0, fmt(diags))
  s.close(uri)
})

// ---------- isolation and the disk index ----------

// A server on a main thread with a 512 KB stack (`ulimit -s`), which a deep
// document would overflow if the server parsed it there: the server runs on a
// thread of its own with a large stack. Not on Windows, which has no ulimit.
test(
  "documents nested 255 levels deep are answered on a 512 KB process stack",
  { skip: process.platform === "win32" && "no ulimit -s" },
  async () => {
    const proc = spawn("sh", ["-c", 'ulimit -s 512 && exec "$0"', serverPath], {
      env: serverEnv(),
      stdio: ["pipe", "pipe", "inherit"],
    })
    const exited = new Promise((r) => proc.once("exit", (code, signal) => r({ code, signal })))
    proc.stdin.on("error", () => {})
    const conn = rpc.createMessageConnection(
      new rpc.StreamMessageReader(proc.stdout),
      new rpc.StreamMessageWriter(proc.stdin),
    )
    conn.listen()
    try {
      await conn.sendRequest("initialize", { processId: process.pid, rootUri: null, capabilities: {} })
      await conn.sendNotification("initialized", {})
      const n = 255
      const docs = {
        "groups.rq": "SELECT * WHERE " + "{ ".repeat(n) + "?s ?p ?o" + " }".repeat(n),
        "optional.rq": "SELECT * WHERE { " + "OPTIONAL { ".repeat(n) + "?s ?p ?o" + " }".repeat(n) + " }",
        "parens.rq": "SELECT * WHERE { ?s ?p ?o FILTER " + "(".repeat(n) + "1" + ")".repeat(n) + " }",
      }
      for (const [name, text] of Object.entries(docs)) {
        const uri = `file:///nowhere/${name}`
        await conn.sendNotification("textDocument/didOpen", {
          textDocument: { uri, languageId: "sparql", version: 1, text },
        })
        const symbols = await Promise.race([
          conn.sendRequest("textDocument/documentSymbol", { textDocument: { uri } }),
          exited.then((e) => assert.fail(`the server died on ${name}: ${JSON.stringify(e)}`)),
        ])
        assert.ok(Array.isArray(symbols), `${name}: ${JSON.stringify(symbols)}`)
      }
      await conn.sendRequest("shutdown")
      await conn.sendNotification("exit")
      assert.deepEqual(await exited, { code: 0, signal: null })
    } finally {
      conn.dispose()
      proc.kill()
    }
  },
)

describe("files changing on disk", () => {
  let dir, w
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
  const ttl = (name) => `@prefix ex: <http://e.org/> .\nex:${name} a ex:Thing .\n`
  const write = (rel, text) => {
    fs.mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true })
    fs.writeFileSync(path.join(dir, rel), text)
    return uriOf(path.join(dir, rel))
  }
  // The files that workspace symbols name, sorted.
  const indexed = async () => {
    const symbols = await w.request("workspace/symbol", { query: "" })
    return [...new Set(symbols.map((x) => path.basename(new URL(x.location.uri).pathname)))].sort()
  }
  before(async () => {
    dir = tempDir("turtle-lsp-disk-")
    write("keep/a.ttl", ttl("Alpha"))
    write("gone/b.ttl", ttl("Beta"))
    write("gone/deep/c.ttl", ttl("Gamma"))
    w = await startServer({ cwd: dir })
  })
  after(async () => {
    await w.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  })

  test("the files of a deleted folder leave the index; a node_modules event is ignored", async () => {
    // The first workspace request waits for the initial index.
    assert.deepEqual(await indexed(), ["a.ttl", "b.ttl", "c.ttl"])
    const ignored = write("node_modules/pkg/n.ttl", ttl("Node"))
    const hidden = write(".cache/h.ttl", ttl("Hidden"))
    w.notify("workspace/didChangeWatchedFiles", {
      changes: [
        { uri: ignored, type: 1 },
        { uri: hidden, type: 2 },
      ],
    })
    assert.deepEqual(await indexed(), ["a.ttl", "b.ttl", "c.ttl"])
    fs.rmSync(path.join(dir, "gone"), { recursive: true })
    w.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: uriOf(path.join(dir, "gone")), type: 3 }] })
    assert.deepEqual(await indexed(), ["a.ttl"])
  })

  test("a document closed outside the workspace is not indexed from disk", async () => {
    const away = tempDir("turtle-lsp-away-")
    try {
      const file = path.join(away, "away.ttl")
      fs.writeFileSync(file, ttl("Away"))
      w.open(uriOf(file), ttl("Away"))
      assert.deepEqual(await indexed(), ["a.ttl", "away.ttl"])
      w.close(uriOf(file))
      assert.deepEqual(await indexed(), ["a.ttl"])
    } finally {
      fs.rmSync(away, { recursive: true, force: true })
    }
  })

  test("a changed prefixes.ttl re-parses its files off the main loop; requests are not held up", async () => {
    write("p/prefixes.ttl", "@prefix ex: <http://one.org/> .\n")
    const x = write("p/x.ttl", "ex:S a ex:T .\n")
    const y = write("p/sub/y.ttl", "ex:S a ex:T .\n")
    w.notify("workspace/didChangeWatchedFiles", {
      changes: [x, y, uriOf(path.join(dir, "p/prefixes.ttl"))].map((uri) => ({ uri, type: 1 })),
    })
    const probe = uriOf(path.join(dir, "probe.ttl"))
    const definesIn = async (iri) => {
      w.open(probe, `<${iri}> <http://e.org/p> <http://e.org/o> .\n`)
      const locs = await w.request("textDocument/definition", {
        textDocument: { uri: probe },
        position: { line: 0, character: 3 },
      })
      return locs.map((l) => path.basename(l.uri)).filter((f) => f !== "probe.ttl").sort()
    }
    assert.deepEqual(await definesIn("http://one.org/S"), ["x.ttl", "y.ttl"])
    write("p/prefixes.ttl", "@prefix ex: <http://two.org/> .\n")
    w.notify("workspace/didChangeWatchedFiles", {
      changes: [{ uri: uriOf(path.join(dir, "p/prefixes.ttl")), type: 2 }],
    })
    // The re-parse runs on a thread; its result lands on the main loop soon.
    const end = Date.now() + 10000
    let at
    while (JSON.stringify((at = await definesIn("http://two.org/S"))) !== JSON.stringify(["x.ttl", "y.ttl"])) {
      assert.ok(Date.now() < end, `still ${JSON.stringify(at)}`)
      await sleep(50)
    }
    assert.deepEqual(await definesIn("http://one.org/S"), [])
    w.close(probe)
  })
})

// ---------- relative IRIs resolve against the document's URI ----------

test("a relative @base or BASE is no syntax error (the document's URI is the base)", async () => {
  for (const [name, text] of [
    ["base-turtle", "@base <rel/> .\n<x> <http://example.org/p> <y> .\n"],
    ["base-sparql", "BASE <../up/>\n<x> <http://example.org/p> <y> .\n"],
  ]) {
    const { diags } = await diagnose(name, text)
    assert.deepEqual(
      diags.filter((d) => d.severity === 1),
      [],
      `${name}: ${fmt(diags)}`,
    )
  }
})

test("a TriG graph and a relative IRI inside it resolve against the file", async () => {
  const uri = uriOf(path.join(ws, "__base.trig"))
  const text = "<g> { <#me> <http://example.org/p> <o> }\n"
  s.open(uri, text, "trig")
  const diags = await s.diagnosticsFor(uri)
  assert.deepEqual(problems(diags).filter((d) => d.severity === 1), [], fmt(diags))
  const locs = await s.request("textDocument/definition", {
    textDocument: { uri },
    position: positionOf(text, "<g>", 2),
  })
  assert.equal(locs.length, 1, JSON.stringify(locs))
  s.close(uri)
})
