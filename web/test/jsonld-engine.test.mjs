// JSON-LD in the browser engine: the same diagnostics, at the same
// positions, as the language server, with contexts from the bundle (the
// browser never fetches one). Each test is pending (`todo`), tagged with its
// phase and the ticket in docs/tickets/jsonld/ that promotes it; see
// test/jsonld.test.mjs. Run `npm run web:build` first.
import { test, before } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createEngine } from "../app/engine.js"
import { SETS } from "../app/samples.js"
import { startServer, tempDir, uriOf } from "../../test/lsp-client.mjs"
import * as F from "../../test/jsonld-fixtures.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const phase1 = (ticket) => ({ todo: `jsonld:phase1 (ticket ${ticket})`, timeout: 40_000 })
const phase2 = { todo: "jsonld:phase2", timeout: 40_000 }

let engine
before(async () => {
  engine = await createEngine(fs.readFileSync(path.join(here, "..", "app", "engine.wasm")))
})

const analyse = (files) => engine.call("analyse", { files })
const fileOf = (res, name) => res.files.find((f) => f.name === name)
const rangesOf = (f, code) => f.diagnostics.filter((d) => d.code === code).map((d) => d.range)
const sortRanges = (rs) => [...rs].sort((a, b) => F.rangeKey(a).localeCompare(F.rangeKey(b)))
const show = (f) => JSON.stringify(f.diagnostics.map((d) => `${F.rangeKey(d.range)} ${d.severity} ${d.code}`))
const languageOf = (name) => (name.endsWith(".jsonld") ? "jsonld" : /\.(rq|ru|sparql)$/.test(name) ? "sparql" : "turtle")
const key = (d) => `${F.rangeKey(d.range)} ${d.severity} ${d.code}`

test("JSON syntax errors at the same positions as in the language server", { timeout: 40_000 }, () => {
  const f = fileOf(analyse([{ name: "broken.jsonld", text: F.SYNTAX }]), "broken.jsonld")
  assert.equal(f.kind, "jsonld")
  assert.deepEqual(sortRanges(rangesOf(f, "json-syntax")), [F.SYNTAX_MISSING_COMMA, F.SYNTAX_BAD_LITERAL], show(f))
})

test("bundled contexts load in the browser; any other remote context is reported at its URL", { timeout: 40_000 }, () => {
  const files = F.BUNDLED_CONTEXTS.map((url, i) => ({ name: `bundled-${i}.jsonld`, text: F.remoteDoc(url) }))
  files.push({ name: "unknown.jsonld", text: F.remoteDoc(F.UNKNOWN_CONTEXT_URL) })
  const res = analyse(files)
  for (let i = 0; i < F.BUNDLED_CONTEXTS.length; i++) {
    const f = fileOf(res, `bundled-${i}.jsonld`)
    assert.deepEqual(rangesOf(f, "jsonld-context"), [], `${F.BUNDLED_CONTEXTS[i]}: ${show(f)}`)
  }
  const unknown = fileOf(res, "unknown.jsonld")
  assert.deepEqual(rangesOf(unknown, "jsonld-context"), [F.remoteDocUrl(F.UNKNOWN_CONTEXT_URL)], show(unknown))
})

test("a context from another file of the workspace is loaded", phase1("06"), () => {
  const res = analyse([
    { name: "people.context.jsonld", text: F.CTX_FILE },
    { name: "uses.jsonld", text: F.USES_CTX_FILE },
  ])
  const f = fileOf(res, "uses.jsonld")
  assert.deepEqual(rangesOf(f, "jsonld-context"), [], show(f))
  assert.deepEqual(rangesOf(f, "jsonld-undefined-term"), [F.USES_CTX_FILE_TYPO], show(f))
})

test("JSON-LD lints at their positions", { timeout: 40_000 }, () => {
  const f = fileOf(analyse([{ name: "lints.jsonld", text: F.LINTS }]), "lints.jsonld")
  assert.deepEqual(rangesOf(f, "jsonld-undefined-term"), [F.LINTS_UNDEFINED], show(f))
  assert.deepEqual(sortRanges(rangesOf(f, "unknown-term")), sortRanges([F.LINTS_DEF_TYPO, F.LINTS_KEY_TYPO]), show(f))
  assert.deepEqual(rangesOf(f, "deprecated-term"), [F.LINTS_DEPRECATED], show(f))
  assert.deepEqual(rangesOf(f, "invalid-iri"), [F.LINTS_BAD_IRI], show(f))
  assert.deepEqual(rangesOf(f, "invalid-language-tag"), [F.LINTS_BAD_LANG], show(f))
})

test("Turtle and JSON-LD files know each other's terms", { timeout: 40_000 }, () => {
  const res = analyse([
    { name: "vocab.jsonld", text: F.VOCAB },
    { name: "uses.ttl", text: F.USES },
    { name: "back.jsonld", text: F.BACK },
  ])
  const uses = fileOf(res, "uses.ttl")
  // Described in vocab.jsonld: only the typo is flagged, and the import resolves.
  assert.deepEqual(rangesOf(uses, "undescribed-term"), [F.USES_GADGT], show(uses))
  assert.deepEqual(rangesOf(uses, "unresolved-import"), [], show(uses))
  const back = fileOf(res, "back.jsonld")
  assert.deepEqual(rangesOf(back, "undescribed-term"), [F.BACK_TYPO], show(back))
})

test("Turtle shapes validate JSON-LD data, each result on what breaks the shape", { timeout: 40_000 }, () => {
  const res = analyse([
    { name: "shapes.ttl", text: F.PERSON_SHAPES },
    { name: "people.jsonld", text: F.PEOPLE },
  ])
  assert.equal(res.hasShapes, true)
  const f = fileOf(res, "people.jsonld")
  assert.deepEqual(sortRanges(rangesOf(f, "shacl-violation")), sortRanges(F.PEOPLE_RESULTS), show(f))
})

test("shapes written in JSON-LD are used", { timeout: 40_000 }, () => {
  const res = analyse([
    { name: "shapes.jsonld", text: F.JSONLD_SHAPES },
    { name: "data.ttl", text: F.THING_DATA },
  ])
  assert.equal(res.hasShapes, true)
  const f = fileOf(res, "data.ttl")
  assert.deepEqual(rangesOf(f, "shacl-violation"), [F.THING_T1], show(f))
})

test("a sample workspace holds a .jsonld file", phase1("09"), () => {
  const names = Object.values(SETS).flatMap((set) => set.files.map((f) => f.name))
  assert.ok(names.some((n) => n.endsWith(".jsonld")), names.join(", "))
})

// The language server's diagnostics for `files`, read once publishing has
// settled (as web/test/engine.test.mjs does).
async function serverDiagnostics(files) {
  const dir = tempDir("turtle-web-jsonld-")
  for (const f of files) fs.writeFileSync(path.join(dir, f.name), f.text)
  const s = await startServer({ cwd: dir })
  try {
    await s.request("workspace/symbol", { query: "" })
    for (const f of files) s.open(uriOf(path.join(dir, f.name)), f.text, languageOf(f.name))
    for (const f of files) await s.diagnosticsFor(uriOf(path.join(dir, f.name)), 10_000)
    const end = Date.now() + 10_000
    let n = -1
    while (n !== s.publications.length && Date.now() < end) {
      n = s.publications.length
      await new Promise((r) => setTimeout(r, 2000))
    }
    const out = {}
    for (const f of files) out[f.name] = await s.diagnosticsFor(uriOf(path.join(dir, f.name)))
    return out
  } finally {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
}

test("the engine reports what the language server reports on a mixed Turtle and JSON-LD workspace", { timeout: 40_000 }, async () => {
  const files = [
    { name: "vocab.jsonld", text: F.VOCAB },
    { name: "uses.ttl", text: F.USES },
    { name: "lints.jsonld", text: F.LINTS },
    { name: "broken.jsonld", text: F.SYNTAX },
    { name: "shapes.ttl", text: F.PERSON_SHAPES },
    { name: "people.jsonld", text: F.PEOPLE },
  ]
  const got = analyse(files)
  for (const name of ["lints.jsonld", "broken.jsonld", "people.jsonld"]) {
    assert.ok(fileOf(got, name).diagnostics.length > 0, `the engine reports ${name}`)
  }
  const want = await serverDiagnostics(files)
  for (const f of got.files) assert.deepEqual(f.diagnostics.map(key).sort(), want[f.name].map(key).sort(), f.name)
})

test("JSON-LD is formatted with its graph kept, and formatted text stays as it is", phase2, () => {
  const f = fileOf(analyse([{ name: "messy.jsonld", text: F.MESSY }]), "messy.jsonld")
  assert.equal(f.format?.status, "changed", JSON.stringify(f.format))
  assert.equal(f.format.guarantee, "same graph")
  assert.deepEqual(JSON.parse(f.format.text), JSON.parse(F.MESSY))
  const again = fileOf(analyse([{ name: "messy.jsonld", text: f.format.text }]), "messy.jsonld")
  assert.equal(again.format.status, "unchanged")
})
