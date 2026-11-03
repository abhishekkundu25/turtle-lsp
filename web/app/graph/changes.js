// The Changes overlay: a `turtle/graphDiff` answer (docs/specs/graph-diff.md)
// read once into what the canvas draws (a status per node, the added and the
// removed triples, removed terms as ghosts beside their old neighbours) and
// the change list in the side panel (rows per file, then the terms by
// status). Rows are buttons whose status is in their text, not only their
// colour.

import { h } from "./el.js"
import { commonRoot, fileLabel, middleCut } from "./model.js"
import { truncate } from "./labels.js"

export const DIFF_METHOD = "turtle/graphDiff"
/** A node's status code on the canvas. */
export const NODE = { added: 1, changed: 2, impacted: 3 }
export const STATUSES = ["added", "removed", "changed", "impacted"]
const WORD = { added: "Added", removed: "Removed", changed: "Changed", impacted: "Impacted" }
const SIGN = { added: "+", removed: "−", changed: "~" }
const GHOSTS_MAX = 500
const FILE_CHARS = 36

const edgeKey = (s, p, o) => `${s}\u0001${p}\u0001${o}`
const isTerm = (t) => typeof t?.id === "string"
const validRow = (r) => SIGN[r?.change] && isTerm(r.subject) && isTerm(r.predicate) && isTerm(r.object) && (r.change !== "changed" || isTerm(r.was))
const termName = (t) => t?.name || t?.id || ""

/**
 * The answer as the view holds it.
 * @param {any} res a `turtle/graphDiff` answer
 */
export function readDiff(res) {
  const files = (Array.isArray(res?.files) ? res.files : [])
    .filter((f) => typeof f?.uri === "string")
    .map((f) => ({ uri: f.uri, rows: (Array.isArray(f.rows) ? f.rows : []).filter(validRow) }))
    .filter((f) => f.rows.length)
  const terms = (Array.isArray(res?.terms) ? res.terms : []).filter((t) => isTerm(t) && WORD[t.status])
  const t = res?.total ?? {}
  const num = (v) => (Number.isFinite(v) ? v : 0)
  const total = { triplesAdded: num(t.triplesAdded), triplesRemoved: num(t.triplesRemoved), added: num(t.added), removed: num(t.removed), changed: num(t.changed), impacted: num(t.impacted), cut: t.cut === true }
  const status = new Map() // node id -> NODE code
  for (const term of terms) if (term.status !== "removed") status.set(term.id, NODE[term.status])
  const added = new Set() // edge keys of the triples added
  const removed = [] // { subject, predicate, object } of the triples removed
  for (const f of files)
    for (const r of f.rows) {
      if (r.change !== "removed") {
        added.add(edgeKey(r.subject.id, r.predicate.id, r.object.id))
        if (r.object.literal) status.set(r.object.id, NODE.added)
      }
      if (r.change !== "added") removed.push({ subject: r.subject, predicate: r.predicate, object: r.change === "changed" ? r.was : r.object })
    }
  return { files, terms, total, status, added, removed }
}

/** Whether the answer has no change at all. */
export const isEmpty = (diff) => !diff.files.length && !diff.terms.length

/** The rows that name node `id` (subject, object or old object): `[{ f, r }]` indices into `diff.files`. */
export function rowsOf(diff, id) {
  const out = []
  diff.files.forEach((file, f) => file.rows.forEach((row, r) => {
    if (row.subject.id === id || row.object.id === id || row.was?.id === id) out.push({ f, r })
  }))
  return out
}

/** The term entry of node `id`, if the answer names it. */
export const termOf = (diff, id) => diff.terms.find((t) => t.id === id) ?? null

/** Where a row opens: the current triple, else (removed) where the base had it. */
export const rowRange = (row) => [row.range, row.baseRange].find((r) => Number.isInteger(r?.start?.line)) ?? null

/**
 * What the canvas draws for `diff` over the shown `scene` of `model`: each
 * node's status (`NODE`, 0 unchanged), the added triples among drawn nodes
 * (index pairs, flat), the removed ones (world coordinates x0 y0 x1 y1,
 * flat) and the ghosts they end at. A ghost is a removed term (or, with
 * literals shown, a removed literal) placed beside its drawn neighbours; one
 * with none drawn is left to the list.
 * @param {{ literals?: boolean, files?: string[] | null }} shown the filters: removed literals only with literals, removed terms only of the files shown
 */
export function overlay(diff, model, scene, { literals = false, files = null } = {}) {
  const drawn = (i) => i !== undefined && i >= 0 && Number.isFinite(scene.x[i])
  const status = new Uint8Array(model.n)
  for (let i = 0; i < model.n; i++) status[i] = diff.status.get(model.nodes[i].id) ?? 0
  const added = []
  for (const e of model.edges)
    if (drawn(e.source) && drawn(e.target) && diff.added.has(edgeKey(model.nodes[e.source].id, e.predicate, model.nodes[e.target].id))) added.push(e.source, e.target)

  const gone = new Set(diff.terms.filter((t) => t.status === "removed" && (!files || files.includes(t.uri))).map((t) => t.id))
  const ghosts = new Map() // key -> { text, x, y, placed, near: [] }
  const ends = []
  const end = (term, key) => {
    const i = model.index.get(term.id)
    if (drawn(i)) return i
    if (term.literal ? !literals : !gone.has(term.id)) return null
    let g = ghosts.get(key)
    if (!g) {
      if (ghosts.size >= GHOSTS_MAX) return null
      ghosts.set(key, (g = { text: truncate(termName(term)), key, x: 0, y: 0, placed: false, near: [] }))
    }
    return g
  }
  diff.removed.forEach((t, k) => {
    const a = end(t.subject, t.subject.id)
    const b = a == null ? null : end(t.object, t.object.literal ? `literal ${k}` : t.object.id)
    if (a == null || b == null) return
    if (typeof a === "object") a.near.push(b)
    if (typeof b === "object") b.near.push(a)
    ends.push(a, b)
  })
  // Each ghost at the middle of its placed neighbours, pushed out in a
  // direction of its own; a ghost next to ghosts only waits a pass for them.
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity
  for (const i of scene.nodes) {
    const x = scene.x[i], y = scene.y[i]
    if (!Number.isFinite(x)) continue
    x0 = Math.min(x0, x), x1 = Math.max(x1, x), y0 = Math.min(y0, y), y1 = Math.max(y1, y)
  }
  const unit = Math.max(12, 0.06 * Math.max(x1 - x0, y1 - y0, 0))
  const at = (n) => (typeof n === "number" ? [scene.x[n], scene.y[n]] : n.placed ? [n.x, n.y] : null)
  for (let pass = 0; pass < 3; pass++)
    for (const g of ghosts.values()) {
      if (g.placed) continue
      let sx = 0, sy = 0, c = 0
      for (const n of g.near) {
        const p = at(n)
        if (p) (sx += p[0]), (sy += p[1]), c++
      }
      if (!c) continue
      const a = angle(g.key)
      g.x = sx / c + Math.cos(a) * unit
      g.y = sy / c + Math.sin(a) * unit
      g.placed = true
    }
  const removed = []
  for (let q = 0; q < ends.length; q += 2) {
    const a = at(ends[q]), b = at(ends[q + 1])
    if (a && b) removed.push(a[0], a[1], b[0], b[1])
  }
  const placed = [...ghosts.values()].filter((g) => g.placed)
  return { status, added, removed, ghosts: { x: placed.map((g) => g.x), y: placed.map((g) => g.y), text: placed.map((g) => g.text) } }
}

// A direction from a string, the same each time.
function angle(s) {
  let v = 2166136261
  for (let i = 0; i < s.length; i++) v = Math.imul(v ^ s.charCodeAt(i), 16777619)
  return ((v >>> 0) / 4294967296) * Math.PI * 2
}

// ---- the change list ----

/** A row's words: `+ s p o`, `− s p o`, `~ s p old → new`, and its line. */
export function rowText(row) {
  const s = termName(row.subject), p = termName(row.predicate), o = termName(row.object)
  const range = rowRange(row)
  const line = range ? `${row.change === "removed" ? "was line" : "line"} ${range.start.line + 1}` : ""
  const what = row.change === "changed" ? `${s} ${p} ${termName(row.was)} → ${o}` : `${s} ${p} ${o}`
  return { what, line, label: `${WORD[row.change]}: ${what}${line ? `, ${line}` : ""}` }
}

/** One row as a button: its sign, the triple (old → new for a change) and its line. `data-f`/`data-r` say which. */
export function rowButton(doc, diff, f, r) {
  const row = diff.files[f].rows[r]
  const { label } = rowText(row)
  const range = rowRange(row)
  const name = (t, cls) => h(doc, "span", { class: cls, text: termName(t) })
  return h(doc, "button", { type: "button", class: `graph-change graph-change-${row.change}`, "data-f": f, "data-r": r, "aria-label": label, title: label },
    h(doc, "span", { class: "graph-change-sign", "aria-hidden": "true", text: SIGN[row.change] }),
    h(doc, "span", { class: "graph-change-what" },
      name(row.subject, "graph-change-s"), " ", name(row.predicate, "graph-pred"), " ",
      row.change === "changed" ? [name(row.was, "graph-change-old"), " → ", name(row.object, "graph-change-new")] : name(row.object, "graph-obj")),
    range ? h(doc, "span", { class: "graph-change-line", "aria-hidden": "true", text: `${row.change === "removed" ? "was " : ""}:${range.start.line + 1}` }) : null)
}

/** The counts as one line of text. */
export function summary(total) {
  const f = (v) => v.toLocaleString("en")
  return `+${f(total.triplesAdded)} −${f(total.triplesRemoved)} triples · ${f(total.added)} added, ${f(total.removed)} removed, ${f(total.changed)} changed, ${f(total.impacted)} impacted`
}

/**
 * The change list in `el`.
 * @param {HTMLElement} el
 * @param {{ open(uri: string, range: object | null, id: string): void }} on a row or a term chosen
 */
export function createChangeList(el, on) {
  const doc = el.ownerDocument
  let diff = null

  el.addEventListener("click", (e) => {
    if (!diff) return
    const b = e.target.closest("button[data-f], button[data-t]")
    if (!b) return
    if (b.dataset.t != null) {
      const t = diff.terms[Number(b.dataset.t)]
      if (t) on.open(t.uri, t.range ?? null, t.id)
      return
    }
    const file = diff.files[Number(b.dataset.f)]
    const row = file?.rows[Number(b.dataset.r)]
    if (row) on.open(file.uri, rowRange(row), row.subject.id)
  })

  return {
    /** Show `next` (a readDiff answer, or null while there is none) and `note` (a reason, an error or "loading"). */
    show(next, note = "") {
      diff = next
      const active = doc.activeElement && el.contains(doc.activeElement) ? doc.activeElement : null
      const key = active ? [active.dataset.f, active.dataset.r, active.dataset.t].join(":") : null
      const parts = [h(doc, "h3", { class: "graph-changes-hd", text: "Changes" })]
      if (note) parts.push(h(doc, "p", { class: "graph-none graph-changes-note", text: note }))
      if (diff) {
        parts.push(h(doc, "p", { class: "graph-changes-sum", text: summary(diff.total) }))
        if (diff.total.cut) parts.push(h(doc, "p", { class: "graph-none graph-changes-cut", text: "Cut at the limit: more changed than is listed here." }))
        if (isEmpty(diff)) parts.push(h(doc, "p", { class: "graph-none", text: "No changes since the base." }))
        const root = commonRoot(diff.files.map((f) => f.uri))
        diff.files.forEach((file, f) => {
          const plus = file.rows.filter((r) => r.change !== "removed").length
          const minus = file.rows.filter((r) => r.change !== "added").length
          parts.push(
            h(doc, "h4", { class: "graph-changes-file", title: file.uri }, h(doc, "span", { class: "graph-check-l", text: middleCut(fileLabel(file.uri, root), FILE_CHARS) }), h(doc, "span", { class: "graph-count", text: `+${plus} −${minus}` })),
            h(doc, "ul", { class: "graph-triples", "aria-label": `Changes in ${fileLabel(file.uri, root)}` }, file.rows.map((_, r) => h(doc, "li", {}, rowButton(doc, diff, f, r)))))
        })
        for (const s of STATUSES) {
          const list = diff.terms.map((t, k) => [t, k]).filter(([t]) => t.status === s)
          if (!list.length) continue
          parts.push(
            h(doc, "h4", { class: "graph-info-sec", text: `${WORD[s]} terms (${list.length})` }),
            h(doc, "ul", { class: "graph-triples", "aria-label": `${WORD[s]} terms` }, list.map(([t, k]) => h(doc, "li", {},
              h(doc, "button", { type: "button", class: `graph-change graph-change-${s}`, "data-t": k, title: `${WORD[s]}: ${t.id}`, "aria-label": `${WORD[s]} term: ${termName(t)}` },
                h(doc, "span", { class: "graph-change-sign graph-change-word", "aria-hidden": "true", text: s }),
                h(doc, "span", { class: "graph-change-what", text: termName(t) }))))))
        }
      }
      el.replaceChildren(...parts)
      if (key) [...el.querySelectorAll("button")].find((b) => [b.dataset.f, b.dataset.r, b.dataset.t].join(":") === key)?.focus()
    },
  }
}
