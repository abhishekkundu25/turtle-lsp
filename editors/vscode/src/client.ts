// The parts of the client that need neither `vscode` nor Node, so both
// entries share them and Node tests them directly (test/find-server.test.mjs,
// test/configuration.test.mjs, test/manifest.test.mjs): the settings the
// server is sent, the documents it serves, and a handle that starts, restarts
// and stops the language client one step at a time.

// ---------------------------------------------------------------------------
// The served files

/**
 * The workspace's RDF and SPARQL files: the `workspaceContains` activation
 * glob (test/manifest.test.mjs checks they are the same) and the extensions
 * of src/workspace/kinds.rs.
 */
export const RDF_GLOB = "**/*.{ttl,turtle,trig,nt,nq,rq,sparql,ru,jsonld}"

/** The languages the extension contributes; the server handles each. */
export const LANGUAGES = ["turtle", "trig", "ntriples", "nquads", "sparql"] as const

// JSON-LD files keep VS Code's own JSON language (and its highlighting): the
// server attaches to them by name, and detects JSON-LD from the extension.
export const JSONLD_PATTERN = "**/*.jsonld"

/** One entry of the client's document selector. */
export type DocumentFilter = { scheme: string; language: string } | { scheme: string; pattern: string }

/** The documents the server handles, in the URI `schemes` given. An untitled document has no `.jsonld` name. */
export function documentSelector(schemes: readonly string[]): DocumentFilter[] {
  return [
    ...LANGUAGES.flatMap((language) => schemes.map((scheme) => ({ scheme, language }))),
    ...schemes.filter((s) => s !== "untitled").map((scheme) => ({ scheme, pattern: JSONLD_PATTERN })),
  ]
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

/**
 * The keys a settings file may lower but not raise: past the default, or 0
 * (no limit), they take the default. `imports.network` it may not set at
 * all. As the native server reads `turtle-lsp.json` (`Layer::File` in
 * src/settings.rs): any repository can ship one.
 */
const FILE_LIMITS = ["query.timeoutSeconds", "query.maxRows", "query.maxBufferedRows", "shacl.maxTriples"]

/**
 * The settings of a parsed `turtle-lsp.json` (keys nested or dotted,
 * optionally inside a top-level `"turtle"` object), as nested objects, with
 * the file's limits applied: `defaultOf(key)` is the manifest's default for
 * `turtle.<key>`. `ignored` names the keys the file asked for and did not get.
 */
export function settingsFile(file: Record<string, unknown>, defaultOf: (key: string) => unknown): { settings: Record<string, unknown>; ignored: string[] } {
  const settings: Record<string, unknown> = {}
  const walk = (o: Record<string, unknown>, at: string[]): void => {
    for (const [k, v] of Object.entries(o)) {
      const path = [...at, ...k.split(".")]
      if (path[0] === "turtle") path.shift()
      if (path.length === 0) {
        if (isObject(v)) walk(v, [])
      } else if (path[0] === "prefixes") {
        // A prefix name may hold dots; a prefixes object is one value.
        if (path.length > 1) put(settings, ["prefixes", path.slice(1).join(".")], v)
        else if (isObject(v)) for (const [name, iri] of Object.entries(v)) put(settings, ["prefixes", name], iri)
      } else if (isObject(v)) walk(v, path)
      else put(settings, path, v)
    }
  }
  walk(file, [])
  const ignored: string[] = []
  const imports = settings.imports
  if (isObject(imports) && "network" in imports) {
    if (imports.network === true) ignored.push("imports.network")
    delete imports.network
  }
  for (const key of FILE_LIMITS) {
    const [group, name] = key.split(".")
    const at = settings[group]
    const limit = defaultOf(key)
    if (!isObject(at) || typeof at[name] !== "number" || typeof limit !== "number") continue
    const n = at[name] as number
    if (n <= 0 || n > limit) {
      at[name] = limit
      ignored.push(key)
    }
  }
  return { settings, ignored }
}

/**
 * `over` (the user's settings) layered on `under` (a settings file's), as the
 * native server applies its layers in turn: objects merge key by key, the
 * lint lists add up, and `prefixes` set in `over` replaces the whole map.
 */
export function layered(under: Record<string, unknown>, over: Record<string, unknown>): Record<string, unknown> {
  const out: Record<string, unknown> = { ...under }
  for (const [k, v] of Object.entries(over)) {
    const below = out[k]
    if (k !== "prefixes" && isObject(v) && isObject(below)) out[k] = layered(below, v)
    else if (Array.isArray(v) && Array.isArray(below)) out[k] = [...new Set([...below, ...v])]
    else out[k] = v
  }
  return out
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
