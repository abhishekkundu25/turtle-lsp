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
import { startServer, tempDir, uriOf, fixtures } from "./lsp-client.mjs"

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const lint = (diags) => diags.filter((d) => d.code === "undescribed-term")
const more = "sd:threshold a owl:DatatypeProperty .\nsd:active a owl:DatatypeProperty .\n"

function workspace() {
  const ws = tempDir("turtle-lint-")
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

test("1,000 undescribed terms against a 30,000-term namespace are diagnosed in under 50 ms", async () => {
  const NS = "https://ontology.example.com/big#"
  // Letters-only names of 9-16 characters, all distinct: a typo of one
  // differs from the others by letters, not only by digits.
  const letters = "abcdefghijklmnopqrstuvwxyz"
  const name = (i) => {
    let s = ""
    let n = i
    for (let k = 0; k < 4; k++, n = Math.floor(n / 26)) s += letters[n % 26]
    for (let k = 0, h = i * 2654435761; k < 5 + (i % 8); k++, h = Math.floor(h / 7) + k * 13) s += letters[(h >>> 0) % 26]
    return s
  }
  const ws = tempDir("turtle-lint-big-")
  const s0 = performance.now()
  const defs = []
  for (let i = 0; i < 30_000; i++) defs.push(`ex:${name(i)} a owl:Class .`)
  fs.writeFileSync(
    path.join(ws, "big.ttl"),
    `@prefix ex: <${NS}> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n${defs.join("\n")}\n`,
  )
  const s = await startServer({ cwd: ws })
  try {
    await s.request("workspace/symbol", { query: "" })
    // 1,000 distinct names that nothing describes, each one letter off a
    // described name (the first letter is kept: ex:Signal is not ex:signal).
    const uses = []
    for (let i = 0; i < 1000; i++) {
      const n = name(i * 29)
      uses.push(`inst:i${i} a ex:${n.slice(0, 6)}${n[6] === "q" ? "z" : "q"}${n.slice(7)} .`)
    }
    const text = `@prefix ex: <${NS}> .\n@prefix inst: <https://ontology.example.com/inst#> .\n${uses.join("\n")}\n`
    const uri = uriOf(path.join(ws, "use.ttl"))
    // Warm up: the first diagnosis of a file also loads the index.
    s.open(uri, text + "\n")
    await s.diagnosticsFor(uri, 30_000)
    let best = Infinity
    let diags
    for (let round = 0; round < 3; round++) {
      const t0 = performance.now()
      s.change(uri, text + `# ${round}\n`)
      diags = await s.diagnosticsFor(uri, 30_000)
      best = Math.min(best, performance.now() - t0)
    }
    console.log(`# lint: 1,000 undescribed terms / 30,000 described diagnosed in ${best.toFixed(0)} ms`)
    const flagged = lint(diags)
    assert.equal(flagged.length, 1000)
    // The first 100 flagged terms get a search; the rest stay information.
    const suggested = flagged.filter((d) => d.severity === 2)
    assert.ok(suggested.length > 0 && suggested.length <= 100, `${suggested.length} suggestions`)
    assert.ok(flagged.slice(0, 100).some((d) => d.severity === 2))
    assert.ok(flagged.slice(100).every((d) => d.severity === 3 && d.data === undefined))
    assert.ok(best < 50, `took ${best.toFixed(0)} ms`)
  } finally {
    await s.stop()
    fs.rmSync(ws, { recursive: true, force: true })
  }
  assert.ok(performance.now() - s0 < 60_000)
})
