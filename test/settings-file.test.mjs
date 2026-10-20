// The workspace settings file `turtle-lsp.json`: a layer under the client's
// settings (defaults < file < initializationOptions < configuration section).
//
// Workspace: a copy of test/fixtures/project plus core/__more.ttl (as in
// lint.test.mjs), so a misspelled sd: term gets an `undescribed-term` warning
// unless `lint.undescribedTerms` is off.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const lint = (diags) => diags.filter((d) => d.code === "undescribed-term")
const typo = "obj:m1 sd:fillsPositon obj:pos-mc2 .\n"

function workspace(settings) {
  const ws = tempDir("turtle-settings-")
  fs.cpSync(path.join(fixtures, "project"), ws, { recursive: true })
  fs.writeFileSync(path.join(ws, "core", "__more.ttl"), "sd:threshold a owl:DatatypeProperty .\nsd:active a owl:DatatypeProperty .\n")
  if (settings !== undefined) fs.writeFileSync(path.join(ws, "turtle-lsp.json"), settings)
  return ws
}

async function until(get, ok, what) {
  let last
  for (let i = 0; i < 200; i++) {
    last = await get()
    if (ok(last)) return last
    await sleep(25)
  }
  assert.fail(`${what}: ${JSON.stringify(last)}`)
}

// Starts a server on `ws`, opens a file with a misspelled term, and returns
// the undescribed-term diagnostics with the session.
async function session(ws, initializationOptions) {
  const s = await startServer({ cwd: ws, initializationOptions })
  await s.request("workspace/symbol", { query: "" })
  const uri = uriOf(path.join(ws, "data", "__typo.ttl"))
  s.open(uri, typo)
  return { s, uri, diags: lint(await s.diagnosticsFor(uri)) }
}

async function done(s, ws) {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
}

test("a setting in turtle-lsp.json changes diagnostics", async () => {
  const ws = workspace(JSON.stringify({ "lint.undescribedTerms": false }))
  const { s, diags } = await session(ws)
  assert.deepEqual(diags, [])
  assert.deepEqual(s.messages, [])
  await done(s, ws)
})

test("the client's settings override turtle-lsp.json", async () => {
  const ws = workspace(JSON.stringify({ turtle: { lint: { undescribedTerms: false } } }))
  const { s, uri, diags } = await session(ws, { lint: { undescribedTerms: true } })
  assert.equal(diags.length, 1, JSON.stringify(diags))
  // The configuration section is the top layer.
  s.notify("workspace/didChangeConfiguration", { settings: { turtle: { lint: { undescribedTerms: false } } } })
  await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 0, "still flagged with the lint off")
  await done(s, ws)
})

test("an invalid turtle-lsp.json warns once and falls back to the defaults", async () => {
  const ws = workspace("{ lint: off")
  const { s, diags } = await session(ws)
  assert.equal(diags.length, 1, JSON.stringify(diags))
  assert.equal(s.messages.length, 1, JSON.stringify(s.messages))
  assert.equal(s.messages[0].type, 2)
  assert.match(s.messages[0].message, /turtle-lsp\.json/)
  await done(s, ws)
})

test("editing turtle-lsp.json on disk applies the change", async () => {
  const ws = workspace()
  const { s, uri, diags } = await session(ws)
  assert.equal(diags.length, 1, JSON.stringify(diags))
  const file = path.join(ws, "turtle-lsp.json")
  fs.writeFileSync(file, JSON.stringify({ lint: { undescribedTerms: false } }))
  s.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: uriOf(file), type: 1 }] })
  await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 0, "still flagged after the file turned the lint off")
  fs.rmSync(file)
  s.notify("workspace/didChangeWatchedFiles", { changes: [{ uri: uriOf(file), type: 3 }] })
  await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 1, "not flagged after the file was deleted")
  assert.deepEqual(s.messages, [])
  await done(s, ws)
})
