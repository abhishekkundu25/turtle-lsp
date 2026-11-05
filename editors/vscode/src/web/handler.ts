// The language client's side of the server worker: JSON-RPC messages from
// `vscode-languageclient/browser` in (plain objects, by postMessage), the
// RDF Editor's host (`EmbeddedServer`) driving the engine, JSON-RPC messages
// out. The engine takes one step at a time, so messages wait in a queue and
// whatever has come when it is free goes in as one engine call, less the
// requests cancelled meanwhile (as web/app/lsp-worker.js does).
//
// - Requests are answered under the client's ids; the host uses its own.
// - Document notifications and the extension's `turtle/web/files` go through
//   the disk and editor layers (layers.ts); `turtle/web/files` never reaches
//   the server.
// - Every URI is mapped both ways (uris.ts), the `initialize` params' and
//   error messages' too.
// - Server requests (refreshes, progress, registrations, configuration) are
//   answered by the host at once with null, as in the RDF Editor, and
//   forwarded to the client under fresh ids. The client's answers are
//   dropped, except to `workspace/configuration`: that answer reaches the
//   server as a `didChangeConfiguration` holding it.
// - `exit` stops the server's timers; a new `initialize` starts a fresh
//   server with the disk files (the client opens its documents again).
// - A crash restarts the server through the host, which replays what it was
//   sent (the layers' union), and the client is told by `window/showMessage`.

import { uriMap, type UriMap } from "./uris.ts"
import { Layers, type ContentChange } from "./layers.ts"
import { EmbeddedServer, plan } from "../../../../web/app/lsp-host.js"

type Json = Record<string, unknown>

export interface Engine {
  call(op: string, payload?: unknown): any
}

interface HostEvents {
  diagnostics(uri: string, list: unknown[]): void
  request(method: string, params: unknown): unknown
  notification(method: string, params: unknown): void
  crash(error: Error & { state: "restarted" | "dropped" | "off"; dropped: string[] }): void
}

type Item =
  | { kind: "initialize" | "request"; id: number | string; method: string; params: unknown }
  | { kind: "notify"; id?: undefined; method: string; params: unknown }
  | { kind: "cancel"; id: unknown }

/** The custom notification the browser entry sends with the workspace's files. */
export const FILES = "turtle/web/files"

/** `turtle/web/files` params. URIs are the client's. */
export interface WorkspaceFiles {
  /** The workspace folders' URIs, in order; replaces the folders `initialize` named. */
  workspaceFolders?: string[]
  changed: { uri: string; text: string }[]
  deleted: string[]
}

export interface Handler {
  /** A message the client posted. */
  receive(data: unknown): void
  /** Resolves when every message received so far is handled and no server timer is pending. */
  idle(): Promise<void>
}

const ErrorCodes = { InvalidRequest: -32600, InternalError: -32603, ServerNotInitialized: -32002, RequestCancelled: -32800 }
const MessageType = { Error: 1, Warning: 2 }

const isObject = (v: unknown): v is Json => typeof v === "object" && v !== null && !Array.isArray(v)
const isId = (v: unknown): v is number | string => typeof v === "number" || typeof v === "string"

// The params of a `turtle/web/files` notification, or null when malformed:
// entries that are not as described are left out.
function filesOf(p: unknown): WorkspaceFiles | null {
  if (!isObject(p)) return null
  const strings = (v: unknown): string[] => (Array.isArray(v) ? v.filter((s) => typeof s === "string") : [])
  const changed = Array.isArray(p.changed) ? p.changed.filter((f): f is { uri: string; text: string } => isObject(f) && typeof f.uri === "string" && typeof f.text === "string") : []
  const files: WorkspaceFiles = { changed, deleted: strings(p.deleted) }
  if (Array.isArray(p.workspaceFolders)) files.workspaceFolders = strings(p.workspaceFolders)
  return files
}

// The folders `initialize` names: its workspace folders, else its root.
function foldersOf(p: Json): string[] {
  if (Array.isArray(p.workspaceFolders)) return p.workspaceFolders.flatMap((f) => (isObject(f) && typeof f.uri === "string" ? [f.uri] : []))
  return typeof p.rootUri === "string" ? [p.rootUri] : []
}

// The client watches files for the native server; here the files come in
// `turtle/web/files`, so the server is not told it can register watchers.
function withoutWatchers(p: Json): Json {
  const caps = isObject(p.capabilities) ? p.capabilities : {}
  const workspace = isObject(caps.workspace) ? { ...caps.workspace, didChangeWatchedFiles: undefined } : caps.workspace
  return { ...p, capabilities: { ...caps, workspace } }
}

/** The worker's handling of client messages: `post` sends one to the client. */
export function createHandler(engine: Engine | Promise<Engine>, post: (message: Json) => void): Handler {
  const ready = Promise.resolve(engine)
  ready.catch(() => {}) // answered to each request, in pump()
  const layers = new Layers()
  let uris: UriMap = uriMap([])
  let server: EmbeddedServer | null = null
  // Server requests forwarded to the client: our id -> method.
  const forwarded = new Map<string, string>()
  let forwards = 0
  const queue: Item[] = []
  let pumping: Promise<void> | null = null

  const respond = (id: number | string, answer: { result: unknown } | { error: { code: number; message: string } }) => post({ jsonrpc: "2.0", id, ...answer })
  const fail = (id: number | string, code: number, message: string) => respond(id, { error: { code, message } })
  const notifyClient = (method: string, params: unknown) => post({ jsonrpc: "2.0", method, params: uris.toClient(params) })

  const events: HostEvents = {
    diagnostics: (uri, diagnostics) => notifyClient("textDocument/publishDiagnostics", { uri, diagnostics }),
    notification: notifyClient,
    request(method, params) {
      const id = `turtle-web-${++forwards}`
      forwarded.set(id, method)
      post({ jsonrpc: "2.0", id, method, params: uris.toClient(params) })
      return null
    },
    crash(e) {
      const files = e.dropped.map((u) => uris.toClient(u)).join(", ")
      const message =
        e.state === "off"
          ? `The Turtle language server crashed repeatedly and is off until it is restarted (Turtle: Restart Language Server): ${e.message}`
          : e.state === "dropped"
            ? `The Turtle language server crashed and was restarted without ${files}, which crash it: ${e.message}`
            : `The Turtle language server crashed and was restarted with your files: ${e.message}`
      post({ jsonrpc: "2.0", method: "window/showMessage", params: { type: e.state === "restarted" ? MessageType.Warning : MessageType.Error, message } })
    },
  }

  // Layer output to the server (nothing while there is none: `initialize` replays).
  const send = (messages: { method: string; params: unknown }[]) => {
    for (const m of messages) server?.notify(m.method, uris.toServer(m.params))
  }

  function files(params: unknown): void {
    const f = filesOf(params)
    if (!f) return
    if (f.workspaceFolders) uris = uriMap(f.workspaceFolders)
    for (const { uri, text } of f.changed) send(layers.diskChanged(uri, text))
    for (const uri of f.deleted) send(layers.diskDeleted(uri))
  }

  function notification(method: string, params: unknown): void {
    const doc = isObject(params) && isObject(params.textDocument) && typeof params.textDocument.uri === "string" ? (params.textDocument as Json & { uri: string }) : null
    switch (method) {
      case FILES:
        return files(params)
      case "textDocument/didOpen":
        if (doc && typeof doc.text === "string") send(layers.didOpen({ uri: doc.uri, languageId: String(doc.languageId ?? ""), version: Number(doc.version), text: doc.text }))
        return
      case "textDocument/didChange":
        if (doc && Array.isArray((params as Json).contentChanges)) send(layers.didChange(doc.uri, Number(doc.version), (params as { contentChanges: ContentChange[] }).contentChanges))
        return
      case "textDocument/didClose":
        if (doc) send(layers.didClose(doc.uri))
        return
      case "initialized": // the host sent it with initialize
      case "workspace/didChangeWatchedFiles": // the files come in turtle/web/files
        return
      case "exit":
        server?.notify(method, params)
        return end()
      default:
        server?.notify(method, uris.toServer(params))
    }
  }

  // The session ends; the next `initialize` starts a fresh server.
  function end(): void {
    server?.stop()
    server = null
  }

  async function initialize(id: number | string, params: unknown, e: Engine): Promise<void> {
    const p = isObject(params) ? params : {}
    end()
    uris = uriMap(foldersOf(p))
    layers.forgetEditor()
    try {
      const fresh = new EmbeddedServer(e, events)
      const result = await fresh.initialize(uris.toServer(withoutWatchers(p)))
      server = fresh
      fresh.batch(() => send(layers.replay()))
      respond(id, { result: uris.toClient(result ?? null) })
    } catch (err) {
      fail(id, ErrorCodes.InternalError, `The Turtle language server did not start: ${(err as Error)?.message ?? err}`)
    }
  }

  // Requests and notifications in one engine call; the answers as they settle.
  function batch(items: Item[]): Promise<unknown>[] {
    const answers: Promise<unknown>[] = []
    const run = () => {
      for (const item of items) {
        if (item.kind === "notify") notification(item.method, item.params)
        else if (item.kind === "request") {
          const { id, method } = item
          if (!server) {
            fail(id, ErrorCodes.ServerNotInitialized, "The Turtle language server is not initialized.")
            continue
          }
          answers.push(
            server.request(method, uris.toServer(item.params)).then(
              (result) => respond(id, { result: layers.forClient(uris.toClient(result ?? null)) }),
              (err: { code?: number; message?: string }) => fail(id, err?.code ?? ErrorCodes.InternalError, uris.toClient({ message: String(err?.message ?? err) }).message),
            ),
          )
        }
      }
    }
    try {
      if (server) server.batch(run)
      else run()
    } catch {
      // The engine refused the batch: its requests were answered with the error.
    }
    return answers
  }

  async function handle(items: Item[], e: Engine): Promise<void> {
    const { run, dropped }: { run: Item[]; dropped: Item[] } = plan(items)
    for (const item of dropped) if (isId(item.id)) fail(item.id, ErrorCodes.RequestCancelled, "The request was cancelled.")
    const answers: Promise<unknown>[] = []
    let pending: Item[] = []
    for (const item of run) {
      if (item.kind !== "initialize") {
        pending.push(item)
        continue
      }
      answers.push(...batch(pending))
      pending = []
      await initialize(item.id, item.params, e)
    }
    answers.push(...batch(pending))
    await Promise.all(answers)
  }

  async function pump(): Promise<void> {
    while (queue.length) {
      let e: Engine
      try {
        e = await ready
      } catch (err) {
        for (const item of queue.splice(0)) {
          if (item.kind !== "notify" && item.kind !== "cancel") fail(item.id, ErrorCodes.InternalError, `The Turtle engine did not load: ${(err as Error)?.message ?? err}`)
        }
        continue
      }
      // Whatever came while the engine was busy is in this batch.
      await handle(queue.splice(0), e)
      // A turn of the event loop, for the messages that came meanwhile.
      await new Promise((r) => setTimeout(r))
    }
  }

  function enqueue(item: Item): void {
    queue.push(item)
    wake()
  }

  // An item queued after the pump's last look at the queue starts it again.
  function wake(): void {
    pumping ??= pump().finally(() => {
      pumping = null
      if (queue.length) wake()
    })
  }

  return {
    receive(data) {
      if (!isObject(data)) return
      const { id, method, params } = data
      if (typeof method !== "string") {
        // A response to a forwarded server request.
        if (!isId(id)) return
        const asked = forwarded.get(String(id))
        forwarded.delete(String(id))
        if (asked === "workspace/configuration" && Array.isArray(data.result) && isObject(data.result[0])) {
          enqueue({ kind: "notify", method: "workspace/didChangeConfiguration", params: { settings: { turtle: data.result[0] } } })
        }
      } else if (method === "$/cancelRequest") {
        if (isObject(params) && isId(params.id)) enqueue({ kind: "cancel", id: params.id })
      } else if (id === undefined) {
        enqueue({ kind: "notify", method, params })
      } else if (!isId(id)) {
        post({ jsonrpc: "2.0", id: null, error: { code: ErrorCodes.InvalidRequest, message: "A request id is a number or a string." } })
      } else {
        enqueue({ kind: method === "initialize" ? "initialize" : "request", id, method, params })
      }
    },
    async idle() {
      while (pumping) await pumping
      await server?.settle()
    },
  }
}
