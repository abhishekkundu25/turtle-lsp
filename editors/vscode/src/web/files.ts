// The workspace's RDF files for the server worker, which has no disk: found
// with the extension's glob, read with `vscode.workspace.fs`, sent as
// `turtle/web/files` (handler.ts), and kept current with a file system
// watcher. Every read is checked here (unreadable, not UTF-8, past the caps;
// the caps from each file's size before it is read, budget.ts); the worker
// trusts what it is sent.
import * as vscode from "vscode"
import { RDF_GLOB } from "../client"
import { gather, type Io, type Room } from "./budget"
import type { WorkspaceFiles } from "./handler"

/** At most this many files are read... */
export const MAX_FILES = 2000
/** ...and this many bytes of text in all. */
export const MAX_BYTES = 50 * 1024 * 1024
const MB = MAX_BYTES / 1024 / 1024
const EXCLUDE = "**/node_modules/**"

const utf8 = new TextDecoder("utf-8", { fatal: true })

const io: Io<vscode.Uri> = {
  size: async (uri) => (await vscode.workspace.fs.stat(uri)).size,
  async read(uri) {
    const bytes = await vscode.workspace.fs.readFile(uri)
    return { text: utf8.decode(bytes), bytes: bytes.byteLength }
  },
}

const count = (n: number): string => n.toLocaleString("en-US")
const files = (n: number): string => `${count(n)} ${n === 1 ? "file" : "files"}`
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

  // What is left of the caps for `uri`, counting what it already takes.
  const roomFor = (uri: string): Room => {
    const own = sent.get(uri)
    return own === undefined
      ? { files: MAX_FILES - sent.size, bytes: MAX_BYTES - bytes }
      : { files: MAX_FILES - sent.size + 1, bytes: MAX_BYTES - bytes + own }
  }
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
    let first = true
    const tally = await gather(found, io, { files: MAX_FILES, bytes: MAX_BYTES }, async (batch) => {
      const changed = batch.map((file) => {
        const uri = file.uri.toString()
        record(uri, file.bytes)
        return { uri, text: file.text }
      })
      await send({ ...(first ? folders() : {}), changed, deleted: [] })
      first = false
    })
    if (first) await send({ ...folders(), changed: [], deleted: [] })
    const capped = tally.tooMany + tally.tooBig
    if (told || (capped === 0 && tally.unreadable === 0)) return
    told = true
    const parts: string[] = []
    if (capped) {
      const why: string[] = []
      if (tally.tooMany) why.push(`${files(tally.tooMany)} past the ${count(MAX_FILES)}-file limit`)
      if (tally.tooBig) why.push(`${files(tally.tooBig)} too large for what was left of the ${MB} MB limit`)
      parts.push(
        `Turtle read ${count(tally.read)} of the workspace's ${count(found.length)} RDF files: in the browser it reads at most ${count(MAX_FILES)} files and ${MB} MB of text. It skipped ${why.join(" and ")}; they are not checked or found by references.`,
      )
    }
    if (tally.unreadable) parts.push(`${files(tally.unreadable)} could not be read as UTF-8 text and ${tally.unreadable === 1 ? "was" : "were"} skipped.`)
    void vscode.window.showInformationMessage(parts.join(" "))
  }

  async function changed(uri: vscode.Uri): Promise<void> {
    if (inNodeModules(uri)) return
    const key = uri.toString()
    const tally = await gather([uri], io, roomFor(key), async ([file]) => {
      record(key, file.bytes)
      await send({ changed: [{ uri: key, text: file.text }], deleted: [] })
    })
    if (tally.read === 0 && sent.has(key)) {
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
