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
  const expected = ["undescribed-term", "undeclared-prefix"]
  const diags = await until("a.ttl's diagnostics", () => vscode.languages.getDiagnostics(doc.uri), (d) => d.some((x) => expected.includes(codeOf(x))), FIRST_DIAGNOSTICS_MS)
  console.log(`# first diagnostics ${Date.now() - start} ms after opening a.ttl, ${Date.now() - loaded} ms after the suite started`)
  const undeclared = diags.filter((d) => codeOf(d) === "undeclared-prefix")
  ok(undeclared.length === 0, `prefixes.ttl not honoured: ${fmt(undeclared)}`)
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
