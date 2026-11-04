// Which server binary the desktop extension runs. It does not import
// `vscode`, so Node tests it directly (test/find-server.test.mjs).
import * as nodePath from "node:path"

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
