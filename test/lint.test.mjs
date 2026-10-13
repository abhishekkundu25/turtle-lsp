// Workspace lint (`undescribed-term`): refs to terms in an own namespace that
// nothing in the workspace describes.
//
// Workspace: a copy of test/fixtures/project plus core/__more.ttl, which
// describes the two sd: terms the fixture only uses, so every sd: term is
// described and sd: is an own namespace.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { startServer, uriOf, fixtures } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const lint = (diags) => diags.filter((d) => d.code === "undescribed-term")
const more = "sd:threshold a owl:DatatypeProperty .\nsd:active a owl:DatatypeProperty .\n"

function workspace() {
  const ws = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-lint-"))
  fs.cpSync(path.join(fixtures, "project"), ws, { recursive: true })
  fs.writeFileSync(path.join(ws, "core", "__more.ttl"), more)
  return ws
}

// Positions are UTF-16 and these texts are ASCII.
function apply(text, edit) {
  const lines = text.split("\n")
  const at = (p) => lines.slice(0, p.line).reduce((n, l) => n + l.length + 1, 0) + p.character
  return text.slice(0, at(edit.range.start)) + edit.newText + text.slice(at(edit.range.end))
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

describe("push", () => {
  let ws, s
  const file = (rel) => uriOf(path.join(ws, rel))
  before(async () => {
    ws = workspace()
    s = await startServer({ cwd: ws })
    await s.request("workspace/symbol", { query: "" })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(ws, { recursive: true, force: true })
  })

  test("a typo of a described term is a warning whose fix clears it", async () => {
    const uri = file("data/__typo.ttl")
    const text = "obj:m1 a sd:Asset ;\n    sd:fillsPositon obj:pos-mc2 .\n"
    s.open(uri, text)
    const diags = lint(await s.diagnosticsFor(uri))
    assert.equal(diags.length, 1, JSON.stringify(diags))
    const d = diags[0]
    assert.equal(d.severity, 2)
    assert.equal(d.message, "'sd:fillsPositon' is not described anywhere in the workspace. Did you mean 'sd:fillsPosition'?")
    assert.deepEqual(d.range, { start: { line: 1, character: 4 }, end: { line: 1, character: 19 } })

    const actions = await s.request("textDocument/codeAction", {
      textDocument: { uri },
      range: d.range,
      context: { diagnostics: [d] },
    })
    const fix = actions.find((a) => a.title === "Change to 'sd:fillsPosition'")
    assert.ok(fix, actions.map((a) => a.title).join("\n"))
    assert.equal(fix.isPreferred, true)
    const fixed = apply(text, fix.edit.changes[uri][0])
    assert.equal(fixed, "obj:m1 a sd:Asset ;\n    sd:fillsPosition obj:pos-mc2 .\n")
    s.change(uri, fixed)
    assert.deepEqual(lint(await s.diagnosticsFor(uri)), [])
    s.close(uri)
  })

  test("an undescribed term with no close match is information; a full IRI gets an IRI fix", async () => {
    const uri = file("data/__info.ttl")
    const text = "obj:m2 sd:unheardOf obj:x ;\n    <https://ontology.example.com/plant/core/hasFunctoin> obj:y .\n"
    s.open(uri, text)
    const diags = lint(await s.diagnosticsFor(uri))
    assert.equal(diags.length, 2, JSON.stringify(diags))
    assert.equal(diags[0].severity, 3)
    assert.equal(diags[0].message, "'sd:unheardOf' is not described anywhere in the workspace")
    assert.equal(diags[0].data, undefined)
    assert.equal(diags[1].severity, 2)
    assert.deepEqual(diags[1].data, { replace: "<https://ontology.example.com/plant/core/hasFunction>" })
    s.close(uri)
  })

  test("described terms, vocabulary terms, subject refs and other namespaces are not flagged", async () => {
    const uri = file("data/__clean.ttl")
    const text = [
      // Subject position: this describes sd:brandNew.
      "sd:brandNew a owl:Class ; rdfs:subClassOf sd:PhysicalObject .",
      "obj:m3 a sd:brandNew , sosa:Sensor ;",
      "    sd:locatedIn obj:pos-mc2 ;",
      "    sosa:observes obj:temp ;",
      // obj: is an instance namespace: mostly referenced, not described.
      "    sd:hasFunction obj:nowhere ;",
      "    <http://example.com/elsewhere/p> 1 .",
      "",
    ].join("\n")
    s.open(uri, text)
    assert.deepEqual(lint(await s.diagnosticsFor(uri)), [])
    s.close(uri)
  })

  test("turning lint.undescribedTerms off removes the diagnostics, on brings them back", async () => {
    const uri = file("data/__toggle.ttl")
    s.open(uri, "obj:m4 sd:fillsPositon obj:pos-mc2 .\n")
    assert.equal(lint(await s.diagnosticsFor(uri)).length, 1)
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { lint: { undescribedTerms: false } } } })
    await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 0, "still flagged with the lint off")
    s.notify("workspace/didChangeConfiguration", { settings: { turtle: { lint: { undescribedTerms: true } } } })
    await until(() => s.diagnosticsFor(uri), (d) => lint(d).length === 1, "not flagged with the lint on")
    s.close(uri)
  })

  test("describing the term in another open file clears the warning here", async () => {
    const use = file("data/__use.ttl")
    const def = file("core/__def.ttl")
    s.open(use, "obj:m5 sd:fillsPositon obj:pos-mc2 .\n")
    assert.equal(lint(await s.diagnosticsFor(use)).length, 1)
    s.open(def, "sd:fillsPositon a owl:ObjectProperty .\n")
    await s.diagnosticsFor(def)
    await until(() => s.diagnosticsFor(use), (d) => lint(d).length === 0, "still flagged after the term was described")
    // Undescribing it again brings the warning back.
    s.change(def, "sd:other a owl:ObjectProperty .\n")
    await until(() => s.diagnosticsFor(use), (d) => lint(d).length === 1, "not flagged after the description went")
    s.close(def)
    s.close(use)
  })

  test("keystrokes that leave the described set alone do not re-diagnose other files", async () => {
    const a = file("data/__quiet-a.ttl")
    const b = file("data/__quiet-b.ttl")
    s.open(a, "obj:q1 sd:locatedIn obj:pos-mc2 .\n")
    s.open(b, "obj:q2 sd:locatedIn obj:pos-mc2 .\n")
    await s.diagnosticsFor(a)
    await s.diagnosticsFor(b)
    s.published.length = 0
    for (const o of ["obj:pos-mc", "obj:pos-m", "obj:pos-mc2"]) s.change(a, `obj:q1 sd:locatedIn ${o} .\n`)
    await s.diagnosticsFor(a)
    await s.request("workspace/symbol", { query: "zz" })
    assert.ok(!s.published.includes(b), s.published.join("\n"))
    s.close(a)
    s.close(b)
  })
})

describe("pull", () => {
  let ws, s
  const file = (rel) => uriOf(path.join(ws, rel))
  before(async () => {
    ws = workspace()
    s = await startServer({
      cwd: ws,
      capabilities: {
        textDocument: { diagnostic: { dynamicRegistration: false } },
        workspace: { diagnostics: { refreshSupport: true } },
      },
    })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(ws, { recursive: true, force: true })
  })
  const pull = (uri, previousResultId) =>
    s.request("textDocument/diagnostic", { textDocument: { uri }, previousResultId })

  test("describing the term elsewhere asks the client to pull again", async () => {
    const use = file("data/__use.ttl")
    const def = file("core/__def.ttl")
    s.open(use, "obj:m6 sd:fillsPositon obj:pos-mc2 .\n")
    const first = await pull(use)
    assert.equal(lint(first.items).length, 1, JSON.stringify(first.items))
    s.serverRequests.length = 0
    s.open(def, "sd:fillsPositon a owl:ObjectProperty .\n")
    await pull(def)
    await until(
      async () => s.serverRequests.map((r) => r.method),
      (m) => m.includes("workspace/diagnostic/refresh"),
      "no refresh",
    )
    const again = await pull(use, first.resultId)
    assert.equal(again.kind, "full")
    assert.deepEqual(lint(again.items), [])
  })
})

test("typing a new subject re-diagnoses other open files once typing pauses, not per keystroke", async () => {
  const ws = workspace()
  const s = await startServer({ cwd: ws })
  try {
    await s.request("workspace/symbol", { query: "" })
    const other = uriOf(path.join(ws, "data", "__other.ttl"))
    s.open(other, "obj:a sd:fillsPosition obj:b .\n")
    await s.diagnosticsFor(other)
    const mine = uriOf(path.join(ws, "data", "__typing.ttl"))
    s.open(mine, "")
    await s.diagnosticsFor(mine)
    const before = s.published.filter((u) => u === other).length
    // Each prefix of the new name is a new described subject.
    const name = "obj:newThing"
    for (let i = 5; i <= name.length; i++) {
      s.change(mine, `${name.slice(0, i)} a sd:Asset .\n`)
      await s.diagnosticsFor(mine)
    }
    await sleep(600)
    const republished = s.published.filter((u) => u === other).length - before
    assert.ok(republished <= 2, `other file re-published ${republished} times for ${name.length - 4} keystrokes`)
  } finally {
    await s.stop()
    fs.rmSync(ws, { recursive: true, force: true })
  }
})
