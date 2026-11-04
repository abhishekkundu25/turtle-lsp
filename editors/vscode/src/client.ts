// The parts of the client that need neither `vscode` nor Node, so both
// entries share them and Node tests them directly (test/find-server.test.mjs,
// test/configuration.test.mjs, test/manifest.test.mjs): the settings the
// server is sent, and a handle that starts, restarts and stops the language
// client one step at a time.

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
