// Hover with reasoning (`turtle.reasoning.rules`): after today's hover on a
// resource, an "Inferred" section lists each inferred type with its proof,
// each step naming its W3C rule and each base triple a link to its line.
// With reasoning off, hover is today's.
//
// The run is in the background: each test hovers again until the section
// is there (no run yet, a run writing the state, or a proof still being
// searched for all answer without it, or with a note).
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { createRequire } from "node:module"
import { startServer, tempDir, uriOf, positionOf } from "./lsp-client.mjs"

const HEADER =
  "@prefix ex: <http://example.org/> .\n" +
  "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n" +
  "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n"
const ONTO = `${HEADER}\nex:Dog rdfs:subClassOf ex:Animal .\n`
const DATA = `${HEADER}ex:rex a ex:Dog ;\n  rdfs:label "Rex" .\nex:a owl:sameAs ex:b .\nex:b owl:sameAs ex:c .\nex:c a ex:C .\n`

let ws, on, off, ontoUri, dataUri
before(async () => {
  ws = tempDir("turtle-reasoning-hover-")
  fs.writeFileSync(path.join(ws, "onto.ttl"), ONTO)
  fs.writeFileSync(path.join(ws, "data.ttl"), DATA)
  ontoUri = uriOf(path.join(ws, "onto.ttl"))
  dataUri = uriOf(path.join(ws, "data.ttl"))
  on = await startServer({ cwd: ws, initializationOptions: { reasoning: { rules: "default" } } })
  // Reasoning is on by default: "off" is asked for.
  off = await startServer({ cwd: ws, initializationOptions: { reasoning: { rules: "off" } } })
  for (const s of [on, off]) {
    await s.request("workspace/symbol", { query: "" })
    s.open(dataUri, DATA)
    await s.diagnosticsFor(dataUri)
  }
})
after(async () => {
  await on?.stop()
  await off?.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

async function hover(s, needle) {
  const h = await s.request("textDocument/hover", { textDocument: { uri: dataUri }, position: positionOf(DATA, needle, 3) })
  return h.contents.value
}

// The hover on `needle` once its proofs are in; fails after `timeoutMs`.
async function inferred(needle, timeoutMs = 20000) {
  const end = Date.now() + timeoutMs
  let md
  for (;;) {
    md = await hover(on, needle)
    if (md.includes("**Inferred**\n\n- ") && !md.includes("still being worked out")) return md
    if (Date.now() > end) throw new Error(`no proofs within ${timeoutMs} ms:\n${md}`)
    await new Promise((r) => setTimeout(r, 100))
  }
}

// The line a `[file:line](uri#Lline)` link to `uri` names, counted from 1.
function linkedLine(md, uri, before) {
  const at = md.indexOf(before)
  assert.ok(at >= 0, `${before} not in\n${md}`)
  const m = md.slice(at).match(/\]\(([^)#]+)#L(\d+)\)/)
  assert.ok(m, md)
  assert.equal(m[1], uri, md)
  return Number(m[2])
}

test("an inferred type comes with its proof, its rule and a link to the axiom's line", async () => {
  const md = await inferred("ex:rex a")
  const today = await hover(off, "ex:rex a")
  assert.ok(!today.includes("Inferred"), today)
  // The section comes after today's hover, which it leaves as it was.
  assert.ok(md.startsWith(`${today}\n\n**Inferred**\n\n`), md)
  assert.ok(md.includes("- **ex:Animal** · _cax-sco_"), md)
  // The asserted type is not repeated.
  assert.ok(!md.includes("- **ex:Dog**"), md)
  const axiom = linkedLine(md, ontoUri, "`ex:Dog rdfs:subClassOf ex:Animal`")
  assert.match(ONTO.split("\n")[axiom - 1], /ex:Dog rdfs:subClassOf ex:Animal/)
  const fact = linkedLine(md, dataUri, "`ex:rex a ex:Dog`")
  assert.match(DATA.split("\n")[fact - 1], /ex:rex a ex:Dog/)
})

test("a type through owl:sameAs shows the chain of sameAs links", async () => {
  const md = await inferred("ex:a owl")
  assert.ok(md.includes("- **ex:C** · _eq-rep-s_"), md)
  for (const link of ["`ex:a owl:sameAs ex:b`", "`ex:b owl:sameAs ex:c`", "`ex:c a ex:C`"]) {
    const line = linkedLine(md, dataUri, link)
    assert.ok(DATA.split("\n")[line - 1].includes(link.slice(1, -1)), `${link} at ${line}`)
  }
})

test("with reasoning off, hover is today's", async () => {
  for (const needle of ["ex:rex a", "ex:a owl", "ex:Dog ;"]) {
    const today = await hover(off, needle)
    assert.ok(!today.includes("Inferred"), today)
    const md = await hover(on, needle)
    assert.equal(md.split("\n\n**Inferred**")[0], today, needle)
  }
})

test("in a JSON-LD document the proofs are named with its @context, the closest type first, each step once", async () => {
  const dir = tempDir("turtle-reasoning-hover-jsonld-")
  const onto = `${HEADER}ex:C rdfs:subClassOf ex:B .\nex:B rdfs:subClassOf ex:A .\n`
  const data = '{\n  "@context": { "ex": "http://example.org/" },\n  "@id": "ex:x",\n  "@type": "ex:C"\n}\n'
  fs.writeFileSync(path.join(dir, "onto.ttl"), onto)
  fs.writeFileSync(path.join(dir, "data.jsonld"), data)
  const uri = uriOf(path.join(dir, "data.jsonld"))
  const s = await startServer({ cwd: dir, initializationOptions: { reasoning: { rules: "default" } } })
  try {
    await s.request("workspace/symbol", { query: "" })
    s.open(uri, data)
    const end = Date.now() + 20000
    let md
    for (;;) {
      const h = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(data, '"ex:x"', 4) })
      md = h.contents.value
      if (md.includes("**Inferred**\n\n- ") && !md.includes("still being worked out")) break
      if (Date.now() > end) throw new Error(`no proofs:\n${md}`)
      await new Promise((r) => setTimeout(r, 100))
    }
    const section = md.slice(md.indexOf("\n\n**Inferred**"))
    assert.ok(!section.includes("<http"), section)
    assert.ok(section.includes("`ex:x a ex:C` [data.jsonld:4]"), section)
    // The closest type first.
    assert.ok(section.indexOf("- **ex:B**") < section.indexOf("- **ex:A**"), section)
    assert.ok(section.indexOf("- **ex:B**") >= 0, section)
    // Each base triple is written once, with its link; a later proof
    // refers to it.
    const links = [...section.matchAll(/\]\(([^)]+)\)/g)].map((m) => m[1])
    assert.equal(links.length, 3, section)
    assert.equal(new Set(links).size, 3, section)
    assert.ok(section.includes(" (above)"), section)
  } finally {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

// `ex:x`, `ex:y`, `ex:w` and `ex:v` at the start of a long rdfs:subClassOf
// chain, each with a type disjoint from its end: their proofs take longer
// than the hover's first wait (200 ms), up to the search's time limit (2 s).
// `ex:z`, near its end, has a quick proof. The test needs these proofs to
// be slow: when TurbOWL explains a long chain quickly (turbowl#6), make the
// chain longer or find another slow proof.
function slowChain(links) {
  let text = `${HEADER}ex:z a ex:C${links - 1} .\n`
  for (const t of ["x", "y", "w", "v"]) text += `ex:${t} a ex:C0 , ex:D .\n`
  for (let i = 0; i < links; i++) text += `ex:C${i} rdfs:subClassOf ex:C${i + 1} .\n`
  return text + `ex:C${links} owl:disjointWith ex:D .\n`
}

test("a hover whose proofs take long is answered once they are found, not told to hover again", async () => {
  const dir = tempDir("turtle-reasoning-hover-slow-")
  const text = slowChain(200)
  fs.writeFileSync(path.join(dir, "chain.ttl"), text)
  const uri = uriOf(path.join(dir, "chain.ttl"))
  const s = await startServer({ cwd: dir, initializationOptions: { reasoning: { rules: "default" } } })
  const hoverOn = (needle, token) =>
    s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, needle, 3) }, token)
  try {
    await s.request("workspace/symbol", { query: "" })
    s.open(uri, text)
    await s.diagnosticsFor(uri)
    // Until the run's result is shown: a cheap proof's hover lists it.
    for (const end = Date.now() + 20000; ; ) {
      const md = (await hoverOn("ex:z a")).contents.value
      if (md.includes("**Inferred**\n\n- ")) break
      if (Date.now() > end) throw new Error(`no run shown:\n${md}`)
      await new Promise((r) => setTimeout(r, 100))
    }

    // Cancelled while its proofs are searched for: answered as cancelled.
    const { CancellationTokenSource } = createRequire(import.meta.url)("vscode-jsonrpc/node")
    const source = new CancellationTokenSource()
    const cancelled = hoverOn("ex:y a", source.token)
    setTimeout(() => source.cancel(), 300)
    await assert.rejects(cancelled, (e) => e.code === -32800)

    // One hover, held until its proofs are found or the search stops at
    // its limit: never "hover again". It may wait for ex:y's search first.
    const start = Date.now()
    const md = (await hoverOn("ex:x a")).contents.value
    const took = Date.now() - start
    assert.ok(!md.includes("still being worked out"), md)
    assert.ok(md.includes("**Inferred**\n\n- "), md)
    assert.ok(took > 200, `answered in ${took} ms: the proofs were not slow`)
    assert.ok(took < 8000, `answered in ${took} ms`)

    // A newer hover cancels the one held: Neovim sends no $/cancelRequest
    // when the cursor moves, and the older hover would open late, about
    // the term left behind.
    const older = hoverOn("ex:w a")
    await new Promise((r) => setTimeout(r, 300))
    const newer = hoverOn("ex:v a")
    await assert.rejects(older, (e) => e.code === -32800)
    const latest = (await newer).contents.value
    assert.ok(!latest.includes("still being worked out"), latest)
    assert.ok(latest.includes("**Inferred**\n\n- "), latest)
  } finally {
    await s.stop()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})
