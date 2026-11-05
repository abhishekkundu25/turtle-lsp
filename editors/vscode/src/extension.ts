// The desktop entry: finds the turtle-lsp binary and runs it over stdio. All
// language features live in the server; what the extension does with the
// client is in src/activate.ts, shared with the browser entry. Folders on a
// virtual file system are not on disk for the binary (`planWorkspace`).
import * as fs from "node:fs"
import * as path from "node:path"
import * as vscode from "vscode"
import { LanguageClient } from "vscode-languageclient/node"
import * as shared from "./activate"
import type { TurtleApi } from "./activate"
import { documentSelector } from "./client"
import { findServer, planWorkspace, type VirtualNotice } from "./server"

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

let noticeShown = false

/** Says, once per window, that virtual folders are not checked, and offers to clone them or open them in the browser. */
async function showVirtualNotice(notice: VirtualNotice): Promise<void> {
  if (noticeShown) return
  noticeShown = true
  const clone = "Clone Repository"
  const browser = "Open in Browser"
  const choice = await vscode.window.showInformationMessage(notice.message, clone, browser)
  if (choice === clone) await vscode.commands.executeCommand("git.clone", ...(notice.cloneUrl ? [notice.cloneUrl] : []))
  else if (choice === browser) await vscode.env.openExternal(vscode.Uri.parse(notice.browserUrl))
}

export function activate(context: vscode.ExtensionContext): Promise<TurtleApi> {
  // A virtual workspace (GitHub Repositories) has no files on disk for the
  // binary: say so when an RDF document there is open, or opens.
  const plan = planWorkspace(
    (vscode.workspace.workspaceFolders ?? []).map((f) => ({ scheme: f.uri.scheme, authority: f.uri.authority, path: f.uri.path, name: f.name })),
  )
  const notice = plan.notice
  if (notice) {
    const unserved = documentSelector(plan.unserved)
    const tell = (doc: vscode.TextDocument): void => {
      if (vscode.languages.match(unserved, doc) > 0) void showVirtualNotice(notice)
    }
    vscode.workspace.textDocuments.forEach(tell)
    context.subscriptions.push(vscode.workspace.onDidOpenTextDocument(tell))
  }
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
      if (!plan.runServer) {
        if (notice) void showVirtualNotice(notice)
        return undefined
      }
      const command = findCommand(context)
      return command ? new LanguageClient("turtle", "Turtle", { command, args: [] }, options) : undefined
    },
  })
}

export const deactivate = shared.deactivate
