// The base for the graph view's "changes since HEAD" mode: the workspace's RDF
// files that differ from git HEAD, with their HEAD text. The git extension's
// API is passed in (it is external data), so tests fake it and this file does
// not import `vscode`.

/** The parts of the built-in git extension's API (`getAPI(1)`) that are used. */
export interface GitApi {
  repositories: GitRepository[]
}
export interface GitRepository {
  rootUri: GitUri
  state: { workingTreeChanges: GitChange[]; indexChanges: GitChange[]; untrackedChanges: GitChange[] }
  show(ref: string, path: string): Promise<string>
}
export interface GitChange {
  uri: GitUri
  originalUri: GitUri
  status: number
}
export interface GitUri {
  fsPath: string
  toString(): string
}

/** What `turtle/graphDiff` takes: `text: null` is a file that is not in HEAD. */
export type BaseFile = { uri: string; text: string | null }

// The git extension's `Status` enum values.
const INDEX_ADDED = 1
const INDEX_RENAMED = 3
const INDEX_COPIED = 4
const UNTRACKED = 7
const IGNORED = 8
const INTENT_TO_ADD = 9
const INTENT_TO_RENAME = 10
/** Not in HEAD under this path. */
const NEW_PATH = new Set([INDEX_ADDED, INDEX_COPIED, UNTRACKED, INTENT_TO_ADD])
/** Created under `uri`; `originalUri` is the HEAD path, now deleted. */
const RENAMED = new Set([INDEX_RENAMED, INTENT_TO_RENAME])

/** Extensions of the files the server reads as RDF (`src/workspace/kinds.rs`), matched in any case. */
const RDF = /\.(ttl|turtle|trig|nt|nq|jsonld)$/i

const slashes = (p: string) => p.replace(/\\/g, "/")
const inside = (path: string, roots: string[]) => roots.some((r) => slashes(path).startsWith(slashes(r).replace(/\/$/, "") + "/"))

/**
 * The files under `roots` (workspace folder paths) whose RDF content differs from HEAD, or the reason
 * nothing can be compared. A rename is a deleted file plus a created one.
 */
export async function gitBase(api: GitApi | undefined, roots: string[]): Promise<BaseFile[] | string> {
  if (!api) return "The git extension is missing or turned off."
  const repos = api.repositories.filter((r) => roots.some((root) => inside(r.rootUri.fsPath + "/x", [root]) || inside(root + "/x", [r.rootUri.fsPath])))
  if (!repos.length) return "No git repository covers this workspace."
  const files = new Map<string, Promise<string | null>>()
  const add = (uri: GitUri, text: () => Promise<string | null>) => {
    const key = uri.toString()
    if (RDF.test(uri.fsPath) && inside(uri.fsPath, roots) && !files.has(key)) files.set(key, text())
  }
  for (const repo of repos) {
    const { workingTreeChanges, indexChanges, untrackedChanges } = repo.state
    for (const c of [...indexChanges, ...workingTreeChanges, ...untrackedChanges]) {
      if (c.status === IGNORED) continue
      const head = () => repo.show("HEAD", c.originalUri.fsPath)
      if (NEW_PATH.has(c.status)) add(c.uri, async () => null)
      else if (RENAMED.has(c.status)) {
        add(c.originalUri, head)
        add(c.uri, async () => null)
      } else add(c.uri, head) // modified, deleted, type-changed: HEAD has the file at originalUri
    }
  }
  try {
    return await Promise.all([...files].map(async ([uri, text]) => ({ uri, text: await text })))
  } catch (err) {
    return `Git could not give the HEAD version of a file: ${err instanceof Error ? err.message : String(err)}`
  }
}
