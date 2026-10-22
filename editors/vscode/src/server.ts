// The parts of the extension that do not need the `vscode` module, so Node can
// test them directly (test/find-server.test.mjs): which server binary to run,
// the settings the server is sent, and a handle that starts, restarts and
// stops the language client one step at a time.
import * as nodePath from "node:path"

// ---------------------------------------------------------------------------
// Finding the binary

/** The file-system calls `findServer` makes; tests pass a stub. */
export interface ServerFs {
  existsSync(p: string): boolean
  /** Resolves symlinks; when absent, paths are compared as given. */
  realpathSync?(p: string): string
}

/** The part of `node:path` that `findServer` uses. */
export type PathApi = Pick<typeof nodePath, "resolve" | "relative" | "isAbsolute" | "join" | "sep" | "delimiter">

export interface FindServerOptions {
  /** `turtle.server.path`, as the user set it (may be empty). */
  configured?: string
  /** `turtle-lsp` or `turtle-lsp.exe`. */
  exe: string
  /** Where the .vsix bundles the server, then where a repository checkout builds it. */
  candidates: string[]
  /** The `PATH` variable. */
  pathEnv?: string
  /** Whether VS Code trusts the workspace. */
  trusted: boolean
  /** Directories of the open workspace folders. */
  workspaceFolders: string[]
  fs: ServerFs
  /** `node:path` (or its `win32` or `posix` flavour); default: the host's. */
  path?: PathApi
}

export type FoundServer = { command: string } | { error: string }

const NOT_FOUND = "turtle-lsp binary not found. Set `turtle.server.path`, or build it with `cargo build --release`."

/**
 * Picks the binary to run. A configured path is used as it is, or reported
 * when it is relative or missing (never silently replaced by another binary).
 * Otherwise the first of the bundled and checkout candidates, then the
 * absolute `PATH` entries, that exists. A relative or empty `PATH` entry means
 * the current directory, which is the workspace for a spawned process, so it
 * is never used. In an untrusted workspace nothing inside a workspace folder
 * runs.
 */
export function findServer(opts: FindServerOptions): FoundServer {
  const p = opts.path ?? nodePath
  const real = (file: string): string => {
    try {
      return opts.fs.realpathSync ? opts.fs.realpathSync(file) : file
    } catch {
      return file
    }
  }
  const folders = opts.workspaceFolders.map((f) => p.resolve(f))
  const inWorkspace = (file: string): boolean => {
    const target = real(p.resolve(file))
    return folders.some((folder) => {
      for (const root of new Set([folder, real(folder)])) {
        const rel = p.relative(root, target)
        if (rel !== "" && rel !== ".." && !rel.startsWith(`..${p.sep}`) && !p.isAbsolute(rel)) return true
      }
      return false
    })
  }
  const refused = (file: string): boolean => !opts.trusted && inWorkspace(file)

  const configured = opts.configured?.trim()
  if (configured) {
    if (!p.isAbsolute(configured)) return { error: `turtle.server.path must be an absolute path, not "${configured}".` }
    if (!opts.fs.existsSync(configured)) return { error: `turtle.server.path does not exist: ${configured}` }
    if (refused(configured)) {
      return { error: `turtle.server.path is inside the workspace, which is not trusted: ${configured}` }
    }
    return { command: configured }
  }

  const onPath = (opts.pathEnv ?? "")
    .split(p.delimiter)
    .filter((dir) => dir !== "" && p.isAbsolute(dir))
    .map((dir) => p.join(dir, opts.exe))
  for (const file of [...opts.candidates, ...onPath]) {
    if (opts.fs.existsSync(file) && !refused(file)) return { command: file }
  }
  return { error: NOT_FOUND }
}

// ---------------------------------------------------------------------------
// Settings

/** One scope-by-scope reading of a setting, as `WorkspaceConfiguration.inspect` returns it. */
export interface Inspected {
  globalValue?: unknown
  workspaceValue?: unknown
  workspaceFolderValue?: unknown
}

export interface SettingsSource {
  /** `contributes.configuration.properties` of the manifest. */
  properties: Record<string, unknown>
  /** `capabilities.untrustedWorkspaces.restrictedConfigurations` of the manifest. */
  restricted: readonly string[]
  /** Reads `key` relative to the `turtle` section, e.g. `format.lineWidth`. */
  inspect(key: string): Inspected | undefined
  trusted: boolean
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === "object" && v !== null && !Array.isArray(v)

/** The `turtle.*` settings that belong to the editor, not the server. */
const isClientSetting = (key: string): boolean => key.startsWith("server.") || key.startsWith("trace.")

/**
 * The `turtle.*` settings to send the server, as nested objects. Only values
 * the user set go: an unset setting is left out, so `turtle-lsp.json` (which the
 * server reads first) decides it, not the default the manifest contributes. In
 * an untrusted workspace a restricted setting takes the user's value only, and
 * network imports are off.
 */
export function settingsFrom(src: SettingsSource): Record<string, unknown> {
  const out: Record<string, unknown> = {}
  for (const full of Object.keys(src.properties)) {
    if (!full.startsWith("turtle.")) continue
    const key = full.slice("turtle.".length)
    if (isClientSetting(key)) continue
    const seen = src.inspect(key)
    if (!seen) continue
    const layers = [seen.globalValue]
    if (src.trusted || !src.restricted.includes(full)) layers.push(seen.workspaceValue, seen.workspaceFolderValue)
    const set = layers.filter((v) => v !== undefined)
    if (set.length === 0) continue
    // Objects (the prefixes) merge across scopes; anything else takes the narrowest.
    const value = set.every(isObject) ? Object.assign({}, ...set) : set[set.length - 1]
    put(out, key.split("."), value)
  }
  if (!src.trusted) put(out, ["imports", "network"], false)
  return out
}

/**
 * The `workspace/didChangeConfiguration` params for `settings` (what
 * `settingsFrom` returns): the server reads them as the `turtle` section, and
 * that section replaces the settings it was started with.
 */
export function configurationChange(settings: Record<string, unknown>): { settings: { turtle: Record<string, unknown> } } {
  return { settings: { turtle: settings } }
}

function put(target: Record<string, unknown>, path: string[], value: unknown): void {
  let at = target
  for (const step of path.slice(0, -1)) {
    const next = at[step]
    at = isObject(next) ? next : (at[step] = {})
  }
  at[path[path.length - 1]] = value
}

// ---------------------------------------------------------------------------
// Starting and stopping

/** What `ServerHandle` needs of a `LanguageClient`. */
export interface ClientLike {
  start(): Promise<void>
  stop(): Promise<void>
}

/**
 * Owns the language client. Starting, restarting and stopping run one at a
 * time, in the order asked. A client that failed to start is dropped (it
 * cannot be stopped, and `stop()` throws unless it is running), and every
 * restart builds a new one.
 */
export class ServerHandle<C extends ClientLike> {
  private current: C | undefined = undefined
  private tail: Promise<void> = Promise.resolve()
  private disposed = false
  private readonly create: () => C | undefined

  /** `create` returns a new, not yet started client, or `undefined` (after telling the user why) when there is no server to run. */
  constructor(create: () => C | undefined) {
    this.create = create
  }

  /** The current client, running or still starting. */
  client(): C | undefined {
    return this.current
  }

  /** Settles when everything asked so far has finished. */
  idle(): Promise<void> {
    return this.tail
  }

  /** Starts the client unless there is one. Rejects when the start fails. */
  ensureStarted(): Promise<C | undefined> {
    return this.enqueue(async () => {
      if (this.disposed) return undefined
      return this.current ?? (await this.startNew())
    })
  }

  /** Stops the client, if any, and starts a new one. Rejects when the start fails. */
  restart(): Promise<C | undefined> {
    return this.enqueue(async () => {
      if (this.disposed) return undefined
      await this.stopCurrent()
      return this.startNew()
    })
  }

  /** Stops the client for good; later calls do nothing. */
  dispose(): Promise<void> {
    this.disposed = true
    return this.enqueue(() => this.stopCurrent())
  }

  private enqueue<T>(task: () => Promise<T>): Promise<T> {
    const run = this.tail.then(task)
    this.tail = run.then(
      () => undefined,
      () => undefined,
    )
    return run
  }

  private async startNew(): Promise<C | undefined> {
    const client = this.create()
    if (!client) return undefined
    this.current = client
    try {
      await client.start()
    } catch (err) {
      this.current = undefined
      await quietStop(client)
      throw err
    }
    return client
  }

  private async stopCurrent(): Promise<void> {
    const client = this.current
    this.current = undefined
    if (!client) return
    await quietStop(client)
  }
}

/** Stops `client`, ignoring the error of one that never ran or is already gone. */
async function quietStop(client: ClientLike): Promise<void> {
  try {
    await client.stop()
  } catch {
    // vscode-languageclient's stop() throws unless the client is running.
  }
}
