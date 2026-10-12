// Runs inside VS Code's extension host. Mirrors test/nvim/spec.lua: drives the
// real editor APIs against the real server. Prints "ok"/"not ok" per check.
const assert = require("node:assert/strict")
const fs = require("node:fs")
const path = require("node:path")
const vscode = require("vscode")

const fixtures = process.env.TURTLE_FIXTURES
const ws = path.join(fixtures, "workspace")
const project = path.join(fixtures, "project")
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function open(file) {
  const doc = await vscode.workspace.openTextDocument(vscode.Uri.file(file))
  await vscode.window.showTextDocument(doc)
  return doc
}

function posOf(doc, needle, offsetInNeedle = needle.length) {
  const i = doc.getText().indexOf(needle)
  assert.ok(i >= 0, `needle not found: ${needle}`)
  return doc.positionAt(i + offsetInNeedle)
}

// The server publishes diagnostics before it answers any request, so a
// round trip guarantees diagnostics for the current text have been sent.
async function settledDiagnostics(doc) {
  await vscode.commands.executeCommand("vscode.executeDocumentHighlights", doc.uri, new vscode.Position(0, 0))
  await sleep(100)
  return vscode.languages.getDiagnostics(doc.uri)
}

const problems = (diags) => diags.filter((d) => d.severity <= vscode.DiagnosticSeverity.Warning)
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.message}`).join("; ")

const checks = []
const check = (name, fn) => checks.push({ name, fn })

check("extension activates and the server starts", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  assert.equal(doc.languageId, "turtle")
  const ext = vscode.extensions.getExtension("abhishekkundu25.turtle-lsp")
  assert.ok(ext, "extension not found")
  const api = await ext.activate()
  for (let i = 0; i < 100 && !api.client()?.isRunning(); i++) await sleep(50)
  assert.ok(api.client()?.isRunning(), "language client is not running")
})

check("valid file shows no warnings", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  const d = problems(await settledDiagnostics(doc))
  assert.deepEqual(d, [], fmt(d))
})

check("syntax error is reported", async () => {
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content: '@prefix ex: <http://e/> .\nex:a ex:p "open .\n' })
  await vscode.window.showTextDocument(doc)
  let d = []
  for (let i = 0; i < 40 && d.length === 0; i++) {
    await sleep(50)
    d = vscode.languages.getDiagnostics(doc.uri).filter((x) => x.severity === vscode.DiagnosticSeverity.Error)
  }
  assert.ok(d.some((x) => /Unterminated string/.test(x.message)), fmt(d))
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("hover on foaf:Person shows documentation", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  const hovers = await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, posOf(doc, "a foaf:Pers"))
  const text = hovers.flatMap((h) => h.contents.map((c) => c.value ?? String(c))).join("\n")
  assert.match(text, /person/i)
})

check("completion after 'foaf:' offers foaf:Person", async () => {
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content: "@prefix foaf: <http://xmlns.com/foaf/0.1/> .\n@prefix ex: <http://e/> .\nex:a a foaf:\n" })
  await vscode.window.showTextDocument(doc)
  const list = await vscode.commands.executeCommand("vscode.executeCompletionItemProvider", doc.uri, new vscode.Position(2, 12))
  const labels = list.items.map((i) => (typeof i.label === "string" ? i.label : i.label.label))
  assert.ok(labels.includes("foaf:Person"), labels.slice(0, 15).join(", "))
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("go to definition crosses files", async () => {
  const doc = await open(path.join(ws, "org.ttl"))
  const locs = await vscode.commands.executeCommand("vscode.executeDefinitionProvider", doc.uri, posOf(doc, "ex:alic"))
  assert.equal(locs.length, 1, JSON.stringify(locs))
  const loc = locs[0]
  const uri = loc.uri ?? loc.targetUri
  const range = loc.range ?? loc.targetSelectionRange
  assert.equal(path.basename(uri.fsPath), "people.ttl")
  assert.equal(range.start.line, 4)
})

check("rename ex:bob applies cleanly", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  const edit = await vscode.commands.executeCommand("vscode.executeDocumentRenameProvider", doc.uri, posOf(doc, "ex:bo"), "ex:robert")
  assert.ok(await vscode.workspace.applyEdit(edit), "applyEdit failed")
  const text = doc.getText()
  assert.equal((text.match(/ex:robert/g) ?? []).length, 2, text)
  assert.ok(!text.includes("ex:bob"))
  await vscode.commands.executeCommand("workbench.action.files.revert")
})

check("headerless fragment gets prefixes from prefixes.ttl", async () => {
  const doc = await open(path.join(project, "data", "instances.ttl"))
  const d = problems(await settledDiagnostics(doc))
  assert.deepEqual(d, [], fmt(d))
})

check("quick fix adds a missing well-known prefix", async () => {
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content: "@prefix ex: <http://e/> .\nex:a a skos:Concept .\n" })
  await vscode.window.showTextDocument(doc)
  let diag
  for (let i = 0; i < 40 && !diag; i++) {
    await sleep(50)
    diag = vscode.languages.getDiagnostics(doc.uri).find((d) => /skos/.test(d.message))
  }
  assert.ok(diag, "no undeclared-prefix diagnostic")
  const actions = await vscode.commands.executeCommand("vscode.executeCodeActionProvider", doc.uri, diag.range)
  const fix = actions.find((a) => /Add @prefix skos:/.test(a.title))
  assert.ok(fix, actions.map((a) => a.title).join(", "))
  assert.ok(await vscode.workspace.applyEdit(fix.edit))
  assert.match(doc.getText(), /@prefix skos: <http:\/\/www\.w3\.org\/2004\/02\/skos\/core#> \./)
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("outline lists subjects", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  const syms = await vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", doc.uri)
  const names = syms.map((s) => s.name)
  assert.ok(names.includes("ex:alice") && names.includes("ex:bob"), names.join(", "))
})

exports.run = async function run() {
  let failed = 0
  for (const { name, fn } of checks) {
    try {
      await fn()
      console.log(`ok - ${name}`)
    } catch (err) {
      failed++
      console.log(`not ok - ${name}: ${String(err?.message ?? err).split("\n").join(" | ")}`)
    }
  }
  console.log(`${checks.length - failed}/${checks.length} VS Code checks passed`)
  if (failed) throw new Error(`${failed} VS Code check(s) failed`)
}
