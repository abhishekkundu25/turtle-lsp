// Label lints (`label-whitespace`): the warning and its trim fix over LSP,
// and the fix under `check --fix`, which applies it only with
// `--unsafe-fixes`.
import { test, describe, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { startServer, tempDir, uriOf, serverPath, serverEnv } from "./lsp-client.mjs"

const PREFIXES =
  "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n" +
  "@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n" +
  "@prefix ex: <http://example.org/> .\n"
const whitespace = (diags) => diags.filter((d) => d.code === "label-whitespace")

// Positions are UTF-16 and these texts are ASCII.
function apply(text, edits) {
  const lines = text.split("\n")
  const at = (p) => lines.slice(0, p.line).reduce((n, l) => n + l.length + 1, 0) + p.character
  const sorted = [...edits].sort((a, b) => at(b.range.start) - at(a.range.start))
  for (const e of sorted) text = text.slice(0, at(e.range.start)) + e.newText + text.slice(at(e.range.end))
  return text
}

describe("label-whitespace over LSP", () => {
  let ws, s
  before(async () => {
    ws = tempDir("turtle-label-")
    s = await startServer({ cwd: ws })
  })
  after(async () => {
    await s.stop()
    fs.rmSync(ws, { recursive: true, force: true })
  })

  test("a label with whitespace at its ends is a warning whose fix trims it", async () => {
    const uri = uriOf(path.join(ws, "a.ttl"))
    const text = `${PREFIXES}ex:a rdfs:label "\\tPump "@en ;\n    skos:altLabel """Kreisel\n"""@de ;\n    rdfs:comment " spaced " .\n`
    s.open(uri, text)
    const diags = whitespace(await s.diagnosticsFor(uri))
    assert.equal(diags.length, 2, JSON.stringify(diags))
    const [label, alt] = diags
    assert.equal(label.severity, 2)
    assert.equal(label.message, "Label starts and ends with whitespace")
    assert.deepEqual(label.range, { start: { line: 3, character: 16 }, end: { line: 3, character: 25 } })
    assert.equal(alt.message, "Label ends with whitespace")

    const fixed = []
    for (const d of diags) {
      const actions = await s.request("textDocument/codeAction", {
        textDocument: { uri },
        range: d.range,
        context: { diagnostics: [d] },
      })
      const fix = actions.find((a) => a.title === "Trim the label's whitespace")
      assert.ok(fix, actions.map((a) => a.title).join("\n"))
      assert.equal(fix.isPreferred, true)
      fixed.push(...fix.edit.changes[uri])
    }
    const after = apply(text, fixed)
    assert.equal(
      after,
      `${PREFIXES}ex:a rdfs:label "Pump"@en ;\n    skos:altLabel """Kreisel"""@de ;\n    rdfs:comment " spaced " .\n`,
    )
    s.change(uri, after)
    assert.deepEqual(whitespace(await s.diagnosticsFor(uri)), [])
    s.close(uri)
  })

  test("a JSON-LD label is trimmed in its string", async () => {
    const uri = uriOf(path.join(ws, "b.jsonld"))
    const text = `{\n  "@context": { "rdfs": "http://www.w3.org/2000/01/rdf-schema#" },\n  "@id": "http://example.org/b",\n  "rdfs:label": " Valve"\n}\n`
    s.open(uri, text)
    const [d] = whitespace(await s.diagnosticsFor(uri))
    assert.ok(d)
    const actions = await s.request("textDocument/codeAction", {
      textDocument: { uri },
      range: d.range,
      context: { diagnostics: [d] },
    })
    const fix = actions.find((a) => a.title === "Trim the label's whitespace")
    assert.ok(fix, actions.map((a) => a.title).join("\n"))
    assert.match(apply(text, fix.edit.changes[uri]), /"rdfs:label": "Valve"\n/)
    s.close(uri)
  })
})

test("check: --fix leaves the trim, --unsafe-fixes applies it", (t) => {
  const ws = tempDir("turtle-label-cli-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  const file = path.join(ws, "a.ttl")
  // Every prefix used: the safe fixes leave the file as it is.
  const prefixes = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix ex: <http://example.org/> .\n"
  const text = `${prefixes}ex:a rdfs:label " Pump" .\n`
  fs.writeFileSync(file, text)
  const run = (args) => {
    const r = spawnSync(serverPath, args, { cwd: ws, encoding: "utf8", timeout: 60000, env: { ...serverEnv(), NO_COLOR: "1" } })
    return { code: r.status, lines: r.stdout.split("\n").filter(Boolean), err: r.stderr }
  }
  const safe = run(["check", "--fix", "--no-shacl"])
  assert.equal(fs.readFileSync(file, "utf8"), text)
  assert.equal(safe.code, 0, "the trim is left as a warning")
  assert.equal(safe.lines.length, 1, safe.lines.join("\n"))
  assert.match(safe.lines[0], /^a\.ttl:3:17: warning\[label-whitespace\]/)
  const all = run(["check", "--fix", "--unsafe-fixes", "--no-shacl"])
  assert.equal(all.code, 0)
  assert.deepEqual(all.lines, [])
  assert.match(all.err, /Applied 1 fix in 1 file/)
  assert.equal(fs.readFileSync(file, "utf8"), `${prefixes}ex:a rdfs:label "Pump" .\n`)
})

// The last diagnostics published for `uri` that `pred` accepts, waiting up to
// `ms` for one: a document another file's edit affects is diagnosed again
// after a pause, without a change of its own.
async function publishedUntil(s, uri, pred, ms = 10000) {
  const end = Date.now() + ms
  for (;;) {
    const last = s.publications.filter((p) => p.uri === uri).at(-1)
    if (last && pred(last.diagnostics)) return last.diagnostics
    if (Date.now() > end) throw new Error(`no matching diagnostics for ${uri}: ${JSON.stringify(last?.diagnostics)}`)
    await new Promise((r) => setTimeout(r, 25))
  }
}

const OWL = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
const duplicates = (diags) => diags.filter((d) => d.code === "duplicate-label")

test("a duplicate label across two open files clears in both when one is renamed", async (t) => {
  const ws = tempDir("turtle-label-dup-")
  const s = await startServer({ cwd: ws })
  t.after(async () => {
    await s.stop()
    fs.rmSync(ws, { recursive: true, force: true })
  })
  const a = uriOf(path.join(ws, "a.ttl"))
  const b = uriOf(path.join(ws, "b.ttl"))
  s.open(a, `${OWL}${PREFIXES}ex:Pump a owl:Class ; rdfs:label "Pump"@en .\n`)
  await s.diagnosticsFor(a)
  s.open(b, `${OWL}${PREFIXES}ex:Pump2 a owl:Class ; skos:prefLabel "pump"@en .\n`)
  const inB = duplicates(await s.diagnosticsFor(b))
  assert.equal(inB.length, 1, JSON.stringify(inB))
  assert.equal(inB[0].severity, 2)
  assert.equal(inB[0].message, 'Label "pump" is also the label of ex:Pump')
  // a.ttl did not change, but b.ttl now collides with it.
  const inA = duplicates(await publishedUntil(s, a, (d) => duplicates(d).length === 1))
  assert.equal(inA[0].message, 'Label "Pump" is also the label of ex:Pump2')
  assert.deepEqual(inA[0].range, { start: { line: 4, character: 33 }, end: { line: 4, character: 39 } })

  s.change(b, `${OWL}${PREFIXES}ex:Pump2 a owl:Class ; skos:prefLabel "Valve"@en .\n`)
  assert.deepEqual(duplicates(await s.diagnosticsFor(b)), [])
  await publishedUntil(s, a, (d) => duplicates(d).length === 0)
})

test("lint.enable in turtle-lsp.json turns on missing-label", async (t) => {
  const ws = tempDir("turtle-label-enable-")
  const text = `${OWL}${PREFIXES}ex:Pump a owl:Class .\n`
  // The missing-* checks wait for the initial index: each run enables
  // missing-definition too, and reads the publication that has it.
  const missing = async (lint) => {
    fs.writeFileSync(path.join(ws, "turtle-lsp.json"), JSON.stringify({ lint }))
    const s = await startServer({ cwd: ws })
    try {
      const uri = uriOf(path.join(ws, "a.ttl"))
      s.open(uri, text)
      const indexed = await publishedUntil(s, uri, (d) => d.some((x) => x.code === "missing-definition"))
      return indexed.filter((d) => d.code === "missing-label")
    } finally {
      await s.stop()
    }
  }
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  assert.deepEqual(await missing({ enable: ["missing-definition"] }), [], "off by default")
  const [d] = await missing({ enable: ["missing-label", "missing-definition"] })
  assert.ok(d)
  assert.equal(d.severity, 2)
  assert.equal(d.message, "ex:Pump has no rdfs:label or skos:prefLabel")
  assert.deepEqual(d.range, { start: { line: 4, character: 0 }, end: { line: 4, character: 7 } })
  assert.deepEqual(
    await missing({ enable: ["missing-label", "missing-definition"], ignore: ["missing-label"] }),
    [],
    "lint.ignore wins",
  )
})
