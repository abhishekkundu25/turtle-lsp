// The workspace knowledge index: project-aware add-prefix fixes, hover that
// follows edits in other open files, and structure from well-known vocabularies.
//
// Workspace (a temp dir): a copy of test/fixtures/project (prefixes.ttl binds
// sd: to a project namespace) next to outside/, which no prefixes.ttl covers.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { startServer, tempDir, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

const PROJECT_SD = "https://ontology.example.com/plant/core/"
const W3C_SD = "http://www.w3.org/ns/sparql-service-description#"

let ws, s
before(async () => {
  ws = tempDir("turtle-knowledge-")
  fs.cpSync(path.join(fixtures, "project"), path.join(ws, "project"), { recursive: true })
  fs.mkdirSync(path.join(ws, "outside"))
  s = await startServer({ cwd: ws })
  // Requests wait for the initial index; after this, diagnostics see it.
  await s.request("workspace/symbol", { query: "" })
})
after(async () => {
  await s.stop()
  fs.rmSync(ws, { recursive: true, force: true })
})

async function codeActions(uri, diags) {
  return s.request("textDocument/codeAction", {
    textDocument: { uri },
    range: diags[0].range,
    context: { diagnostics: diags },
  })
}

test("undeclared sd: outside the prefixes.ttl scope prefers the project's binding, then W3C", async () => {
  const uri = uriOf(path.join(ws, "outside", "uses-sd.ttl"))
  const text = "<urn:x:a> a sd:Asset .\n"
  s.open(uri, text)
  const diags = (await s.diagnosticsFor(uri)).filter((d) => d.code === "undeclared-prefix")
  assert.equal(diags.length, 1, JSON.stringify(diags))
  assert.ok(diags[0].message.includes(PROJECT_SD), diags[0].message)
  const fixes = (await codeActions(uri, diags)).filter((a) => a.title.startsWith("Add @prefix sd:"))
  assert.equal(fixes.length, 2, fixes.map((f) => f.title).join("\n"))
  assert.match(fixes[0].title, new RegExp(`<${PROJECT_SD}> \\(used in \\d+ files\\)`))
  assert.equal(fixes[0].isPreferred, true)
  assert.equal(fixes[1].title, `Add @prefix sd: <${W3C_SD}>`)
  assert.equal(fixes[1].isPreferred, false)
  const edit = Object.values(fixes[0].edit.changes)[0][0]
  assert.equal(edit.newText, `@prefix sd: <${PROJECT_SD}> .\n`)
})

test("'add all missing prefixes' uses the first candidate", async () => {
  const uri = uriOf(path.join(ws, "outside", "uses-two.ttl"))
  const text = "<urn:x:a> a sd:Asset ; foaf:name \"a\" .\n"
  s.open(uri, text)
  const diags = (await s.diagnosticsFor(uri)).filter((d) => d.code === "undeclared-prefix")
  assert.equal(diags.length, 2, JSON.stringify(diags))
  const all = (await codeActions(uri, diags)).find((a) => a.title.startsWith("Add all missing prefixes"))
  assert.ok(all, "no 'add all' action")
  const added = Object.values(all.edit.changes)[0][0].newText
  assert.ok(added.includes(`@prefix sd: <${PROJECT_SD}> .`), added)
  assert.ok(added.includes("@prefix foaf: <http://xmlns.com/foaf/0.1/> ."), added)
})

test("hover on a term described in another open file follows edits to that file", async () => {
  const defUri = uriOf(path.join(ws, "outside", "described.ttl"))
  const useUri = uriOf(path.join(ws, "outside", "uses.ttl"))
  const header = '@prefix ex: <http://example.org/k/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n'
  const def = (label) => `${header}\nex:Widget a rdfs:Class ;\n    rdfs:label "${label}" .\n`
  const use = `${header}ex:w1 a ex:Widget .\n`
  s.open(defUri, def("Old widget"))
  s.open(useUri, use)
  await s.diagnosticsFor(useUri)
  const hover = async () =>
    (await s.request("textDocument/hover", { textDocument: { uri: useUri }, position: positionOf(use, "a ex:Wid") })).contents.value
  let md = await hover()
  assert.ok(md.includes("Old widget"), md)
  assert.ok(md.includes("described.ttl:4"), md)

  // Move the description down a line and relabel it.
  s.change(defUri, def("New widget").replace("\nex:Widget", "\n\nex:Widget"))
  md = await hover()
  assert.ok(md.includes("New widget") && !md.includes("Old widget"), md)
  assert.ok(md.includes("described.ttl:5"), md)

  // Closing a file that does not exist on disk drops what it described.
  s.close(defUri)
  md = await hover()
  assert.ok(!md.includes("New widget") && !md.includes("described.ttl"), md)
})

test("hover on foaf:knows shows its domain and range from the bundled vocabulary", async () => {
  const uri = uriOf(path.join(ws, "outside", "knows.ttl"))
  const text = "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n<urn:x:a> foaf:knows <urn:x:b> .\n"
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, "foaf:kn") })
  const md = h.contents.value
  assert.ok(md.includes("Domain: foaf:Person · Range: foaf:Person"), md)
})

test("hover on a project property shows workspace domain and range", async () => {
  const uri = uriOf(path.join(ws, "project", "data", "__knowledge.ttl"))
  const text = "obj:x sd:locatedIn obj:y .\n"
  s.open(uri, text)
  await s.diagnosticsFor(uri)
  const h = await s.request("textDocument/hover", { textDocument: { uri }, position: positionOf(text, "sd:loc") })
  const md = h.contents.value
  assert.ok(md.includes("Domain: sd:PhysicalObject · Range: sd:Position"), md)
})
