// Helpers shared by the SPARQL tests: a server on test/fixtures/query and
// documents opened with an explicit language id.
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures, positionOf } from "./lsp-client.mjs"

export const dir = path.join(fixtures, "query")
export const read = (f) => fs.readFileSync(path.join(dir, f), "utf8")
export const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.code} ${d.message}`).join("\n")
export const problems = (diags) => diags.filter((d) => d.severity <= 2)
export const ofCode = (diags, code) => diags.filter((d) => d.code === code)
export const where = (locs) => locs.map((l) => `${path.basename(l.uri)}:${l.range.start.line}`).sort()
export const labelsOf = (res) => (Array.isArray(res) ? res : (res?.items ?? [])).map((i) => i.label)
export const itemsOf = (res) => (Array.isArray(res) ? res : (res?.items ?? []))
export { positionOf, uriOf }

const languageOf = (name) =>
  name.endsWith(".rq") || name.endsWith(".sparql") || name.endsWith(".ru")
    ? "sparql"
    : name.endsWith(".nq")
      ? "nquads"
      : name.endsWith(".trig")
        ? "trig"
        : "turtle"

// Apply LSP edits (UTF-16 positions, ASCII test texts) to a text.
export function apply(text, edits) {
  const lines = text.split("\n")
  const offset = ({ line, character }) => {
    let o = 0
    for (let i = 0; i < line && i < lines.length; i++) o += lines[i].length + 1
    return Math.min(o + character, text.length)
  }
  const sorted = [...edits].sort((a, b) => offset(b.range.start) - offset(a.range.start))
  for (const e of sorted) text = text.slice(0, offset(e.range.start)) + e.newText + text.slice(offset(e.range.end))
  return text
}

// The text of `range` in `text`.
export function slice(text, range) {
  const lines = text.split("\n")
  if (range.start.line === range.end.line) return lines[range.start.line].slice(range.start.character, range.end.character)
  return [
    lines[range.start.line].slice(range.start.character),
    ...lines.slice(range.start.line + 1, range.end.line),
    lines[range.end.line].slice(0, range.end.character),
  ].join("\n")
}

export async function queryServer(options = {}) {
  const s = await startServer({ cwd: dir, ...options })
  // Requests wait for the workspace index; diagnostics after this include
  // the workspace lints.
  await s.request("workspace/symbol", { query: "" })
  let n = 0
  const api = {
    s,
    // Opens `name` (relative to the fixture directory) with `text` (default:
    // the file's content) and waits for its diagnostics.
    async open(name, text = read(name), languageId = languageOf(name)) {
      const uri = uriOf(path.join(dir, name))
      s.open(uri, text, languageId)
      return { uri, text, diags: await s.diagnosticsFor(uri) }
    },
    // Opens a fresh scratch document with this text.
    scratch(text, ext = "rq", languageId) {
      return api.open(`__scratch-${n++}.${ext}`, text, languageId)
    },
    // Replaces the whole text (versions continue from the open's 1).
    async change(uri, text) {
      s.change(uri, text)
      return s.diagnosticsFor(uri)
    },
    close: (uri) => s.close(uri),
    async complete(text, needle = text, ext = "rq") {
      const { uri } = await api.scratch(text, ext)
      try {
        return await s.request("textDocument/completion", { textDocument: { uri }, position: positionOf(text, needle) })
      } finally {
        s.close(uri)
      }
    },
    // Runs `fn(doc)` on a scratch document, closing it afterwards.
    async withScratch(text, fn, ext = "rq", languageId) {
      const doc = await api.scratch(text, ext, languageId)
      try {
        return await fn(doc)
      } finally {
        s.close(doc.uri)
      }
    },
    async withFile(name, fn, text) {
      const doc = await api.open(name, text)
      try {
        return await fn(doc)
      } finally {
        s.close(doc.uri)
      }
    },
    stop: () => s.stop(),
  }
  return api
}
