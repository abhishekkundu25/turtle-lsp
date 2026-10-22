// Starts the turtle-lsp binary and connects VS Code to it. All language
// features live in the server; this file only finds and launches it.
import * as fs from "node:fs"
import * as path from "node:path"
import * as vscode from "vscode"
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node"
import { findServer, ServerHandle, settingsFrom } from "./server"

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

/** Where this extension finds the server: bundled in the .vsix, or built in a checkout. */
function findCommand(context: vscode.ExtensionContext): string | undefined {
  const found = findServer({
    configured: vscode.workspace.getConfiguration("turtle").get<string>("server.path"),
    exe,
    candidates: [
      // Bundled in the .vsix by scripts/package.mjs.
      context.asAbsolutePath(path.join("server", exe)),
      // Running from a checkout of the repository.
      context.asAbsolutePath(path.join("..", "..", "target", "release", exe)),
    ],
    pathEnv: process.env.PATH,
    trusted: vscode.workspace.isTrusted,
    workspaceFolders: (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.fsPath),
    fs,
  })
  if ("error" in found) {
    void vscode.window.showErrorMessage(found.error)
    return undefined
  }
  return found.command
}

/**
 * The `turtle.*` settings the server reads, as nested objects: the ones the
 * user set (not the defaults the manifest contributes, which would override
 * `turtle-lsp.json`), with network imports off in an untrusted workspace.
 */
function settingsFor(context: vscode.ExtensionContext, trusted: boolean): Record<string, unknown> {
  const manifest = context.extension.packageJSON
  const config = vscode.workspace.getConfiguration("turtle")
  return settingsFrom({
    properties: manifest.contributes.configuration.properties,
    restricted: manifest.capabilities.untrustedWorkspaces.restrictedConfigurations,
    inspect: (key) => config.inspect(key),
    trusted,
  })
}

function createClient(context: vscode.ExtensionContext): LanguageClient | undefined {
  let created: LanguageClient | undefined
  const command = findCommand(context)
  if (!command) return undefined
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
    initializationOptions: settingsFor(context, vscode.workspace.isTrusted),
    // Later changes reach the server as workspace/didChangeConfiguration.
    synchronize: { configurationSection: "turtle" },
    outputChannelName: "Turtle",
    middleware: {
      workspace: {
        // The client would send the whole section, defaults included, which
        // beats turtle-lsp.json at the server. Send the user's settings instead.
        didChangeConfiguration: async () => {
          await created?.sendNotification("workspace/didChangeConfiguration", {
            settings: settingsFor(context, vscode.workspace.isTrusted),
          })
        },
      },
      window: {
        // Query results open beside the query, which keeps the focus.
        showDocument: async (params, token, next) => {
          if (!/\/query-results\/[^/]+\.md$/.test(params.uri)) return next(params, token)
          await vscode.window.showTextDocument(vscode.Uri.parse(params.uri), {
            viewColumn: vscode.ViewColumn.Beside,
            preserveFocus: true,
            preview: false,
          })
          return { success: true }
        },
      },
    },
  }
  created = new LanguageClient("turtle", "Turtle", serverOptions, clientOptions)
  return created
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
  /** Settles when every start, restart and stop asked so far has finished. */
  idle(): Promise<void>
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

let serverHandle: ServerHandle<LanguageClient> | undefined

export async function activate(context: vscode.ExtensionContext): Promise<TurtleApi> {
  const handle = (serverHandle = new ServerHandle<LanguageClient>(() => createClient(context)))
  let wantedServer = false
  /** Runs a start or restart; a failure is shown, not thrown, so the restart command still works. */
  const run = async (step: () => Promise<unknown>): Promise<void> => {
    try {
      await step()
    } catch (err) {
      void vscode.window.showErrorMessage(`The Turtle language server did not start: ${err instanceof Error ? err.message : String(err)}`)
    }
  }
  const start = (): Promise<void> => {
    wantedServer = true
    return run(() => handle.ensureStarted())
  }
  context.subscriptions.push(
    vscode.commands.registerCommand("turtle.restartServer", () => {
      wantedServer = true
      return run(() => handle.restart())
    }),
    vscode.commands.registerCommand("turtle.showReferences", showReferences),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("turtle.server.path")) {
        await vscode.commands.executeCommand("turtle.restartServer")
      }
    }),
    // Trust decides which binary may run and what the server may fetch.
    vscode.workspace.onDidGrantWorkspaceTrust(async () => {
      if (wantedServer) await vscode.commands.executeCommand("turtle.restartServer")
    }),
  )
  if (await wanted()) {
    await start()
  } else {
    const waiting = vscode.workspace.onDidOpenTextDocument(async (doc) => {
      if (wantedServer || !isServed(doc)) return
      waiting.dispose()
      await start()
    })
    context.subscriptions.push(waiting)
  }
  return { client: () => handle.client(), idle: () => handle.idle() }
}

export async function deactivate(): Promise<void> {
  await serverHandle?.dispose()
}
