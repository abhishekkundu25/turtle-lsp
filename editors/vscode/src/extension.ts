// Starts the turtle-lsp binary and connects VS Code to it. All language
// features live in the server; this file only finds and launches it.
import * as fs from "node:fs"
import * as path from "node:path"
import * as vscode from "vscode"
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node"

let client: LanguageClient | undefined

const RDF_LANGUAGES = ["turtle", "trig", "ntriples", "nquads", "sparql"]

// JSON-LD files keep VS Code's own JSON language (and its highlighting): the
// server attaches to them by name, and detects JSON-LD from the extension.
const JSONLD_PATTERN = "**/*.jsonld"

/** Whether the server handles `doc`. */
function isServed(doc: vscode.TextDocument): boolean {
  if (doc.uri.scheme !== "file" && doc.uri.scheme !== "untitled") return false
  return RDF_LANGUAGES.includes(doc.languageId) || (doc.uri.scheme === "file" && doc.uri.path.endsWith(".jsonld"))
}

const exe = process.platform === "win32" ? "turtle-lsp.exe" : "turtle-lsp"

function findServer(context: vscode.ExtensionContext): string | undefined {
  const configured = vscode.workspace.getConfiguration("turtle").get<string>("server.path")
  if (configured) return configured
  const candidates = [
    // Bundled in the .vsix by scripts/package.mjs.
    context.asAbsolutePath(path.join("server", exe)),
    // Running from a checkout of the repository.
    context.asAbsolutePath(path.join("..", "..", "target", "release", exe)),
  ]
  const found = candidates.find((p) => fs.existsSync(p))
  if (found) return found
  const onPath = (process.env.PATH ?? "").split(path.delimiter).map((d) => path.join(d, exe))
  return onPath.find((p) => fs.existsSync(p))
}

/** The `turtle.*` settings the server reads, as nested objects. */
function serverSettings(): Record<string, unknown> {
  const c = vscode.workspace.getConfiguration("turtle")
  return {
    prefixes: c.get<Record<string, string>>("prefixes") ?? {},
    inlayHints: { labels: c.get<string>("inlayHints.labels") },
    shacl: { enabled: c.get<boolean>("shacl.enabled"), maxTriples: c.get<number>("shacl.maxTriples") },
    lint: { undescribedTerms: c.get<boolean>("lint.undescribedTerms") },
    // A restricted setting: in an untrusted workspace VS Code returns only the user's value.
    imports: { network: c.get<boolean>("imports.network"), unresolved: c.get<string>("imports.unresolved") },
    format: { lineWidth: c.get<number>("format.lineWidth") },
    codeLens: { references: c.get<boolean>("codeLens.references") },
    diagnostics: { workspace: c.get<boolean>("diagnostics.workspace") },
  }
}

async function start(context: vscode.ExtensionContext): Promise<void> {
  const command = findServer(context)
  if (!command) {
    void vscode.window.showErrorMessage(
      "turtle-lsp binary not found. Set `turtle.server.path`, or build it with `cargo build --release`.",
    )
    return
  }
  const serverOptions: ServerOptions = { command, args: [] }
  const clientOptions: LanguageClientOptions = {
    documentSelector: [
      { scheme: "file", language: "turtle" },
      { scheme: "untitled", language: "turtle" },
      { scheme: "file", language: "trig" },
      { scheme: "untitled", language: "trig" },
      { scheme: "file", language: "ntriples" },
      { scheme: "untitled", language: "ntriples" },
      { scheme: "file", language: "nquads" },
      { scheme: "untitled", language: "nquads" },
      { scheme: "file", language: "sparql" },
      { scheme: "untitled", language: "sparql" },
      { scheme: "file", pattern: JSONLD_PATTERN },
    ],
    initializationOptions: serverSettings(),
    // Later changes reach the server as workspace/didChangeConfiguration.
    synchronize: { configurationSection: "turtle" },
    outputChannelName: "Turtle",
  }
  client = new LanguageClient("turtle", "Turtle", serverOptions, clientOptions)
  await client.start()
}

interface LspPosition {
  line: number
  character: number
}
interface LspLocation {
  uri: string
  range: { start: LspPosition; end: LspPosition }
}

const asPosition = (p: LspPosition) => new vscode.Position(p.line, p.character)

/**
 * A reference lens's command (`turtle.showReferences`, from the server, with
 * the document URI, the lens position and the reference locations): opens
 * the references view there. The client runs it itself: a lens command the
 * server does not offer as an `executeCommand` reaches VS Code's command
 * registry as is, with the protocol's JSON as arguments.
 */
async function showReferences(uri: string, position: LspPosition, locations: LspLocation[]): Promise<void> {
  await vscode.commands.executeCommand(
    "editor.action.showReferences",
    vscode.Uri.parse(uri),
    asPosition(position),
    locations.map((l) => new vscode.Location(vscode.Uri.parse(l.uri), new vscode.Range(asPosition(l.range.start), asPosition(l.range.end)))),
  )
}

/** Exposed for the integration tests. */
export interface TurtleApi {
  client(): LanguageClient | undefined
}

/**
 * Whether to start the server now. Opening any JSON file activates the
 * extension (`onLanguage:json`, for `.jsonld` files): start only once the
 * workspace has a file the server handles, or one is open.
 */
async function wanted(): Promise<boolean> {
  if (vscode.workspace.textDocuments.some(isServed)) return true
  const files = await vscode.workspace.findFiles("**/*.{ttl,turtle,trig,nt,nq,rq,sparql,ru,jsonld}", "**/node_modules/**", 1)
  return files.length > 0
}

export async function activate(context: vscode.ExtensionContext): Promise<TurtleApi> {
  context.subscriptions.push(
    vscode.commands.registerCommand("turtle.restartServer", async () => {
      await client?.stop()
      await start(context)
    }),
    vscode.commands.registerCommand("turtle.showReferences", showReferences),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("turtle.server.path")) {
        await vscode.commands.executeCommand("turtle.restartServer")
      }
    }),
  )
  if (await wanted()) {
    await start(context)
  } else {
    const waiting = vscode.workspace.onDidOpenTextDocument(async (doc) => {
      if (client || !isServed(doc)) return
      waiting.dispose()
      await start(context)
    })
    context.subscriptions.push(waiting)
  }
  return { client: () => client }
}

export async function deactivate(): Promise<void> {
  await client?.stop()
}
