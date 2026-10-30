// `turtle/schema` over the public corpus (test/corpus/, downloaded by
// `npm run corpus`, not tracked): each file alone, opened in a server with
// an empty workspace, then the whole corpus as one workspace. Each answer
// is checked by test/schema-invariants.mjs. Without the corpus the tests
// are skipped; with CI=true they fail instead.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir } from "./lsp-client.mjs"
import { checkWorkspace, languageOf } from "./schema-invariants.mjs"

const root = path.join(import.meta.dirname, "corpus")
const present = fs.existsSync(root)
if (!present && process.env.CI === "true") {
  throw new Error("test/corpus is absent (run tools/corpus/fetch.sh, or npm run corpus); CI=true does not skip the corpus tests")
}
const RDF = /\.(ttl|trig|nt|nq|jsonld)$/i

// The RDF files under `dir` the server indexes, relative to `root`, sorted.
function rdfFiles(dir) {
  return fs
    .readdirSync(dir, { recursive: true })
    .filter((f) => RDF.test(f) && fs.statSync(path.join(dir, f)).isFile())
    .map((f) => path.relative(root, path.join(dir, f)))
    .sort()
}

// One test per top-level file, one per folder (its files one at a time).
const groups = present
  ? fs
      .readdirSync(root)
      .sort()
      .flatMap((name) => {
        const full = path.join(root, name)
        if (fs.statSync(full).isDirectory()) {
          const files = rdfFiles(full)
          return files.length ? [[name + "/", files]] : []
        }
        return RDF.test(name) ? [[name, [name]]] : []
      })
  : []

describe("turtle/schema on each corpus file alone", { skip: !present && "test/corpus is absent (npm run corpus)" }, () => {
  let s, dir
  const seen = { files: 0, classes: 0, rows: 0, problems: 0, ats: 0, slowest: [0, ""] }
  before(async () => {
    dir = tempDir("turtle-lsp-schema-corpus-")
    s = await startServer({ cwd: dir })
  })
  after(async () => {
    await s?.stop()
    if (dir) fs.rmSync(dir, { recursive: true, force: true })
    if (present) console.log(`# schema, corpus files alone: ${JSON.stringify(seen)}`)
  })

  for (const [name, files] of groups) {
    test(name, async () => {
      for (const f of files) {
        const file = path.join(root, f)
        const uri = uriOf(file)
        const text = fs.readFileSync(file, "utf8")
        s.open(uri, text, languageOf(f))
        try {
          const r = await checkWorkspace(s, {
            budgetMs: 10000,
            textOf: (u) => (u === uri ? text : undefined),
            opened: { uri, text },
            expectUri: uri,
          })
          seen.files++
          seen.classes += r.classes
          seen.rows += r.rows
          seen.problems += r.problems
          seen.ats += r.ats
          if (r.ms > seen.slowest[0]) seen.slowest = [Math.round(r.ms), f]
        } catch (e) {
          e.message = `${f}: ${e.message}`
          throw e
        } finally {
          s.close(uri)
        }
      }
    })
  }
})

// The progress of the initial index: done once its `end` arrived.
function indexedOnce(s) {
  const begun = new Set()
  return () =>
    s.progress.some(({ token, value }) => {
      if (value.kind === "begin" && /index/i.test(value.title)) begun.add(token)
      return value.kind === "end" && begun.has(token)
    })
}

test("turtle/schema on the whole corpus as one workspace", { skip: !present && "test/corpus is absent (npm run corpus)", timeout: 100000 }, async () => {
  const s = await startServer({ cwd: root, capabilities: { window: { workDoneProgress: true } } })
  try {
    const done = indexedOnce(s)
    const end = Date.now() + 60000
    while (!done()) {
      assert.ok(Date.now() < end, "the corpus was not indexed within 60 s")
      await new Promise((r) => setTimeout(r, 50))
    }
    // A workspace file opened, then edited: the answer follows the edit.
    // Any Turtle file at the corpus root: the corpus differs between
    // machines (npm run corpus fetches what it can).
    const name = fs.readdirSync(root).filter((n) => n.endsWith(".ttl")).sort()[0]
    assert.ok(name, "no .ttl file at the corpus root")
    const file = path.join(root, name)
    const uri = uriOf(file)
    let text = fs.readFileSync(file, "utf8")
    const textOf = (u) => (u === uri ? text : undefined)
    s.open(uri, text)
    const r = await checkWorkspace(s, { budgetMs: 15000, textOf, opened: { uri, text } })
    console.log(`# schema, whole corpus: ${JSON.stringify(r)}`)
    assert.ok(r.classes > 100, `only ${r.classes} classes in the whole corpus`)
    const added = "http://example.org/schema-corpus/Edited"
    text += `\n<${added}> a <http://www.w3.org/2002/07/owl#Class> .\n`
    s.change(uri, text)
    const edited = await checkWorkspace(s, { budgetMs: 15000, textOf, opened: { uri, text } })
    assert.equal(edited.classes, r.classes + 1, "the class the edit added is not counted")
    const f = await s.request("turtle/schema", { focus: added, maxClasses: 1 })
    assert.equal(f.classes[f.focus].uri, uri)
  } finally {
    await s.stop()
  }
})
