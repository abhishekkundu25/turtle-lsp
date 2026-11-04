// The browser entry (vscode.dev, github.dev): the server is turtle-lsp's
// engine in a web worker (worker.ts). It has no disk, so this side sends it
// the workspace's files (files.ts) and layers `turtle-lsp.json` under the
// user's settings; Run query, which writes its results to disk, is left out.
// What the extension does with the client is in src/activate.ts, shared with
// the desktop entry.
import * as vscode from "vscode"
import { LanguageClient, State, type InitializeParams, type LanguageClientOptions } from "vscode-languageclient/browser"
import * as shared from "../activate"
import type { TurtleApi } from "../activate"
import { documentSelector, settingsFile } from "../client"
import { workspaceFileSync } from "./files"
import { FILES } from "./handler"

export type { TurtleApi }

// The server's lenses that run a query (src/commands.rs): the browser has no
// disk for the results. The `turtle/query` request still works.
const QUERY_COMMANDS = new Set(["turtle.runQuery", "turtle.cancelQuery"])

const SETTINGS_FILE = "turtle-lsp.json"
/** The largest settings file read, as the native server (src/settings.rs). */
const SETTINGS_FILE_MAX_BYTES = 1 << 20

// The tsconfig has no DOM lib; the extension host has a Worker.
declare const Worker: new (url: string) => WebWorker
interface WebWorker {
  postMessage(message: unknown): void
  terminate(): void
}

/** The server worker, told where the engine is (worker.ts). */
function serverWorker(script: string, engine: string): WebWorker {
  const worker = new Worker(script)
  worker.postMessage(engine)
  return worker
}

/**
 * A client whose server runs in its own worker, ended when the client stops.
 * The client's own restarts (after a crash) reuse the worker, which takes a
 * new `initialize`.
 */
class WorkerClient extends LanguageClient {
  private readonly worker: WebWorker

  constructor(worker: WebWorker, options: LanguageClientOptions) {
    // Passed as is: the client checks a factory's result with `instanceof
    // Worker`, which the extension host's own Worker class fails.
    super("turtle", "Turtle", worker as never, options)
    this.worker = worker
  }

  // The client pulls diagnostics only for the documents it has open; the
  // embedded server has every workspace file open (the files sent from disk)
  // and pushes theirs when the client cannot pull, so a problem in a file
  // never opened is in the Problems panel.
  protected override fillInitializeParams(params: InitializeParams): void {
    super.fillInitializeParams(params)
    delete params.capabilities.textDocument?.diagnostic
  }

  // stop() ends the session (shutdown, exit) but leaves the worker running;
  // dispose() and the client's own shutdown on errors come through here too.
  override async stop(timeout?: number): Promise<void> {
    try {
      await super.stop(timeout)
    } finally {
      this.worker.terminate()
    }
  }
}

/**
 * The URI schemes the server serves: the workspace folders' (`vscode-vfs`
 * on github.dev, `file` for a local folder, `vscode-test-web` …), local
 * files and untitled documents. Not every scheme: `git:` (the old side of a
 * diff) and other extensions' virtual documents would be second copies.
 */
function schemes(): string[] {
  return [...new Set(["file", "untitled", ...(vscode.workspace.workspaceFolders ?? []).map((f) => f.uri.scheme)])]
}

/** `turtle-lsp.json` at the first workspace folder, layered under the user's settings. */
function settingsFileLayer(context: vscode.ExtensionContext): { current(): Record<string, unknown>; onDidChange: vscode.Event<void>; load(): Promise<void> } & vscode.Disposable {
  const root = vscode.workspace.workspaceFolders?.[0]?.uri
  const properties = context.extension.packageJSON.contributes.configuration.properties
  const changed = new vscode.EventEmitter<void>()
  const shown = new Set<string>()
  let settings: Record<string, unknown> = {}
  const warn = (message: string): void => {
    if (shown.has(message)) return
    shown.add(message)
    void vscode.window.showWarningMessage(`turtle-lsp: ${message}`)
  }

  async function read(uri: vscode.Uri): Promise<unknown> {
    let bytes: Uint8Array
    try {
      bytes = await vscode.workspace.fs.readFile(uri)
    } catch (err) {
      if (err instanceof vscode.FileSystemError && err.code === "FileNotFound") return undefined
      throw new Error(err instanceof Error ? err.message : String(err))
    }
    if (bytes.byteLength > SETTINGS_FILE_MAX_BYTES) throw new Error(`larger than ${SETTINGS_FILE_MAX_BYTES >> 20} MB`)
    let text: string
    try {
      text = new TextDecoder("utf-8", { fatal: true }).decode(bytes)
    } catch {
      throw new Error("not valid UTF-8")
    }
    const value = JSON.parse(text)
    if (typeof value !== "object" || value === null || Array.isArray(value)) throw new Error("expected a JSON object")
    return value
  }

  async function load(): Promise<void> {
    if (!root) return
    try {
      const raw = await read(vscode.Uri.joinPath(root, SETTINGS_FILE))
      const file = raw ? settingsFile(raw as Record<string, unknown>, (key) => properties[`turtle.${key}`]?.default) : { settings: {}, ignored: [] }
      settings = file.settings
      if (file.ignored.length) {
        warn(`${SETTINGS_FILE}: ignored ${file.ignored.join(", ")}: a settings file cannot turn on network access or raise a limit past its default; set them in the editor`)
      }
    } catch (err) {
      settings = {}
      warn(`${SETTINGS_FILE}: ${err instanceof Error ? err.message : String(err)}; its settings are ignored`)
    }
  }

  const disposables: vscode.Disposable[] = [changed]
  if (root) {
    const watcher = vscode.workspace.createFileSystemWatcher(new vscode.RelativePattern(root, SETTINGS_FILE))
    const reload = async () => {
      await load()
      changed.fire()
    }
    disposables.push(watcher, watcher.onDidCreate(reload), watcher.onDidChange(reload), watcher.onDidDelete(reload))
  }
  return {
    current: () => settings,
    onDidChange: changed.event,
    load,
    dispose: () => disposables.forEach((d) => d.dispose()),
  }
}

export async function activate(context: vscode.ExtensionContext): Promise<TurtleApi> {
  const script = vscode.Uri.joinPath(context.extensionUri, "out/web/worker.js").toString(true)
  const engine = vscode.Uri.joinPath(context.extensionUri, "out/web/engine.wasm").toString(true)
  let current: LanguageClient | undefined
  const files = workspaceFileSync(async (message) => {
    if (current?.isRunning()) await current.sendNotification(FILES, message)
  })
  const baseSettings = settingsFileLayer(context)
  await baseSettings.load()
  context.subscriptions.push(files, baseSettings)
  return shared.activate(context, {
    documentSelector: documentSelector(schemes()),
    baseSettings,
    createClient: (options) => {
      const client = new WorkerClient(serverWorker(script, engine), {
        ...options,
        middleware: {
          ...options.middleware,
          provideCodeLenses: async (document, token, next) => {
            const lenses = await next(document, token)
            return lenses?.filter((l) => !QUERY_COMMANDS.has(l.command?.command ?? ""))
          },
        },
      })
      current = client
      // Every start sends every file (a new worker has none; to the same
      // worker, after the client's own restart, a file sent again changes nothing).
      client.onDidChangeState((e) => {
        if (e.newState === State.Running && current === client) void files.sendAll()
      })
      return client
    },
  })
}

export const deactivate = shared.deactivate
