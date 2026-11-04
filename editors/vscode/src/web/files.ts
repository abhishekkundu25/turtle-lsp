// The workspace's RDF files for the server worker, which has no disk: found
// with the extension's glob, read with `vscode.workspace.fs`, sent as
// `turtle/web/files` (handler.ts), and kept current with a file system
// watcher. Every read is checked here (unreadable, not UTF-8, past the caps);
// the worker trusts what it is sent.
import * as vscode from "vscode"
import { RDF_GLOB } from "../client"
import type { WorkspaceFiles } from "./handler"

/** At most this many files are read... */
export const MAX_FILES = 2000
/** ...and this many bytes of text in all. */
export const MAX_BYTES = 50 * 1024 * 1024
/** Files per notification, so no one message stalls the worker. */
const CHUNK = 200
const EXCLUDE = "**/node_modules/**"

const utf8 = new TextDecoder("utf-8", { fatal: true })

/** `uri`'s text, or undefined when it cannot be read or is not UTF-8. */
async function read(uri: vscode.Uri): Promise<{ text: string; bytes: number } | undefined> {
  try {
    const bytes = await vscode.workspace.fs.readFile(uri)
    return { text: utf8.decode(bytes), bytes: bytes.byteLength }
  } catch {
    return undefined
  }
}

const count = (n: number): string => n.toLocaleString("en-US")
const inNodeModules = (uri: vscode.Uri): boolean => uri.path.split("/").includes("node_modules")

export interface WorkspaceFileSync extends vscode.Disposable {
  /** Reads every file again and sends them all, with the workspace folders: after each (re)start of the server. */
  sendAll(): Promise<void>
}

/**
 * Keeps the server's copy of the workspace files. `send` delivers one
 * notification to the running server (or drops it when there is none: the
 * next start sends everything again).
 */
export function workspaceFileSync(send: (files: WorkspaceFiles) => Promise<void>): WorkspaceFileSync {
  // The files sent, with their size in bytes.
  const sent = new Map<string, number>()
  let bytes = 0
  let told = false
  // One step at a time, so a watcher event after a full send wins.
  let tail: Promise<void> = Promise.resolve()
  const enqueue = (step: () => Promise<void>): Promise<void> => (tail = tail.then(step).catch(() => {}))

  const fits = (uri: string, size: number): boolean =>
    sent.has(uri) ? bytes - sent.get(uri)! + size <= MAX_BYTES : sent.size < MAX_FILES && bytes + size <= MAX_BYTES
  const record = (uri: string, size: number): void => {
    bytes += size - (sent.get(uri) ?? 0)
    sent.set(uri, size)
  }
  const forget = (uri: string): void => {
    bytes -= sent.get(uri) ?? 0
    sent.delete(uri)
  }

  async function sendAll(): Promise<void> {
    sent.clear()
    bytes = 0
    const found = await vscode.workspace.findFiles(RDF_GLOB, EXCLUDE)
    let unreadable = 0
    let capped = false
    let first = true
    for (let i = 0; i < found.length && !capped; i += CHUNK) {
      const chunk = found.slice(i, i + CHUNK)
      const texts = await Promise.all(chunk.map(read))
      const changed: WorkspaceFiles["changed"] = []
      for (const [j, file] of texts.entries()) {
        const uri = chunk[j].toString()
        if (!file) unreadable++
        else if (!fits(uri, file.bytes)) {
          capped = true
          break
        } else {
          record(uri, file.bytes)
          changed.push({ uri, text: file.text })
        }
      }
      await send({ ...(first ? folders() : {}), changed, deleted: [] })
      first = false
    }
    if (first) await send({ ...folders(), changed: [], deleted: [] })
    if (told || (!capped && unreadable === 0)) return
    told = true
    const parts: string[] = []
    if (capped) {
      parts.push(
        `Turtle read ${count(sent.size)} of the workspace's ${count(found.length)} RDF files: in the browser it reads at most ${count(MAX_FILES)} files and ${MAX_BYTES / 1024 / 1024} MB, and the rest are not checked or found by references.`,
      )
    }
    if (unreadable) parts.push(`${unreadable} ${unreadable === 1 ? "file" : "files"} could not be read as UTF-8 text and ${unreadable === 1 ? "was" : "were"} skipped.`)
    void vscode.window.showInformationMessage(parts.join(" "))
  }

  async function changed(uri: vscode.Uri): Promise<void> {
    if (inNodeModules(uri)) return
    const key = uri.toString()
    const file = await read(uri)
    if (file && fits(key, file.bytes)) {
      record(key, file.bytes)
      await send({ changed: [{ uri: key, text: file.text }], deleted: [] })
    } else if (sent.has(key)) {
      // No longer readable or past the cap: the server forgets it rather than keep stale text.
      forget(key)
      await send({ changed: [], deleted: [key] })
    }
  }

  async function deleted(uri: vscode.Uri): Promise<void> {
    const key = uri.toString()
    if (!sent.has(key)) return
    forget(key)
    await send({ changed: [], deleted: [key] })
  }

  const watcher = vscode.workspace.createFileSystemWatcher(RDF_GLOB)
  const disposables = [
    watcher,
    watcher.onDidCreate((uri) => enqueue(() => changed(uri))),
    watcher.onDidChange((uri) => enqueue(() => changed(uri))),
    watcher.onDidDelete((uri) => enqueue(() => deleted(uri))),
  ]
  return {
    sendAll: () => enqueue(sendAll),
    dispose: () => disposables.forEach((d) => d.dispose()),
  }
}

function folders(): Pick<WorkspaceFiles, "workspaceFolders"> {
  return { workspaceFolders: (vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.toString()) }
}
