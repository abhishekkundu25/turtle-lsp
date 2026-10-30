// `turtle/schema` over the real-world fragments of test/fixtures/real/
// (local-only, gitignored; see fixtures/README.md): each file alone with
// the shared prefix header, then the folder as one workspace. Each answer
// is checked by test/schema-invariants.mjs. Skipped when the folder is not
// there.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, tempDir, fixtures } from "./lsp-client.mjs"
import { checkWorkspace } from "./schema-invariants.mjs"

const dir = path.join(fixtures, "real")
const present = fs.existsSync(path.join(dir, "prefixes.ttl"))
const skip = !present && "test/fixtures/real not present (local-only)"
const header = present ? fs.readFileSync(path.join(dir, "prefixes.ttl"), "utf8") : ""
const files = present ? fs.readdirSync(dir).filter((f) => f.endsWith(".ttl") && f !== "prefixes.ttl").sort() : []

describe("turtle/schema on each real fragment alone", { skip }, () => {
  let s, empty
  const seen = { files: 0, classes: 0, rows: 0, problems: 0, ats: 0 }
  before(async () => {
    empty = tempDir("turtle-lsp-schema-real-")
    s = await startServer({ cwd: empty })
  })
  after(async () => {
    await s?.stop()
    if (empty) fs.rmSync(empty, { recursive: true, force: true })
    if (present) console.log(`# schema, real fragments alone: ${JSON.stringify(seen)}`)
  })

  for (const f of files) {
    test(f, async () => {
      // Not a file on disk: the fragment behind the header, as one document.
      const uri = uriOf(path.join(empty, `__schema_${f}`))
      const text = header + fs.readFileSync(path.join(dir, f), "utf8")
      s.open(uri, text)
      try {
        const r = await checkWorkspace(s, {
          budgetMs: 10000,
          textOf: (u) => (u === uri ? text : undefined),
          opened: { uri, text },
          expectUri: uri,
        })
        for (const k of ["classes", "rows", "problems", "ats"]) seen[k] += r[k]
        seen.files++
      } finally {
        s.close(uri)
      }
    })
  }
})

test("turtle/schema on the real fragments as one workspace", { skip, timeout: 60000 }, async () => {
  const s = await startServer({ cwd: dir, capabilities: { window: { workDoneProgress: true } } })
  try {
    const begun = new Set()
    const done = () =>
      s.progress.some(({ token, value }) => {
        if (value.kind === "begin" && /index/i.test(value.title)) begun.add(token)
        return value.kind === "end" && begun.has(token)
      })
    const end = Date.now() + 30000
    while (!done()) {
      assert.ok(Date.now() < end, "the fragments were not indexed within 30 s")
      await new Promise((r) => setTimeout(r, 50))
    }
    const uri = uriOf(path.join(dir, files[0]))
    const text = fs.readFileSync(path.join(dir, files[0]), "utf8")
    s.open(uri, text)
    const r = await checkWorkspace(s, { budgetMs: 10000, textOf: (u) => (u === uri ? text : undefined), opened: { uri, text } })
    console.log(`# schema, real fragments as one workspace: ${JSON.stringify(r)}`)
  } finally {
    await s.stop()
  }
})
