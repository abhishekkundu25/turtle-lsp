// Runs inside VS Code for the Web's extension host (a browser worker), with
// test/fixtures/web mounted as the workspace; run.mjs bundles it. Drives the
// browser entry through the editor's API and prints "ok"/"not ok" per check.
import * as vscode from "vscode"

const EXTENSION = "abhishekkundu25.turtle-language-server"
/** Spec budget: the first diagnostics within 10 s of opening the first file. */
const FIRST_DIAGNOSTICS_MS = 10_000

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const root = () => vscode.workspace.workspaceFolders[0].uri
const uri = (name) => vscode.Uri.joinPath(root(), name)
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} [${d.code?.value ?? d.code}] ${d.message}`).join("; ")
const codeOf = (d) => String(d.code?.value ?? d.code)

function ok(value, message) {
  if (!value) throw new Error(message)
}

async function open(name) {
  const doc = await vscode.workspace.openTextDocument(uri(name))
  await vscode.window.showTextDocument(doc)
  return doc
}

function posOf(doc, needle, offset = 1) {
  const i = doc.getText().indexOf(needle)
  ok(i >= 0, `${needle} not in ${doc.uri.path}`)
  return doc.positionAt(i + offset)
}

/** Polls `get` every 100 ms until `done(value)`, or fails after `ms`. */
async function until(what, get, done, ms) {
  const end = Date.now() + ms
  for (;;) {
    const value = await get()
    if (done(value)) return value
    if (Date.now() > end) throw new Error(`${what} not within ${ms} ms (last: ${JSON.stringify(value)?.slice(0, 300)})`)
    await sleep(100)
  }
}

const hoverText = (hovers) =>
  (hovers ?? []).flatMap((h) => h.contents).map((c) => (typeof c === "string" ? c : c.value)).join("\n")
const locations = (result) => (result ?? []).map((l) => (l.targetUri ?? l.uri).path)

const checks = []
const check = (name, fn) => checks.push({ name, fn })
let api
const loaded = Date.now()

check("activates in the browser with its server running", async () => {
  const ext = vscode.extensions.getExtension(EXTENSION)
  ok(ext, `${EXTENSION} is not installed in the web extension host`)
  api = await ext.activate()
  ok(api && typeof api.client === "function", "activate() did not return the TurtleApi")
  await until("a running client", async () => (await api.idle(), api.client()?.isRunning() ?? false), Boolean, 10_000)
})

check(`diagnostics on an open headerless file within ${FIRST_DIAGNOSTICS_MS / 1000} s, prefixes.ttl honoured`, async () => {
  const start = Date.now()
  const doc = await open("a.ttl")
  // The first diagnostics may come before prefixes.ttl has been read (the
  // extension sends the workspace's files after the client starts); a.ttl
  // is checked again when it arrives. Wait for that settled state: the
  // expected hint, and no undeclared prefix.
  const settled = (d) => d.some((x) => codeOf(x) === "undescribed-term") && !d.some((x) => codeOf(x) === "undeclared-prefix")
  let diags
  try {
    diags = await until("a.ttl's diagnostics with prefixes.ttl read", () => vscode.languages.getDiagnostics(doc.uri), settled, FIRST_DIAGNOSTICS_MS)
  } catch (e) {
    const undeclared = vscode.languages.getDiagnostics(doc.uri).filter((d) => codeOf(d) === "undeclared-prefix")
    ok(undeclared.length === 0, `prefixes.ttl not honoured within ${FIRST_DIAGNOSTICS_MS} ms: ${fmt(undeclared)}`)
    throw e
  }
  console.log(`# diagnostics with prefixes.ttl ${Date.now() - start} ms after opening a.ttl, ${Date.now() - loaded} ms after the suite started (${diags.length})`)
})

check("hover on ex:Thing shows its type and comment from b.ttl", async () => {
  const doc = await open("a.ttl")
  const hovers = await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, posOf(doc, "ex:Thing", 4))
  const text = hoverText(hovers)
  ok(text.includes("owl:Class") && text.includes("Anything at all."), `hover: ${JSON.stringify(text)}`)
  // Its "Described in" link is the workspace's URI, not the server's.
  ok(!text.includes("file:///workspace/") && text.includes(`${vscode.workspace.workspaceFolders[0].uri.toString().replace(/\/+$/, "")}/b.ttl`), `hover link: ${JSON.stringify(text)}`)
})

check("definition of ex:Thing lands in b.ttl", async () => {
  const doc = await open("a.ttl")
  const result = await vscode.commands.executeCommand("vscode.executeDefinitionProvider", doc.uri, posOf(doc, "ex:Thing", 4))
  ok(locations(result).some((p) => p.endsWith("/b.ttl")), `definition: ${JSON.stringify(locations(result))}`)
})

check("a file never opened has its problem listed (bad.ttl)", async () => {
  ok(!vscode.workspace.textDocuments.some((d) => d.uri.path.endsWith("/bad.ttl")), "bad.ttl was opened")
  const diags = await until("bad.ttl's syntax error", () => vscode.languages.getDiagnostics(uri("bad.ttl")), (d) => d.some((x) => codeOf(x) === "syntax"), 10_000)
  ok(diags.length > 0, "no diagnostic")
})

check("references to ex:Thing still include b.ttl after b.ttl is closed", async () => {
  await open("b.ttl")
  await vscode.commands.executeCommand("workbench.action.closeActiveEditor")
  ok(!vscode.window.visibleTextEditors.some((e) => e.document.uri.path.endsWith("/b.ttl")), "b.ttl is still shown")
  const doc = await open("a.ttl")
  const result = await vscode.commands.executeCommand("vscode.executeReferenceProvider", doc.uri, posOf(doc, "ex:Thing", 4))
  const paths = locations(result)
  ok(paths.some((p) => p.endsWith("/b.ttl")), `references: ${JSON.stringify(paths)}`)
})

check("formatting returns edits for an unformatted file", async () => {
  const doc = await open("unformatted.ttl")
  const edits = await vscode.commands.executeCommand("vscode.executeFormatDocumentProvider", doc.uri, { tabSize: 2, insertSpaces: true })
  ok(edits?.length > 0, `no edits for ${JSON.stringify(doc.getText())}`)
  await vscode.commands.executeCommand("workbench.action.closeActiveEditor")
})

check("a .owl file keeps VS Code's XML language and gets the server's hover and rename, but no formatter, completion or code actions of the extension", async () => {
  const doc = await open("onto.owl")
  ok(doc.languageId === "xml", `language ${doc.languageId}`)
  const at = posOf(doc, 'about="http://example.org/Widg', 10)
  const text = await until("hover in onto.owl", async () => hoverText(await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, at)), (t) => t.includes("A thing with parts."), 10_000)
  ok(text.includes("A thing with parts."), `hover: ${JSON.stringify(text)}`)
  // The server registers these for the documents it writes: a Turtle file has them, the .owl does
  // not, but for rename, which edits RDF/XML exactly or not at all.
  const ttl = await open("unformatted.ttl")
  for (const method of ["textDocument/formatting", "textDocument/rename", "textDocument/completion", "textDocument/codeAction"]) {
    const feature = api.client().getFeature(method)
    ok(feature.getProvider(ttl) !== undefined, `${method}: no provider for unformatted.ttl`)
    if (method === "textDocument/rename") ok(feature.getProvider(doc) !== undefined, `${method}: no provider for onto.owl`)
    else ok(feature.getProvider(doc) === undefined, `${method}: a provider for onto.owl`)
  }
  await vscode.commands.executeCommand("workbench.action.closeAllEditors")
})

check("a .ofn file is OWL Functional syntax: its errors are listed before and after it is opened, its terms hover, lead to their declaration and are outlined, and the extension offers no formatter, rename, completion or code actions there", async () => {
  // The browser reads the workspace's files: listed before it is opened.
  ok(!vscode.workspace.textDocuments.some((d) => d.uri.path.endsWith("/onto.ofn")), "onto.ofn was opened")
  const errors = (d) => d.some((x) => codeOf(x) === "ofn-syntax")
  await until("onto.ofn's error, unopened", () => vscode.languages.getDiagnostics(uri("onto.ofn")), errors, 10_000)
  const doc = await open("onto.ofn")
  ok(doc.languageId === "ofn", `language ${doc.languageId}`)
  const diags = await until("onto.ofn's error", () => vscode.languages.getDiagnostics(doc.uri), errors, 10_000)
  ok(fmt(diags) === "3:14 [ofn-syntax] Undeclared prefix 'nope:'", fmt(diags))
  // Its axioms are read: the label in hover, the declaration as the
  // definition, the class in the outline.
  const at = posOf(doc, ":A))", 1)
  const text = await until("hover in onto.ofn", async () => hoverText(await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, at)), (t) => t.includes("Apple"), 10_000)
  ok(text.includes("Apple"), `hover: ${JSON.stringify(text)}`)
  const defs = locations(await vscode.commands.executeCommand("vscode.executeDefinitionProvider", doc.uri, posOf(doc, ":A \"Apple", 1)))
  ok(defs.length === 1 && defs[0].endsWith("/onto.ofn"), `definition: ${JSON.stringify(defs)}`)
  const symbols = await vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", doc.uri)
  ok(JSON.stringify(symbols.map((x) => x.name)) === JSON.stringify(["prefixes", "http://example.org/o", ":A"]), `symbols: ${JSON.stringify(symbols.map((x) => x.name))}`)
  for (const method of ["textDocument/formatting", "textDocument/rename", "textDocument/completion", "textDocument/codeAction"]) {
    const feature = api.client().getFeature(method)
    ok(feature.getProvider(doc) === undefined, `${method}: a provider for onto.ofn`)
  }
  await vscode.commands.executeCommand("workbench.action.closeAllEditors")
})

check("a .owx file is OWL/XML: its errors are listed before and after it is opened, its terms hover, lead to their declaration and are outlined, and nothing writes it", async () => {
  ok(!vscode.workspace.textDocuments.some((d) => d.uri.path.endsWith("/onto.owx")), "onto.owx was opened")
  const errors = (d) => d.some((x) => codeOf(x) === "owx-syntax")
  await until("onto.owx's error, unopened", () => vscode.languages.getDiagnostics(uri("onto.owx")), errors, 10_000)
  const doc = await open("onto.owx")
  // VS Code's XML language, through the extension's files.associations
  // default: the fixture has no XML declaration, which alone would make it XML.
  ok(doc.languageId === "xml", `language ${doc.languageId}`)
  const diags = await until("onto.owx's error", () => vscode.languages.getDiagnostics(doc.uri), errors, 10_000)
  ok(
    fmt(diags) === `9:27 [owx-syntax] Undeclared prefix 'nope:': an OWL/XML document declares each prefix it uses, as <Prefix name="nope" IRI="…"/>`,
    fmt(diags),
  )
  const at = posOf(doc, ':P"/>\n  </Declaration>', 1)
  const text = await until("hover in onto.owx", async () => hoverText(await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, at)), (t) => t.includes("Pear"), 10_000)
  ok(text.includes("Pear"), `hover: ${JSON.stringify(text)}`)
  const defs = locations(await vscode.commands.executeCommand("vscode.executeDefinitionProvider", doc.uri, posOf(doc, ":P</AbbreviatedIRI>", 1)))
  ok(defs.length === 1 && defs[0].endsWith("/onto.owx"), `definition: ${JSON.stringify(defs)}`)
  const symbols = await vscode.commands.executeCommand("vscode.executeDocumentSymbolProvider", doc.uri)
  ok(JSON.stringify(symbols.map((x) => x.name)) === JSON.stringify(["http://example.org/x", "prefixes", ":P"]), `symbols: ${JSON.stringify(symbols.map((x) => x.name))}`)
  for (const method of ["textDocument/formatting", "textDocument/rename", "textDocument/completion", "textDocument/codeAction"]) {
    const feature = api.client().getFeature(method)
    ok(feature.getProvider(doc) === undefined, `${method}: a provider for onto.owx`)
  }
  await vscode.commands.executeCommand("workbench.action.closeAllEditors")
})

check("a user's own files.associations applies beside the extension's default for .owx, and wins over it", async () => {
  const files = () => vscode.workspace.getConfiguration("files")
  const languageOf = (name) => vscode.workspace.textDocuments.find((d) => d.uri.toString() === uri(name).toString())?.languageId
  ok(files().inspect("associations").defaultValue?.["*.owx"] === "xml", `default: ${JSON.stringify(files().inspect("associations").defaultValue)}`)
  await open("onto.owx")
  await open("q.rq")
  try {
    // Another entry of the user's: both apply.
    await files().update("associations", { "*.rq": "plaintext" }, vscode.ConfigurationTarget.Global)
    const both = files().get("associations")
    ok(both["*.owx"] === "xml" && both["*.rq"] === "plaintext", `associations: ${JSON.stringify(both)}`)
    await until("q.rq as plain text", () => languageOf("q.rq"), (l) => l === "plaintext", 10_000)
    ok(languageOf("onto.owx") === "xml", `onto.owx: ${languageOf("onto.owx")}`)
    // The user's entry for .owx wins over the default.
    await files().update("associations", { "*.owx": "plaintext" }, vscode.ConfigurationTarget.Global)
    await until("onto.owx as plain text", () => languageOf("onto.owx"), (l) => l === "plaintext", 10_000)
  } finally {
    await files().update("associations", undefined, vscode.ConfigurationTarget.Global)
  }
  await until("onto.owx as XML again", () => languageOf("onto.owx"), (l) => l === "xml", 10_000)
  await until("q.rq as SPARQL again", () => languageOf("q.rq"), (l) => l === "sparql", 10_000)
  await vscode.commands.executeCommand("workbench.action.closeAllEditors")
})

check("turtle/query, as another extension sends it, uses the root's prefixes.ttl", async () => {
  const api = vscode.extensions.getExtension(EXTENSION).exports
  const r = await api.client().sendRequest("turtle/query", { query: "SELECT ?c WHERE { ?c a owl:Class }" })
  ok(r.kind === "select" && r.results.bindings.some((b) => b.c.value === "http://example.org/Thing"), `turtle/query: ${JSON.stringify(r)}`)
})

check("a SPARQL file has no Run query lens", async () => {
  const doc = await open("q.rq")
  // A hover answered shows the server has q.rq, so an empty lens list is the
  // filter at work rather than a server not yet ready.
  const hover = hoverText(await vscode.commands.executeCommand("vscode.executeHoverProvider", doc.uri, posOf(doc, "ex:Thing", 4)))
  ok(/Thing/.test(hover), `hover in q.rq: ${JSON.stringify(hover)}`)
  const lenses = (await vscode.commands.executeCommand("vscode.executeCodeLensProvider", doc.uri, 100)) ?? []
  const commands = lenses.map((l) => l.command?.command)
  ok(!commands.some((c) => c === "turtle.runQuery" || c === "turtle.cancelQuery"), `lenses: ${JSON.stringify(commands)}`)
})

export async function run() {
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
  console.log(`${checks.length - failed}/${checks.length} web checks passed`)
  if (failed) throw new Error(`${failed} web check(s) failed`)
}
