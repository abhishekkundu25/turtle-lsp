// Starts the turtle-lsp binary and connects VS Code to it. All language
// features live in the server; this file only finds and launches it.
import * as fs from "node:fs"
import * as path from "node:path"
import * as vscode from "vscode"
import { LanguageClient, LanguageClientOptions, ServerOptions } from "vscode-languageclient/node"

let client: LanguageClient | undefined

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
    imports: { network: c.get<boolean>("imports.network") },
    format: { lineWidth: c.get<number>("format.lineWidth") },
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
    ],
    initializationOptions: serverSettings(),
    // Later changes reach the server as workspace/didChangeConfiguration.
    synchronize: { configurationSection: "turtle" },
    outputChannelName: "Turtle",
  }
  client = new LanguageClient("turtle", "Turtle", serverOptions, clientOptions)
  await client.start()
}

/** Exposed for the integration tests. */
export interface TurtleApi {
  client(): LanguageClient | undefined
}

export async function activate(context: vscode.ExtensionContext): Promise<TurtleApi> {
  context.subscriptions.push(
    vscode.commands.registerCommand("turtle.restartServer", async () => {
      await client?.stop()
      await start(context)
    }),
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("turtle.server.path")) {
        await vscode.commands.executeCommand("turtle.restartServer")
      }
    }),
  )
  await start(context)
  return { client: () => client }
}

export async function deactivate(): Promise<void> {
  await client?.stop()
}
