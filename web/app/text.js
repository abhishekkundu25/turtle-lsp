// Positions in text. LSP counts lines from 0 and characters in UTF-16 units,
// as JavaScript strings do; Monaco counts both from 1.

/** The offset where each line of `text` starts (line 0 starts at 0). */
export function lineStarts(text) {
  const starts = [0]
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1)
  return starts
}

/** The offset of LSP position `p` in `text`, clamped to the text. */
export const offsetOf = (starts, text, p) => Math.min(text.length, (starts[p.line] ?? text.length) + p.character)

/** The LSP position of `offset` (binary search over `starts`). */
export function positionAt(starts, offset) {
  let lo = 0
  let hi = starts.length - 1
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1
    if (starts[mid] <= offset) lo = mid
    else hi = mid - 1
  }
  return { line: lo, character: offset - starts[lo] }
}

/** Line `line` of `text` without its line break (`starts`: its `lineStarts`). */
export function lineAt(text, starts, line) {
  if (line < 0 || line >= starts.length) return ""
  const end = line + 1 < starts.length ? starts[line + 1] - 1 : text.length
  const s = text.slice(starts[line], end)
  return s.endsWith("\r") ? s.slice(0, -1) : s
}

/**
 * `text` with LSP text edits applied (all positions refer to the original).
 * Edits that start at one position apply in the order of the array, as LSP
 * says: the result is built from the front, in (start, array index) order.
 */
export function applyEdits(text, edits) {
  const starts = lineStarts(text)
  const spans = edits.map((e, i) => [offsetOf(starts, text, e.range.start), offsetOf(starts, text, e.range.end), e.newText, i])
  spans.sort((a, b) => a[0] - b[0] || a[3] - b[3])
  let out = ""
  let from = 0
  for (const [s, e, t] of spans) {
    out += text.slice(from, s) + t
    from = Math.max(from, e)
  }
  return out + text.slice(from)
}

/** An LSP range as a Monaco range. */
export const toMonacoRange = (monaco, r) => new monaco.Range(r.start.line + 1, r.start.character + 1, r.end.line + 1, r.end.character + 1)

/** A Monaco range as an LSP range. */
export const fromMonacoRange = (r) => ({
  start: { line: r.startLineNumber - 1, character: r.startColumn - 1 },
  end: { line: r.endLineNumber - 1, character: r.endColumn - 1 },
})
