// The desktop entry: finds the turtle-lsp binary and runs it over stdio. All
// language features live in the server; what the extension does with the
// client is in src/activate.ts, shared with the browser entry.
import * as fs from "node:fs"
import * as path from "node:path"
import * as vscode from "vscode"
import { LanguageClient } from "vscode-languageclient/node"
import * as shared from "./activate"
import type { TurtleApi } from "./activate"
import { documentSelector } from "./client"
import { findServer } from "./server"

export type { TurtleApi }

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

export function activate(context: vscode.ExtensionContext): Promise<TurtleApi> {
  context.subscriptions.push(
    vscode.workspace.onDidChangeConfiguration(async (e) => {
      if (e.affectsConfiguration("turtle.server.path")) {
        await vscode.commands.executeCommand("turtle.restartServer")
      }
    }),
  )
  return shared.activate(context, {
    documentSelector: documentSelector(["file", "untitled"]),
    createClient: (options) => {
      const command = findCommand(context)
      return command ? new LanguageClient("turtle", "Turtle", { command, args: [] }, options) : undefined
    },
  })
}

export const deactivate = shared.deactivate
