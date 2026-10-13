// Runs inside VS Code's extension host. Mirrors test/nvim/spec.lua: drives the
// real editor APIs against the real server. Prints "ok"/"not ok" per check.
const assert = require("node:assert/strict")
const fs = require("node:fs")
const path = require("node:path")
const vscode = require("vscode")

const fixtures = process.env.TURTLE_FIXTURES
const ws = path.join(fixtures, "workspace")
const project = path.join(fixtures, "project")
const datasets = path.join(fixtures, "datasets")
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
  const ext = vscode.extensions.getExtension("abhishekkundu25.turtle-language-server")
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

check("organise prefixes is offered as a source action", async () => {
  const content = "@prefix z: <http://z/> .\n@prefix a: <http://a/> .\n@prefix u: <http://u/> .\na:x z:p a:y .\n"
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content })
  await vscode.window.showTextDocument(doc)
  await settledDiagnostics(doc)
  const range = new vscode.Range(0, 0, 0, 0)
  const actions = await vscode.commands.executeCommand("vscode.executeCodeActionProvider", doc.uri, range, vscode.CodeActionKind.SourceOrganizeImports.value)
  const organise = actions.find((a) => a.title === "Organise prefixes")
  assert.ok(organise, actions.map((a) => a.title).join(", "))
  assert.ok(vscode.CodeActionKind.SourceOrganizeImports.contains(organise.kind))
  assert.ok(await vscode.workspace.applyEdit(organise.edit))
  assert.equal(doc.getText(), "@prefix a: <http://a/> .\n@prefix z: <http://z/> .\na:x z:p a:y .\n")
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("shorten all IRIs is resolved and applied", async () => {
  const content = "@prefix a: <http://a/> .\n<http://a/x> a:p <http://a/y> .\n"
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content })
  await vscode.window.showTextDocument(doc)
  await settledDiagnostics(doc)
  const range = new vscode.Range(1, 0, 1, 0)
  const actions = await vscode.commands.executeCommand("vscode.executeCodeActionProvider", doc.uri, range, vscode.CodeActionKind.RefactorRewrite.value, 10)
  const all = actions.find((a) => a.title === "Shorten all IRIs in file")
  assert.ok(all?.edit, actions.map((a) => a.title).join(", "))
  assert.ok(await vscode.workspace.applyEdit(all.edit))
  assert.equal(doc.getText(), "@prefix a: <http://a/> .\na:x a:p a:y .\n")
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("Format Document lays out the file", async () => {
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content: "@prefix ex: <http://e/> .\nex:a a ex:Thing;ex:knows ex:b .\n" })
  await vscode.window.showTextDocument(doc)
  await settledDiagnostics(doc)
  const edits = await vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", doc.uri, { tabSize: 4, insertSpaces: true })
  assert.ok(edits?.length > 0, "no formatting edits")
  const edit = new vscode.WorkspaceEdit()
  edit.set(doc.uri, edits)
  assert.ok(await vscode.workspace.applyEdit(edit))
  assert.equal(doc.getText(), "@prefix ex: <http://e/> .\n\nex:a a ex:Thing ;\n    ex:knows ex:b .\n")
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("outline lists subjects", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  const syms = await vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", doc.uri)
  const names = syms.map((s) => s.name)
  assert.ok(names.includes("ex:alice") && names.includes("ex:bob"), names.join(", "))
})

check("a .trig file is TriG: no warnings, prefixes from prefixes.ttl", async () => {
  const doc = await open(path.join(datasets, "graphs.trig"))
  assert.equal(doc.languageId, "trig")
  const d = problems(await settledDiagnostics(doc))
  assert.deepEqual(d, [], fmt(d))
})

check("hover in a .trig graph block shows the vocabulary comment", async () => {
  const doc = await open(path.join(datasets, "graphs.trig"))
  const hovers = await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, posOf(doc, "a dx:Pers"))
  const text = hovers.flatMap((h) => h.contents.map((c) => c.value ?? String(c))).join("\n")
  assert.match(text, /Someone the dataset describes/)
})

check("a TriG syntax error is reported", async () => {
  const doc = await vscode.workspace.openTextDocument({ language: "trig", content: "@prefix ex: <http://e/> .\nex:g {\n  ex:a ex:p ex:o .\n" })
  await vscode.window.showTextDocument(doc)
  let d = []
  for (let i = 0; i < 40 && d.length === 0; i++) {
    await sleep(50)
    d = vscode.languages.getDiagnostics(doc.uri).filter((x) => x.severity === vscode.DiagnosticSeverity.Error)
  }
  assert.ok(d.some((x) => /Unclosed '\{'/.test(x.message)), fmt(d))
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("an N-Triples file is N-Triples: @prefix is an error", async () => {
  const nt = await open(path.join(datasets, "people.nt"))
  assert.equal(nt.languageId, "ntriples")
  assert.deepEqual(problems(await settledDiagnostics(nt)), [])
  const doc = await vscode.workspace.openTextDocument({ language: "ntriples", content: "@prefix ex: <http://e/> .\n<http://e/a> <http://e/p> <http://e/o> .\n" })
  await vscode.window.showTextDocument(doc)
  let d = []
  for (let i = 0; i < 40 && d.length === 0; i++) {
    await sleep(50)
    d = vscode.languages.getDiagnostics(doc.uri).filter((x) => x.severity === vscode.DiagnosticSeverity.Error)
  }
  assert.ok(d.some((x) => /N-Triples does not allow directives/.test(x.message)), fmt(d))
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("semantic tokens provider returns tokens (foaf:Person is a class)", async () => {
  const doc = await open(path.join(ws, "people.ttl"))
  await settledDiagnostics(doc)
  const legend = await vscode.commands.executeCommand("vscode.provideDocumentSemanticTokensLegend", doc.uri)
  const tokens = await vscode.commands.executeCommand("vscode.provideDocumentSemanticTokens", doc.uri)
  assert.ok(legend && tokens?.data?.length > 0, "no semantic tokens")
  const at = posOf(doc, "a foaf:", 0)
  let line = 0
  let char = 0
  const found = []
  for (let i = 0; i < tokens.data.length; i += 5) {
    if (tokens.data[i] > 0) char = 0
    line += tokens.data[i]
    char += tokens.data[i + 1]
    if (line === at.line) found.push(legend.tokenTypes[tokens.data[i + 3]])
  }
  assert.ok(found.includes("class") && found.includes("keyword"), found.join(", "))
})

check("inlay hints show the label after an opaque name", async () => {
  const content = '@prefix ex: <http://e/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\nex:P0042 rdfs:label "pump pressure" .\nex:x ex:p ex:P0042 .\n'
  const doc = await vscode.workspace.openTextDocument({ language: "turtle", content })
  await vscode.window.showTextDocument(doc)
  await settledDiagnostics(doc)
  const range = new vscode.Range(0, 0, doc.lineCount, 0)
  const hints = await vscode.commands.executeCommand("vscode.executeInlayHintProvider", doc.uri, range)
  const labels = hints.map((h) => (typeof h.label === "string" ? h.label : h.label.map((p) => p.value).join("")))
  assert.deepEqual(labels, ["pump pressure", "pump pressure"])
  assert.equal(hints[1].position.line, 3)
  await vscode.commands.executeCommand("workbench.action.revertAndCloseActiveEditor")
})

check("saving a file that violates the project shapes shows a SHACL violation", async () => {
  // Created for this check and removed afterwards (the fixtures stay as they are).
  const file = path.join(project, "data", "__vscode-shacl.ttl")
  fs.writeFileSync(file, 'obj:pump-09 a sd:Asset ;\n    sd:designation "=P09" .\n')
  try {
    const doc = await open(file)
    const edit = new vscode.WorkspaceEdit()
    edit.replace(doc.uri, new vscode.Range(posOf(doc, '"=P09"', 1), posOf(doc, '"=P09"', 5)), "P09")
    assert.ok(await vscode.workspace.applyEdit(edit))
    assert.ok(await doc.save())
    const code = (d) => (typeof d.code === "object" ? d.code.value : d.code)
    let d = []
    for (let i = 0; i < 100 && d.length === 0; i++) {
      await sleep(50)
      d = vscode.languages.getDiagnostics(doc.uri).filter((x) => code(x) === "shacl-violation")
    }
    assert.equal(d.length, 1, fmt(vscode.languages.getDiagnostics(doc.uri)))
    assert.match(d[0].message, /designation/)
    assert.equal(d[0].range.start.line, 1)
    await vscode.commands.executeCommand("workbench.action.closeActiveEditor")
  } finally {
    fs.rmSync(file, { force: true })
  }
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
