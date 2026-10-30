// What every `turtle/schema` answer keeps, whatever the workspace: read by
// the real-data tests (test/schema-corpus.test.mjs,
// test/schema-real.test.mjs). tests/chaos.rs checks the same on random input.
import assert from "node:assert/strict"
import fs from "node:fs"
import { fileURLToPath } from "node:url"

// The language id a client opens a file of this extension with.
export function languageOf(file) {
  const ext = file.slice(file.lastIndexOf(".") + 1).toLowerCase()
  return { ttl: "turtle", trig: "trig", nt: "ntriples", nq: "nquads", jsonld: "jsonld" }[ext] ?? "turtle"
}

// The answer to `params` well formed: indexes in range, the caps kept,
// used <= of == instances, and `problems` the rows that are not fine (state
// not ok, or instances missing), the rows cut counted in (the worst rows
// are kept, so a fine row kept means none was cut).
export function sound(r, params = {}) {
  const at = (m) => `${m}\n  params: ${JSON.stringify(params)}`
  const maxClasses = params.maxClasses ?? 300
  const maxRows = params.maxRows ?? 64
  const n = r.classes.length
  assert.ok(n <= maxClasses && n <= r.total.classes, at(`${n} classes, total ${r.total.classes}`))
  assert.equal(typeof r.imported, "number")
  const bad = (row) => row.state !== "ok" || (row.missing ?? 0) > 0
  for (const c of r.classes) {
    assert.ok(["class", "imported", "undefined"].includes(c.kind), at(`${c.id}: kind ${c.kind}`))
    for (const p of c.parents) assert.ok(p >= 0 && p < n, at(`${c.id}: parent ${p} of ${n}`))
    assert.ok(c.rows.length <= maxRows, at(`${c.id}: ${c.rows.length} rows`))
    if (c.moreRows > 0) assert.equal(c.rows.length, maxRows, at(`${c.id}: rows cut below the cap`))
    for (const row of c.rows) {
      assert.ok(row.used <= row.of && row.of === c.instances, at(`${c.id} ${row.property}: ${row.used}/${row.of} of ${c.instances}`))
      if (row.target !== undefined) assert.ok(row.target >= 0 && row.target < n, at(`${c.id} ${row.property}: target ${row.target}`))
    }
    const shown = c.rows.filter(bad).length
    const cutFine = c.rows.some((row) => !bad(row) && row.property !== params.focus)
    const exact = c.moreRows === 0 || cutFine
    assert.ok(
      c.problems >= shown && c.problems <= shown + c.moreRows && (!exact || c.problems === shown),
      at(`${c.id}: ${c.problems} problems, ${shown} rows not fine, ${c.moreRows} cut`),
    )
  }
  if (r.focus !== undefined) {
    assert.ok(r.focus >= 0 && r.focus < n, at(`focus ${r.focus} of ${n}`))
    if (r.focusRow !== undefined) assert.ok(r.focusRow < r.classes[r.focus].rows.length, at(`focus row ${r.focusRow}`))
  } else {
    assert.equal(r.focusRow, undefined, at("a focus row without a focus"))
  }
}

// Every range the answer names (where a class is defined, where a row is
// declared) inside the file it names: `textOf(uri)` gives the text the
// server read, else the file is read from disk.
export function rangesInside(r, textOf = () => undefined) {
  const lines = new Map()
  const linesOf = (uri) => {
    if (!lines.has(uri)) {
      const text = textOf(uri) ?? fs.readFileSync(fileURLToPath(uri), "utf8")
      lines.set(uri, text.split(/\r\n|\r|\n/))
    }
    return lines.get(uri)
  }
  const inside = (uri, range, what) => {
    const ls = linesOf(uri)
    const ok = (p) => p.line < ls.length && p.character <= ls[p.line].length
    const { start, end } = range
    const ordered = start.line < end.line || (start.line === end.line && start.character <= end.character)
    assert.ok(ok(start) && ok(end) && ordered, `${what}: ${JSON.stringify(range)} outside ${uri} (${ls.length} lines)`)
  }
  for (const c of r.classes) {
    assert.equal(c.uri === undefined, c.range === undefined, `${c.id}: uri without range`)
    if (c.uri) inside(c.uri, c.range, c.id)
    for (const row of c.rows) for (const d of row.declared) inside(d.uri, d.range, `${c.id} ${row.property}`)
  }
}

// `turtle/schema` with `params`, asked twice: the same answer both times,
// each within `budgetMs`. Resolves to the answer and the slower time.
export async function ask(s, params, budgetMs) {
  const once = async () => {
    const t0 = performance.now()
    const r = await s.request("turtle/schema", params)
    const ms = performance.now() - t0
    assert.ok(ms < budgetMs, `turtle/schema ${JSON.stringify(params)} took ${ms.toFixed(0)} ms (budget ${budgetMs})`)
    return [r, ms]
  }
  const [r, a] = await once()
  const [again, b] = await once()
  assert.equal(JSON.stringify(again), JSON.stringify(r), `turtle/schema ${JSON.stringify(params)} answered differently twice`)
  return { r, ms: Math.max(a, b) }
}

// Every check over one workspace: no params, small caps, a focus on its
// first class and `at` on a few positions of `opened` (a document the
// server has open, with its text). Resolves to the numbers to report.
export async function checkWorkspace(s, { budgetMs, textOf, opened, expectUri } = {}) {
  const { r, ms } = await ask(s, undefined, budgetMs)
  sound(r)
  rangesInside(r, textOf)
  if (expectUri) for (const c of r.classes) if (c.uri) assert.equal(c.uri, expectUri, `${c.id} defined outside the file`)
  for (const params of [{ maxClasses: 5 }, { maxRows: 3 }, { maxClasses: 5, maxRows: 3, imports: true }]) {
    const { r } = await ask(s, params, budgetMs)
    sound(r, params)
    rangesInside(r, textOf)
  }
  if (r.classes.length > 0) {
    const params = { focus: r.classes[0].id, maxClasses: 5 }
    const { r: f } = await ask(s, params, budgetMs)
    sound(f, params)
    assert.ok(f.focus !== undefined && f.classes[f.focus].id === params.focus, `focus ${params.focus} not kept`)
    const rows = r.classes.find((c) => c.rows.length > 0)
    if (rows) {
      const p = { focus: rows.rows[0].property, maxRows: 3 }
      sound((await ask(s, p, budgetMs)).r, p)
    }
  }
  let ats = 0
  if (opened) {
    const lines = opened.text.split(/\r\n|\r|\n/)
    const positions = [{ line: 0, character: 0 }]
    for (const frac of [0.25, 0.5, 0.9]) {
      const line = Math.floor((lines.length - 1) * frac)
      positions.push({ line, character: Math.floor(lines[line].length / 2) })
    }
    const own = r.classes.find((c) => c.uri === opened.uri)
    if (own) positions.push(own.range.start)
    for (const position of positions) {
      const params = { at: { uri: opened.uri, position }, maxClasses: 5 }
      const { r: a } = await ask(s, params, budgetMs)
      if (a === null) continue
      ats++
      sound(a, params)
      rangesInside(a, textOf)
    }
  }
  const rows = r.classes.reduce((sum, c) => sum + c.rows.length, 0)
  const problems = r.classes.reduce((sum, c) => sum + c.problems, 0)
  return { classes: r.total.classes, listed: r.classes.length, rows, problems, ats, ms }
}
