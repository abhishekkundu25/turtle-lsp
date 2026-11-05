// The server derives a file's kind, folder and `prefixes.ttl` scope from
// `file:` URIs, and the browser's workspace folders are not `file:` URIs
// (`vscode-vfs://github/owner/repo`, `vscode-test-web://mount`). Folder n is
// `file:///workspace/<n>` to the server: every URI of a message going in is
// mapped to it, and back coming out. Untitled documents and URIs outside the
// folders pass through.

/** Maps whole protocol messages; neither changes its argument. */
export interface UriMap {
  /** A message from the client, as the server sees it. */
  toServer<T>(message: T): T
  /** A message from the server, as the client sees it. */
  toClient<T>(message: T): T
}

const SERVER_ROOT = "file:///workspace/"

/** What workspace folder `n` is to the server. */
export const workspaceUri = (n: number): string => `${SERVER_ROOT}${n}`

// Strings that are document content, not URIs: an IRI a user wrote in a file
// stays as written even when it starts with a folder's URI.
const CONTENT = new Set(["text", "newText", "newName", "insertText"])

// Strings the server writes for people, where it puts file URIs among other
// words: a diagnostic's, an error's or a shown message's `message`, an inlay
// hint's `tooltip`, and a MarkupContent's `value` (a hover's "Described in"
// link). Server URIs are replaced anywhere in these; any other string is
// mapped only when it is a URI as a whole.
const PROSE = new Set(["message", "tooltip"])
const MARKUP = new Set(["markdown", "plaintext"])

// A SPARQL results term that is data, not a URI (`turtle/query`'s bindings
// and triples): its value stays as the store holds it.
const DATA_TERM = new Set(["literal", "bnode"])

// Percent escapes compare equal in either letter case (`%3A`, `%3a`).
const canon = (s: string): string => s.replace(/%[0-9a-f]{2}/gi, (m) => m.toUpperCase())

type Pair = { from: string; key: string; to: string }

// `s` with its prefix `from` (the whole of it, or up to a `/`) replaced.
function swap(pairs: Pair[], s: string): string {
  for (const { from, key, to } of pairs) {
    if (s.length < from.length) continue
    const next = s[from.length]
    if ((next === undefined || next === "/") && canon(s.slice(0, from.length)) === key) return to + s.slice(from.length)
  }
  return s
}

// `s` with every server URI in it replaced, wherever it stands. The digit
// check keeps `file:///workspace/1` from matching `…/10`.
function swapAll(pairs: Pair[], s: string): string {
  if (!s.includes(SERVER_ROOT)) return s
  for (const { from, to } of pairs) s = s.replace(new RegExp(`${from.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&")}(?![0-9])`, "g"), to)
  return s
}

// `v` with its URIs mapped; `prose` (outward only) maps inside prose strings.
function walk(pairs: Pair[], v: unknown, prose: boolean): unknown {
  if (typeof v === "string") return swap(pairs, v)
  if (Array.isArray(v)) return v.map((x) => walk(pairs, x, prose))
  if (v === null || typeof v !== "object") return v
  const o = v as Record<string, unknown>
  if (DATA_TERM.has(o.type as string)) return v
  const markup = MARKUP.has(o.kind as string)
  const out: Record<string, unknown> = {}
  for (const [k, x] of Object.entries(o)) {
    const text = prose && typeof x === "string" && (PROSE.has(k) || (markup && k === "value"))
    out[swap(pairs, k)] = CONTENT.has(k) ? x : text ? swapAll(pairs, x as string) : walk(pairs, x, prose)
  }
  return out
}

const pair = (from: string, to: string): Pair => ({ from, key: canon(from), to })

/** The map for the workspace folders' URIs, in order (folder n is `file:///workspace/<n>`). */
export function uriMap(folders: readonly string[]): UriMap {
  const roots = folders.map((f) => f.replace(/\/+$/, ""))
  // A folder inside another one wins over it.
  const inward = roots.map((f, n) => pair(f, workspaceUri(n))).sort((a, b) => b.from.length - a.from.length)
  const outward = roots.map((f, n) => pair(workspaceUri(n), f))
  return {
    toServer: <T>(m: T) => walk(inward, m, false) as T,
    toClient: <T>(m: T) => walk(outward, m, true) as T,
  }
}
