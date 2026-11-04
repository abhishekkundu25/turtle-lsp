// In the browser the server's workspace is the documents it has open. Each URI
// has two layers: the disk text the extension read, and the editor's document
// while the client has it open. The server sees their union, the editor's
// text winning. Each event returns the notifications that bring the server to
// the new union:
//
// - client didOpen: didOpen; a full-text didChange when the server has the
//   URI already (from disk).
// - client didChange: passed on (the layer keeps the edited text).
// - client didClose: a full-text didChange back to the disk text when there
//   is one (nothing when the texts are equal); else didClose.
// - disk changed: nothing while the editor has it (closing it brings the disk
//   text in) or when the text is the same; else didOpen of a new file or a
//   full-text didChange.
// - disk deleted: nothing while the editor has it (closing it later closes
//   it); else didClose.
// - replay(): didOpen of every URI in the union, for a fresh server.
//
// The versions the server sees are its own: they rise by one per message for
// each URI, across opens and closes. The client's versions are kept for the
// edits the server computes (`forClient`). URIs are the client's; the caller
// maps them for the server.

import { track } from "../../../../web/app/lsp-host.js"

/** A notification for the server (the host adds `jsonrpc`). */
export interface ServerNotification {
  method: "textDocument/didOpen" | "textDocument/didChange" | "textDocument/didClose"
  params: Record<string, unknown>
}

export interface TextDocumentItem {
  uri: string
  languageId: string
  version: number
  text: string
}

export interface ContentChange {
  range?: unknown
  text: string
}

type EditorDoc = Omit<TextDocumentItem, "uri">

export class Layers {
  // The client's open documents, as edited (uri -> doc), kept by the host's
  // `track` so that ranged edits apply as the RDF Editor applies them.
  private editor: { open: Map<string, EditorDoc> } = { open: new Map() }
  private disk = new Map<string, string>()
  // The last version the server was sent, per URI; never forgotten.
  private versions = new Map<string, number>()

  didOpen(doc: TextDocumentItem): ServerNotification[] {
    const known = this.has(doc.uri)
    track(this.editor, "textDocument/didOpen", { textDocument: doc })
    return [known ? this.whole(doc.uri, doc.text) : this.open(doc.uri, doc.languageId, doc.text)]
  }

  didChange(uri: string, version: number, contentChanges: ContentChange[]): ServerNotification[] {
    if (!this.editor.open.has(uri)) return []
    track(this.editor, "textDocument/didChange", { textDocument: { uri, version }, contentChanges })
    return [this.change(uri, contentChanges)]
  }

  didClose(uri: string): ServerNotification[] {
    const doc = this.editor.open.get(uri)
    if (!doc) return []
    track(this.editor, "textDocument/didClose", { textDocument: { uri } })
    const disk = this.disk.get(uri)
    if (disk === undefined) return [this.close(uri)]
    return disk === doc.text ? [] : [this.whole(uri, disk)]
  }

  diskChanged(uri: string, text: string): ServerNotification[] {
    const before = this.disk.get(uri)
    this.disk.set(uri, text)
    if (this.editor.open.has(uri) || before === text) return []
    return [before === undefined ? this.open(uri, "", text) : this.whole(uri, text)]
  }

  diskDeleted(uri: string): ServerNotification[] {
    if (!this.disk.delete(uri) || this.editor.open.has(uri)) return []
    return [this.close(uri)]
  }

  /** The client is gone (a new session starts): its documents are no longer open. Sends nothing. */
  forgetEditor(): void {
    this.editor.open.clear()
  }

  /** The union, for a server that has nothing open. */
  replay(): ServerNotification[] {
    const uris = new Set([...this.disk.keys(), ...this.editor.open.keys()])
    return [...uris].map((uri) => {
      const doc = this.editor.open.get(uri)
      return this.open(uri, doc?.languageId ?? "", doc?.text ?? (this.disk.get(uri) as string))
    })
  }

  /**
   * `value` (an answer of the server, URIs mapped back) with each versioned
   * document of a workspace edit's `documentChanges` given the client's
   * version, or null when the client does not have it open: the server's
   * versions mean nothing to the client, which refuses an edit whose version
   * is not its document's.
   */
  forClient<T>(value: T): T {
    const fix = (v: unknown): unknown => {
      if (Array.isArray(v)) return v.map(fix)
      if (v === null || typeof v !== "object") return v
      const out: Record<string, unknown> = {}
      for (const [k, x] of Object.entries(v)) out[k] = k === "documentChanges" && Array.isArray(x) ? x.map((e) => this.versioned(e)) : fix(x)
      return out
    }
    return fix(value) as T
  }

  // ---- internals ----

  private has(uri: string): boolean {
    return this.editor.open.has(uri) || this.disk.has(uri)
  }

  private next(uri: string): number {
    const v = (this.versions.get(uri) ?? 0) + 1
    this.versions.set(uri, v)
    return v
  }

  // An empty language id: the server goes by the file name, as for files on disk.
  private open(uri: string, languageId: string, text: string): ServerNotification {
    return { method: "textDocument/didOpen", params: { textDocument: { uri, languageId, version: this.next(uri), text } } }
  }

  private whole(uri: string, text: string): ServerNotification {
    return this.change(uri, [{ text }])
  }

  private change(uri: string, contentChanges: ContentChange[]): ServerNotification {
    return { method: "textDocument/didChange", params: { textDocument: { uri, version: this.next(uri) }, contentChanges } }
  }

  private close(uri: string): ServerNotification {
    return { method: "textDocument/didClose", params: { textDocument: { uri } } }
  }

  private versioned(entry: unknown): unknown {
    const doc = (entry as { textDocument?: { uri?: unknown; version?: unknown } } | null)?.textDocument
    if (typeof doc?.uri !== "string" || doc.version === undefined) return entry
    const version = this.editor.open.get(doc.uri)?.version ?? null
    return { ...(entry as object), textDocument: { ...doc, version } }
  }
}
