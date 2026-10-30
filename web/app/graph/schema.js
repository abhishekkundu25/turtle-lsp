// Schema mode: the classes of a `turtle/schema` answer as boxes, a row per
// property, on a tidy top-down rdfs:subClassOf layout. DOM, one element per
// box with its rows as children; one SVG under them for the subclass lines
// and the one arrow a hovered link chip draws. Pan and zoom move one
// transformed layer; under zoom FAR the boxes fold to their header. A
// class's problems are derived here from its rows (problemsOf), for its box
// and for the inspector (panel.js). An answer lists the places of one
// class's problems (`examples`, the focus's): needsPlaces says a class has
// problems with none listed, mergePlaces adds those of a later answer.
// A name shown is displayName's: the prefixed name, else the IRI's local
// name; the full IRI is the element's title. schemaModel, displayName,
// rowBadge, problemsOf, needsPlaces, mergePlaces and tidyLayout are pure, so
// the tests run them in Node.

import { h } from "./el.js"
import { commonRoot, fileLabel, middleCut } from "./model.js"

export const SCHEMA_METHOD = "turtle/schema"
/** Under this zoom the boxes show their header only. */
export const FAR = 0.5
/** The first view fits the whole model when that keeps the zoom at this or above. */
export const READABLE = 0.6
/** Else it centres the focus class at this zoom. */
export const FOCUS_ZOOM = 0.9
/** Fit never zooms out further: the boxes folded to their header. */
export const FIT_FLOOR = FAR * 0.99
// A box's width in px: .schema-box in graph.css has the same.
const W = 264
const GAP_X = 28
const GAP_Y = 56
const ZOOM = [0.05, 2]
const FILE_CHARS = 26
const STATES = ["ok", "unused", "undeclared", "badValues", "conflict"]
/** A row's state in words: what a screen reader says, what the legend explains. */
export const STATE_TEXT = { ok: "declared and used", unused: "never used", undeclared: "not declared", badValues: "bad values", conflict: "range and shape disagree" }
export const MARK = { ok: "✓", unused: "○", undeclared: "?", badValues: "⚠", conflict: "✕" }
const SEV_RANK = { error: 0, warning: 1, "": 2 }
// The examples' kind for each problem that has them.
const EXAMPLE_KIND = { missing: "missing", badValues: "badValue", undeclared: "undeclared", conflict: "shapeValue" }
const EXAMPLE_KINDS = Object.values(EXAMPLE_KIND)
const SVG = "http://www.w3.org/2000/svg"

// One formatter: toLocaleString makes one per call, thousands per answer.
const fmt = new Intl.NumberFormat("en").format
const count = (v) => (Number.isFinite(v) && v > 0 ? Math.floor(v) : 0)
const plural = (n, one, many = `${one}s`) => `${fmt(n)} ${n === 1 ? one : many}`

/**
 * The name to show for a term: `name` (a prefixed name, as the server
 * writes it) when it is one, else the local name of `iri` (or of `name`'s
 * `<iri>`): what follows the last `#` or `/`. The full IRI goes in a title.
 */
export function displayName(name, iri = "") {
  const n = typeof name === "string" ? name : ""
  if (n && !n.startsWith("<")) return n
  const full = (typeof iri === "string" && iri) || n.replace(/^<|>$/g, "")
  const trimmed = full.replace(/[#/]+$/, "")
  const local = trimmed.slice(Math.max(trimmed.lastIndexOf("#"), trimmed.lastIndexOf("/")) + 1)
  return local || full
}

/** A shown name split at its prefix: ["ex:", "Pump"], or ["", "Pump"] for a local name. */
export function nameParts(name) {
  const at = name.indexOf(":")
  return at > 0 && !name.includes("/") ? [name.slice(0, at + 1), name.slice(at + 1)] : ["", name]
}
const short = (name) => nameParts(name)[1]

/**
 * The answer checked once: indexes in range, unknown states as ok, counts as
 * numbers, each class's problems derived from its rows.
 * @param {any} res a `turtle/schema` answer
 */
export function schemaModel(res) {
  const raw = Array.isArray(res?.classes) ? res.classes.filter((c) => c && typeof c === "object") : []
  const n = raw.length
  const ok = (v) => Number.isInteger(v) && v >= 0 && v < n
  const classes = raw.map((c, i) => {
    const rows = (Array.isArray(c.rows) ? c.rows : [])
      .filter((r) => r && typeof r.property === "string")
      .map((r) => ({
        property: r.property,
        name: displayName(r.name, r.property),
        link: r.link === true,
        target: ok(r.target) ? r.target : -1,
        range: typeof r.range === "string" ? r.range : "",
        rangeName: r.rangeName || r.range ? displayName(r.rangeName, r.range) : "",
        declared: (Array.isArray(r.declared) ? r.declared : []).filter((d) => d && typeof d.uri === "string"),
        used: count(r.used),
        of: count(r.of),
        state: STATES.includes(r.state) ? r.state : "ok",
        badValues: count(r.badValues),
        required: r.required && Number.isFinite(r.required.min) ? { min: r.required.min, max: Number.isFinite(r.required.max) ? r.required.max : undefined } : null,
        missing: count(r.missing),
        conflict: r.conflict && typeof r.conflict === "object" ? { declared: displayName(String(r.conflict.declared ?? "")), shape: displayName(String(r.conflict.shape ?? "")) } : null,
        shapeBadValues: count(r.shapeBadValues),
        declaredFor: (Array.isArray(r.declaredFor) ? r.declaredFor : []).filter((d) => typeof d === "string" && d).map((d) => displayName(d)),
        examples: (Array.isArray(r.examples) ? r.examples : [])
          .filter((e) => e && typeof e.uri === "string" && e.range?.start && EXAMPLE_KINDS.includes(e.kind))
          .map((e) => ({ kind: e.kind, uri: e.uri, range: e.range, name: displayName(String(e.name ?? "")) })),
      }))
    const id = typeof c.id === "string" ? c.id : String(i)
    const name = displayName(c.name, id)
    const label = typeof c.label === "string" ? c.label : ""
    const problems = problemsOf(rows, label || short(name))
    return {
      id,
      name,
      label,
      uri: typeof c.uri === "string" ? c.uri : "",
      range: c.range && c.range.start ? c.range : null,
      kind: ["class", "imported", "undefined"].includes(c.kind) ? c.kind : "class",
      parents: [...new Set((Array.isArray(c.parents) ? c.parents : []).filter((p) => ok(p) && p !== i))],
      instances: count(c.instances),
      rows,
      moreRows: count(c.moreRows),
      problems,
      // The answer's count also covers rows past the per-class cap.
      problemCount: Math.max(count(c.problems), problems.length),
      worst: problems.length ? problems[0].sev : "",
    }
  })
  const index = new Map(classes.map((c, i) => [c.id, i]))
  const focus = ok(res?.focus) ? res.focus : -1
  const focusRow = focus >= 0 && Number.isInteger(res?.focusRow) && res.focusRow >= 0 && res.focusRow < classes[focus].rows.length ? res.focusRow : -1
  return { classes, index, focus, focusRow, total: Math.max(count(res?.total?.classes), n), imported: count(res?.imported) }
}

/**
 * The one badge a row shows, the worst first: a conflict, then instances
 * missing a required property, then bad values, then never used or not declared.
 * `sev`: "error" or "warning" (red, amber), "" for neither.
 */
export function rowBadge(r) {
  if (r.state === "conflict") return { kind: "conflict", sev: "error", text: "range ≠ shape" }
  if (r.missing > 0) return { kind: "missing", sev: "error", text: `${fmt(r.missing)} missing` }
  if (r.state === "badValues") return { kind: "badValues", sev: "warning", text: plural(r.badValues, "bad value") }
  if (r.state === "unused") return { kind: "unused", sev: "", text: "never used" }
  if (r.state === "undeclared") return { kind: "undeclared", sev: "", text: "not declared" }
  return null
}

/**
 * A class's problems: one per row whose state is not ok and one per row with
 * instances missing it, errors first. `examples`: the places the answer lists
 * for it (missing, bad values, not declared, values outside a conflicting
 * shape), of `count` in all. `at`: where to fix it: the first example, else
 * the row's first declaration, else null. `cls`: the class's name, for "declared for X, not cls".
 */
export function problemsOf(rows, cls = "this class") {
  const out = []
  const push = (r, p) => {
    const kind = EXAMPLE_KIND[p.kind]
    const examples = kind ? (r.examples ?? []).filter((e) => e.kind === kind) : []
    const count = p.kind === "missing" ? r.missing : p.kind === "badValues" ? r.badValues : p.kind === "undeclared" ? r.used : p.kind === "conflict" ? r.shapeBadValues ?? 0 : 0
    out.push({ ...p, examples, count: Math.max(count, examples.length), at: examples[0] ?? p.at })
  }
  rows.forEach((r, row) => {
    const at = r.declared[0] ?? null
    const range = r.rangeName ? ` ${r.rangeName}` : ""
    if (r.state === "conflict") {
      const outside = r.shapeBadValues ? `${plural(r.shapeBadValues, "value")} outside the shape` : ""
      push(r, { row, kind: "conflict", sev: "error", at, text: `${r.name}: range and shape disagree`, detail: [r.conflict ? `rdfs:range ${r.conflict.declared}, shape ${r.conflict.shape}` : "", outside].filter(Boolean).join(" · ") })
    }
    else if (r.state === "badValues") push(r, { row, kind: "badValues", sev: "warning", at, text: `${plural(r.badValues, "value")} of ${r.name} not${range}`, detail: `Declared${range}` })
    else if (r.state === "unused") push(r, { row, kind: "unused", sev: "", at, text: `${r.name} is never used`, detail: `None of ${plural(r.of, "instance")} has it` })
    else if (r.state === "undeclared") push(r, { row, kind: "undeclared", sev: "", at: null, text: `${r.name} is used but not declared for this class`, detail: `${declaredForText(r, cls)}; used by ${fmt(r.used)} of ${plural(r.of, "instance")}` })
    if (r.missing > 0) push(r, { row, kind: "missing", sev: "error", at, text: `${plural(r.missing, "instance")} miss ${r.name}`, detail: `Required: sh:minCount ${r.required?.min ?? 1}` })
  })
  return out.sort((a, b) => SEV_RANK[a.sev] - SEV_RANK[b.sev])
}

/** Where a not-declared row's property is declared: "Declared for Pipe, not Gasket", or "Declared for no class". */
export function declaredForText(r, cls = "this class") {
  const by = r.declaredFor ?? []
  return by.length ? `Declared for ${by.map(short).join(", ")}, not ${cls}` : "Declared for no class"
}

/** A class's name in text: its label, else its shown name without the prefix. */
export const classText = (c) => c.label || short(c.name)

/**
 * Class `c` (of schemaModel) has problems that have places (missing, bad
 * values, not declared) and its rows list none: an answer lists the places
 * of one class only (the focus's, or the class `examples` names).
 */
export function needsPlaces(c) {
  return Boolean(c) && !c.rows.some((r) => r.examples.length) && c.problems.some((p) => EXAMPLE_KIND[p.kind] && p.count > 0)
}

/**
 * Put the places a later answer `res` lists for class `i` of `model` into
 * its rows (by property) and derive its problems again; every other field
 * and class stays. false when `res` does not list the class.
 */
export function mergePlaces(model, i, res) {
  const c = model.classes[i]
  const fresh = c ? schemaModel(res) : null
  const j = fresh?.index.get(c.id)
  if (j == null) return false
  const places = new Map(fresh.classes[j].rows.map((r) => [r.property, r.examples]))
  for (const r of c.rows) if (places.get(r.property)?.length) r.examples = places.get(r.property)
  c.problems = problemsOf(c.rows, classText(c))
  return true
}

/**
 * A tidy top-down layout of boxes `W` wide: each box under its first parent,
 * parents centred over their children, one row of boxes per depth as tall as
 * its tallest box. Trees side by side; boxes with no parent and no child in a
 * grid below them. A cycle is cut at its first box.
 * @param {number[][]} parents per box, indexes of its parents (the first places it)
 * @param {ArrayLike<number>} heights per box, px
 * @returns {{ x: Float64Array, y: Float64Array, width: number, height: number, links: { parent: number, child: number, primary: boolean }[], first: Int32Array }}
 */
export function tidyLayout(parents, heights, { width = W, gapX = GAP_X, gapY = GAP_Y } = {}) {
  const n = parents.length
  const valid = (p, i) => Number.isInteger(p) && p >= 0 && p < n && p !== i
  const first = new Int32Array(n).fill(-1)
  for (let i = 0; i < n; i++) first[i] = parents[i].find((p) => valid(p, i)) ?? -1
  // Cut cycles: walk each box's first parents; a walk that comes back to a
  // box on its own path cuts that box loose.
  const mark = new Uint8Array(n) // 0 new, 1 on the walk, 2 done
  for (let s = 0; s < n; s++) {
    const path = []
    let i = s
    while (i >= 0 && !mark[i]) (mark[i] = 1), path.push(i), (i = first[i])
    if (i >= 0 && mark[i] === 1) first[i] = -1
    for (const p of path) mark[p] = 2
  }
  const children = Array.from({ length: n }, () => [])
  for (let i = 0; i < n; i++) if (first[i] >= 0) children[first[i]].push(i)
  const depth = new Int32Array(n)
  const roots = []
  for (let i = 0; i < n; i++) if (first[i] < 0) roots.push(i)
  // Depths and each depth's height, breadth-first from the roots of trees.
  const levelH = []
  const trees = roots.filter((r) => children[r].length)
  const singles = roots.filter((r) => !children[r].length)
  const queue = [...trees]
  for (let q = 0; q < queue.length; q++) {
    const i = queue[q]
    levelH[depth[i]] = Math.max(levelH[depth[i]] ?? 0, heights[i] || 0)
    for (const c of children[i]) (depth[c] = depth[i] + 1), queue.push(c)
  }
  const levelY = [0]
  for (let d = 1; d <= levelH.length; d++) levelY[d] = levelY[d - 1] + levelH[d - 1] + gapY
  const x = new Float64Array(n)
  const y = new Float64Array(n)
  let cursor = 0
  // Post-order without recursion: leaves take the next slot, parents sit over their children.
  for (const r of trees) {
    const stack = [[r, 0]]
    while (stack.length) {
      const top = stack[stack.length - 1]
      const kids = children[top[0]]
      if (top[1] < kids.length) {
        stack.push([kids[top[1]++], 0])
        continue
      }
      stack.pop()
      const i = top[0]
      if (!kids.length) (x[i] = cursor), (cursor += width + gapX)
      else x[i] = (x[kids[0]] + x[kids[kids.length - 1]]) / 2
      y[i] = levelY[depth[i]]
    }
    cursor += gapX
  }
  let right = Math.max(0, cursor - 2 * gapX)
  let bottom = trees.length ? levelY[levelH.length] - gapY : 0
  if (singles.length) {
    const cols = Math.max(1, Math.min(singles.length, Math.max(Math.floor((right + gapX) / (width + gapX)), Math.ceil(Math.sqrt(singles.length * 1.5)))))
    let top = trees.length ? bottom + gapY * 1.5 : 0
    for (let k = 0; k < singles.length; k += cols) {
      let rowH = 0
      for (let c = 0; c < cols && k + c < singles.length; c++) {
        const i = singles[k + c]
        x[i] = c * (width + gapX)
        y[i] = top
        rowH = Math.max(rowH, heights[i] || 0)
      }
      top += rowH + gapY / 2
    }
    bottom = top - gapY / 2
    right = Math.max(right, cols * (width + gapX) - gapX)
  }
  const links = []
  for (let i = 0; i < n; i++) for (const p of new Set(parents[i])) if (valid(p, i)) links.push({ parent: p, child: i, primary: p === first[i] })
  return { x, y, width: right || width, height: bottom, links, first }
}

/**
 * The Schema drawing in `el`.
 * @param {HTMLElement} el
 * @param {object} on
 * @param {(i: number) => void} on.select a class chosen (-1: none); the view shows it in the inspector
 * @param {(i: number) => void} on.open open class `i`'s definition
 * @param {(i: number, row: number) => void} on.row a row chosen with Enter
 * @param {(show: boolean) => void} on.imports list imported classes one by one, or fold them
 * @param {(on: boolean) => void} on.problems "Problems only" toggled
 */
export function createSchemaView(el, on) {
  const doc = el.ownerDocument
  const win = doc.defaultView
  const svg = doc.createElementNS(SVG, "svg")
  svg.setAttribute("class", "schema-lines")
  svg.setAttribute("aria-hidden", "true")
  const path = (cls) => {
    const p = doc.createElementNS(SVG, "path")
    p.setAttribute("class", cls)
    svg.append(p)
    return p
  }
  const treeLines = path("schema-tree")
  const extraLines = path("schema-extra")
  const heads = path("schema-head")
  const hot = path("schema-hot")
  const hotDot = doc.createElementNS(SVG, "circle")
  hotDot.setAttribute("class", "schema-hot-dot")
  hotDot.setAttribute("r", "4")
  svg.append(hotDot)
  const world = h(doc, "div", { class: "schema-world" })
  const viewport = h(doc, "div", { class: "schema-view", role: "group", "aria-roledescription": "class schema", "aria-label": "Class schema. Arrow keys move between classes and rows, Enter selects or opens, plus and minus zoom, 0 fits." }, world)
  const problemsButton = h(doc, "button", { type: "button", class: "graph-btn schema-problems-btn", "aria-pressed": "false", text: "Problems only", title: "Fade classes without problems and hide rows that are fine" })
  const legendButton = h(doc, "button", { type: "button", class: "graph-btn schema-legend-btn", "aria-expanded": "false", "aria-label": "Legend", title: "Legend", text: "?" })
  const usage = h(doc, "span", { class: "schema-use", "aria-hidden": "true" }, h(doc, "span", { class: "schema-bar" }))
  usage.firstChild.style.setProperty("--used", "60%")
  const legend = h(doc, "div", { class: "schema-legend", id: `schema-legend-${Math.random().toString(36).slice(2)}`, role: "list", "aria-label": "Legend", hidden: true },
    STATES.map((s) => h(doc, "div", { role: "listitem", class: `schema-key schema-st-${s}` }, h(doc, "span", { class: "schema-mk", "aria-hidden": "true", text: MARK[s] }), STATE_TEXT[s])),
    h(doc, "div", { role: "listitem", class: "schema-key" }, usage, "bar: the share of the class's instances that use the property"),
    h(doc, "div", { role: "listitem", class: "schema-key" }, h(doc, "span", { class: "schema-use", text: "30/57" }), "30 of the class's 57 instances use it"),
    h(doc, "div", { role: "listitem", class: "schema-key" }, h(doc, "span", { class: "schema-card", text: "1..1" }), "values each instance must have, min..max (sh:minCount, sh:maxCount; * no maximum)"),
    h(doc, "div", { role: "listitem", class: "schema-key" }, h(doc, "span", { class: "schema-badge sev-error", text: "3 missing" }), "instances lack a required property"),
    h(doc, "div", { role: "listitem", class: "schema-key" }, h(doc, "span", { class: "schema-chip", text: "→ Class" }), "link: hover for the arrow, click to go"),
    h(doc, "div", { role: "listitem", class: "schema-key" }, h(doc, "span", { class: "schema-pb sev-error", text: "2" }), "problems in the class"),
    h(doc, "div", { role: "listitem", class: "schema-key" }, h(doc, "span", { class: "schema-ghost-key", "aria-hidden": "true" }), "used, never defined"),
    h(doc, "p", { class: "schema-legend-note", text: "Red and amber mark problems only." }))
  legendButton.setAttribute("aria-controls", legend.id)
  const importsButton = h(doc, "button", { type: "button", class: "graph-btn schema-imports-btn", "aria-pressed": "false", text: "Imports", title: "List imported classes one by one" })
  const tools = h(doc, "div", { class: "schema-tools" }, problemsButton, importsButton, legendButton)
  const empty = h(doc, "p", { class: "schema-empty", hidden: true, text: "No classes in the selected files." })
  el.replaceChildren(viewport, tools, legend, empty)

  let model = null
  let imports = false
  let problemsOnly = false
  let far = false
  // Boxes: per box its class index (-1: the imported classes' box) and element.
  let items = []
  let boxEl = []
  // Per box, what it draws (sigOf): a box whose class is drawn the same is kept.
  let sigs = []
  let itemOf = new Int32Array(0)
  let importsItem = -1
  let lay = null
  let heights = []
  let selected = -1
  let lit = -1
  let current = null // the element with tabindex 0
  // The boxes were rebuilt by the last show (false: updated in place).
  let rebuilt = false
  let hotTarget = null
  const cam = { x: 0, y: 0, k: 1 }
  let userMoved = false
  let fileRoot = ""

  // ---- building the boxes ----
  function fileText(uri, range) {
    return middleCut(fileLabel(uri, fileRoot), FILE_CHARS) + (range ? `:${range.start.line + 1}` : "")
  }

  // A row's fixed parts, cloned per row (faster than building each element).
  const rowTemplate = h(doc, "div", { tabindex: "-1", role: "listitem" },
    h(doc, "span", { class: "schema-mk", "aria-hidden": "true" }),
    h(doc, "span", { class: "graph-sr" }),
    h(doc, "span", { class: "schema-pn" }),
    h(doc, "div", { class: "schema-meta" }, h(doc, "span", { class: "schema-use" }, h(doc, "span", { class: "schema-bar", "aria-hidden": "true" }), "")))
  const span = (cls, text, title) => {
    const e = doc.createElement("span")
    e.className = cls
    e.textContent = text
    if (title) e.title = title
    return e
  }

  function rowEl(r, k) {
    const badge = rowBadge(r)
    const row = rowTemplate.cloneNode(true)
    row.className = `schema-row schema-st-${r.state}${r.state === "ok" && !r.missing ? " is-fine" : ""}`
    row.dataset.r = k
    row.title = r.property
    const words = [STATE_TEXT[r.state]]
    if (badge && (badge.kind === "missing" || badge.kind === "badValues")) words.push(badge.text)
    row.setAttribute("aria-label", [r.name, r.rangeName, words.join(", ")].filter(Boolean).join(", "))
    const [mk, sr, pn, meta] = row.children
    mk.textContent = MARK[r.state]
    sr.textContent = `${STATE_TEXT[r.state]}: `
    pn.textContent = r.name
    if (r.link && (r.target >= 0 || r.rangeName)) {
      const chip = doc.createElement("button")
      chip.type = "button"
      chip.className = r.target >= 0 ? "schema-chip" : "schema-chip is-dead"
      chip.tabIndex = -1
      chip.dataset.t = r.target
      chip.title = r.range || r.rangeName
      chip.setAttribute("aria-label", `Link to ${r.rangeName || "an unknown class"}`)
      chip.textContent = `→ ${r.rangeName ? short(r.rangeName) : "?"}`
      row.insertBefore(chip, meta)
    } else if (!r.link && r.rangeName) row.insertBefore(span("schema-rg", r.rangeName, r.range), meta)
    const use = meta.firstChild
    use.title = `${fmt(r.used)} of ${plural(r.of, "instance")} use it`
    use.firstChild.style.setProperty("--used", `${r.of ? Math.min(100, (100 * r.used) / r.of) : 0}%`)
    use.lastChild.data = `${fmt(r.used)}/${fmt(r.of)}`
    if (r.required) meta.append(span("schema-card", `${r.required.min}..${r.required.max ?? "*"}`, `Required: sh:minCount ${r.required.min}${r.required.max != null ? `, sh:maxCount ${r.required.max}` : ""}`))
    if (badge) {
      const b = span(`schema-badge sev-${badge.sev || "none"}`, badge.text)
      b.dataset.badge = badge.kind
      meta.append(b)
    }
    return row
  }

  function boxOf(i) {
    const c = model.classes[i]
    const [pre, local] = nameParts(c.name)
    const label = `${c.label || c.name}, ${c.kind === "undefined" ? "never defined" : plural(c.instances, "instance")}${c.problemCount ? `, ${plural(c.problemCount, "problem")}` : ""}`
    const head = h(doc, "div", { class: "schema-hd" },
      h(doc, "span", { class: "schema-name", title: c.id }, pre ? h(doc, "span", { class: "schema-pre", text: pre }) : null, local),
      c.kind !== "undefined" ? h(doc, "span", { class: "schema-count", title: plural(c.instances, "instance"), text: fmt(c.instances) }) : null,
      c.problemCount ? h(doc, "span", { class: `schema-pb sev-${c.worst || "none"}`, title: plural(c.problemCount, "problem"), text: fmt(c.problemCount) }) : null,
      c.kind === "imported" ? h(doc, "span", { class: "schema-tag", text: "imported" }) : null,
      c.uri ? h(doc, "span", { class: "schema-file", title: c.uri, text: fileText(c.uri, c.range) }) : null,
      c.kind === "undefined" ? h(doc, "span", { class: "schema-sub", text: "used, never defined" }) : null)
    return h(doc, "div", {
      class: `schema-box is-${c.kind}${c.problemCount ? " has-problems" : ""}`,
      tabindex: "-1", role: "group", "aria-label": label, "data-c": i,
    }, head,
    c.rows.length ? h(doc, "div", { class: "schema-rows", role: "list", "aria-label": `Properties of ${classText(c)}` }, c.rows.map(rowEl), c.moreRows ? h(doc, "div", { class: "schema-more", role: "listitem", text: `+${fmt(c.moreRows)} more` }) : null)
      : c.kind === "class" ? h(doc, "div", { class: "schema-rows" }, h(doc, "div", { class: "schema-none", text: "No properties" })) : null)
  }

  function importsBox(n) {
    return h(doc, "div", { class: "schema-box is-imports", tabindex: "-1", role: "group", "aria-label": `${plural(n, "imported class", "imported classes")}` },
      h(doc, "div", { class: "schema-hd" },
        h(doc, "span", { class: "schema-name", text: plural(n, "imported class", "imported classes") }),
        h(doc, "button", { type: "button", class: "graph-btn schema-imports", "data-act": "imports", "aria-pressed": String(imports), text: imports ? "Fold" : "List them" })))
  }

  /**
   * Draw an answer. `o.imports`: imported classes one box each (else one box
   * counts them); `o.focus`: select the answer's focus and centre it (an
   * answer that lists the same classes updates the selection and the camera
   * in place, without building the boxes again); else the selection stays
   * and the camera keeps its zoom and pan, shifted so the selected (else the
   * focus) class stays where it was on screen. The first answer opens on
   * firstView.
   * @returns {number} ms to build, measure and place the boxes
   */
  function show(res, o = {}) {
    const t0 = win.performance.now()
    const fresh = schemaModel(res)
    const wantImports = o.imports === true
    if (o.focus && model && lay && wantImports === imports && sameClasses(model, fresh)) {
      model = fresh
      rebuilt = false
      mark(model.focus, model.focusRow)
      if (selected >= 0) center(selected)
      return done(t0)
    }
    const prev = model && lay ? anchorOf() : null
    const prevId = selected >= 0 ? model?.classes[selected]?.id : null
    model = fresh
    rebuilt = true
    imports = wantImports
    importsButton.setAttribute("aria-pressed", String(imports))
    fileRoot = commonRoot(model.classes.map((c) => c.uri).filter(Boolean))
    items = []
    itemOf = new Int32Array(model.classes.length).fill(-1)
    let foldedImports = 0
    model.classes.forEach((c, i) => {
      if (c.kind === "imported" && !imports) return void foldedImports++
      itemOf[i] = items.length
      items.push(i)
    })
    const importsN = (imports ? model.classes.filter((c) => c.kind === "imported").length : foldedImports) + model.imported
    importsItem = importsN ? items.length : -1
    if (importsN) items.push(-1)
    // Boxes drawn the same stay (no new elements to style and lay out);
    // the others are built, and the DOM order follows the answer's.
    const old = new Map()
    boxEl.forEach((b, k) => sigs[k] && old.set(sigs[k], b))
    if (current) current.tabIndex = -1
    world.querySelector(".schema-box.is-selected")?.classList.remove("is-selected")
    world.querySelector(".schema-row.is-lit")?.classList.remove("is-lit")
    sigs = items.map((i) => (i < 0 ? "" : sigOf(i)))
    boxEl = items.map((i, k) => {
      const b = sigs[k] && old.get(sigs[k])
      if (b) old.delete(sigs[k])
      return b || (i < 0 ? importsBox(importsN) : boxOf(i))
    })
    boxEl.forEach((b, k) => (b.dataset.item = k))
    let at = world.firstChild
    if (at !== svg) world.insertBefore(svg, at)
    at = svg.nextSibling
    for (const b of boxEl) {
      if (at === b) at = at.nextSibling
      else world.insertBefore(b, at)
    }
    while (at) {
      const next = at.nextSibling
      at.remove()
      at = next
    }
    empty.hidden = items.length > 0
    hideArrow()
    current = null
    relayout()
    const keep = prevId != null ? model.index.get(prevId) ?? -1 : -1
    selected = -1
    lit = -1
    const next = o.focus || keep < 0 ? model.focus : keep
    if (next >= 0) mark(next, next === model.focus ? model.focusRow : -1)
    else setCurrent(boxEl[0] ?? null)
    if (o.focus && selected >= 0) center(selected)
    else if (prev) keepAnchor(prev)
    else firstView()
    return done(t0)
  }
  function done(t0) {
    const ms = win.performance.now() - t0
    el.dataset.drawMs = ms.toFixed(2)
    return ms
  }

  // What box `i` draws: its class but the places (the inspector's), its index, the file root.
  const sigOf = (i) => JSON.stringify([i, fileRoot, model.classes[i]], (k, v) => (k === "examples" || k === "problems" ? undefined : v))

  // The same classes in the same order, each with as many rows, instances
  // and problems: the boxes drawn still show the answer.
  function sameClasses(a, b) {
    if (a.classes.length !== b.classes.length || a.imported !== b.imported) return false
    for (let i = 0; i < a.classes.length; i++) {
      const x = a.classes[i], y = b.classes[i]
      if (x.id !== y.id || x.kind !== y.kind || x.rows.length !== y.rows.length || x.instances !== y.instances || x.problemCount !== y.problemCount || x.moreRows !== y.moreRows) return false
    }
    return true
  }

  // The class the camera keeps in place across a new answer, and where on screen it is.
  function anchorOf() {
    const i = selected >= 0 ? selected : focusClass()
    const k = i >= 0 ? itemOf[i] : -1
    if (k == null || k < 0) return { id: null }
    return { id: model.classes[i].id, sx: lay.x[k] * cam.k + cam.x, sy: lay.y[k] * cam.k + cam.y }
  }
  function keepAnchor(a) {
    const i = a.id != null ? model.index.get(a.id) : undefined
    const k = i != null ? itemOf[i] : -1
    if (lay && k >= 0) {
      cam.x = a.sx - lay.x[k] * cam.k
      cam.y = a.sy - lay.y[k] * cam.k
    }
    applyCam()
  }
  /** The focus class: the answer's focus, else its first class listed in a box; -1: none. */
  function focusClass() {
    if (!model) return -1
    if (model.focus >= 0 && itemOf[model.focus] >= 0) return model.focus
    return items.find((i) => i >= 0) ?? -1
  }

  // Measure every box (one layout), lay them out, then write every position.
  function relayout() {
    if (!items.length) {
      lay = null
      return
    }
    heights = boxEl.map((b) => b.offsetHeight)
    const parents = items.map((i) => (i < 0 ? [] : model.classes[i].parents.map((p) => (itemOf[p] >= 0 ? itemOf[p] : importsItem)).filter((p) => p >= 0)))
    lay = tidyLayout(parents, heights)
    for (let k = 0; k < items.length; k++) boxEl[k].style.transform = `translate(${lay.x[k]}px,${lay.y[k]}px)`
    drawLines()
  }

  function drawLines() {
    const { x, y, links } = lay
    let tree = "", extra = "", tri = ""
    const headed = new Set()
    for (const { parent: p, child: c, primary } of links) {
      const px = x[p] + W / 2, pb = y[p] + heights[p]
      const cx = x[c] + W / 2, ct = y[c]
      if (primary) {
        const mid = Math.max(pb + 9 + 8, ct - GAP_Y / 2)
        tree += `M${cx},${ct}V${mid}H${px}V${pb + 9}`
      } else extra += `M${cx + 12},${ct}C${cx + 12},${ct - 60} ${px},${pb + 70} ${px},${pb + 9}`
      if (!headed.has(p)) headed.add(p), (tri += `M${px},${pb}l-6,9h12z`)
    }
    treeLines.setAttribute("d", tree)
    extraLines.setAttribute("d", extra)
    heads.setAttribute("d", tri)
    svg.setAttribute("width", String(Math.max(1, lay.width)))
    svg.setAttribute("height", String(Math.max(1, lay.height)))
  }

  // ---- the hover arrow: one path, from a link chip to its target's box ----
  function showArrow(chip) {
    const t = Number(chip.dataset.t)
    const row = chip.closest(".schema-row")
    const from = Number(chip.closest(".schema-box")?.dataset.item)
    const to = t >= 0 ? (itemOf[t] >= 0 ? itemOf[t] : importsItem) : -1
    if (!lay || !(from >= 0) || to < 0) return hideArrow()
    const x1 = lay.x[from] + W, y1 = lay.y[from] + row.offsetTop + row.offsetHeight / 2
    const ty = lay.y[to] + 14
    let x2, c1, c2
    if (to === from) (x2 = x1), (c1 = x1 + 80), (c2 = x1 + 80)
    else if (lay.x[to] > x1) (x2 = lay.x[to]), (c1 = (x1 + x2) / 2), (c2 = c1)
    else (x2 = lay.x[to] + W), (c1 = Math.max(x1, x2) + 120), (c2 = c1)
    hot.setAttribute("d", `M${x1},${y1}C${c1},${y1} ${c2},${ty} ${x2},${ty}`)
    hotDot.setAttribute("cx", String(x2))
    hotDot.setAttribute("cy", String(ty))
    svg.classList.add("is-hot")
    hotTarget?.classList.remove("is-target")
    hotTarget = boxEl[to]
    hotTarget.classList.add("is-target")
  }
  function hideArrow() {
    svg.classList.remove("is-hot")
    hot.setAttribute("d", "")
    hotTarget?.classList.remove("is-target")
    hotTarget = null
  }

  // ---- the camera ----
  // `anchor`: a point of the view (a zoom's pointer) that keeps the box
  // under it in place when the boxes fold or unfold, which moves them all.
  function applyCam(anchor) {
    const nowFar = cam.k < FAR
    if (nowFar !== far) {
      const keep = anchor && lay ? boxAt(anchor) : null
      far = nowFar
      el.classList.toggle("schema-far", far)
      if (model) relayout()
      if (keep && lay) {
        const { k, fx, fy } = keep
        cam.x = anchor[0] - (lay.x[k] + fx * W) * cam.k
        cam.y = anchor[1] - (lay.y[k] + fy * heights[k]) * cam.k
      }
    }
    world.style.transform = `translate(${cam.x}px,${cam.y}px) scale(${cam.k})`
  }

  // The box nearest to view point `p`, and where in it (0 to 1) `p` falls.
  function boxAt([sx, sy]) {
    const wx = (sx - cam.x) / cam.k, wy = (sy - cam.y) / cam.k
    let best = -1, bestD = Infinity
    for (let k = 0; k < lay.x.length; k++) {
      const dx = Math.max(lay.x[k] - wx, 0, wx - lay.x[k] - W)
      const dy = Math.max(lay.y[k] - wy, 0, wy - lay.y[k] - heights[k])
      const d = dx * dx + dy * dy
      if (d < bestD) (bestD = d), (best = k)
    }
    if (best < 0) return null
    const unit = (v) => Math.min(1, Math.max(0, v))
    return { k: best, fx: unit((wx - lay.x[best]) / W), fy: unit((wy - lay.y[best]) / Math.max(1, heights[best])) }
  }
  const viewSize = () => {
    const b = viewport.getBoundingClientRect()
    return [b.width, b.height]
  }
  const clamp = (k) => Math.min(ZOOM[1], Math.max(ZOOM[0], k))

  // The zoom that shows the whole layout as it is now (folded or not).
  function fitScale() {
    const [vw, vh] = viewSize()
    return clamp(Math.min(1, (vw - 32) / lay.width, (vh - 56) / Math.max(1, lay.height)))
  }

  /** The whole model in view, at FIT_FLOOR or closer: past the floor, the selected (else focus) class is centred. */
  function fit() {
    if (!lay) return applyCam()
    const [vw, vh] = viewSize()
    if (vw < 2 || vh < 2) return applyCam()
    cam.k = fitScale()
    applyCam()
    // Under FAR the boxes fold: the floor, folded, whatever the folded layout's fit.
    if (far) cam.k = FIT_FLOOR
    const w = lay.width * cam.k, hgt = lay.height * cam.k
    const anchor = selected >= 0 ? selected : focusClass()
    if ((w > vw - 16 || hgt > vh - 40) && anchor >= 0) return center(anchor, cam.k, true), (userMoved = false)
    cam.x = (vw - w) / 2
    cam.y = Math.max(40, (vh - hgt) / 2)
    userMoved = false
    applyCam()
  }

  /**
   * The first view: the whole model when it fits at READABLE or closer,
   * else the focus class centred at FOCUS_ZOOM with its neighbours around it.
   */
  function firstView() {
    if (!lay) return applyCam()
    const [vw, vh] = viewSize()
    if (vw < 2 || vh < 2) return applyCam()
    // Measure unfolded.
    if (far) (cam.k = 1), applyCam()
    const f = focusClass()
    if (fitScale() >= READABLE || f < 0) return fit()
    center(f, FOCUS_ZOOM, true)
    userMoved = false
  }

  // Centre class `i` at zoom `kMin` or closer (`exact`: at `kMin`).
  function center(i, kMin = 0.8, exact = false) {
    const k = itemOf[i]
    if (!lay || k == null || k < 0) return
    const [vw, vh] = viewSize()
    cam.k = exact ? clamp(kMin) : Math.max(cam.k, kMin)
    applyCam()
    const hgt = heights[k] * cam.k
    cam.x = vw / 2 - (lay.x[k] + W / 2) * cam.k
    cam.y = hgt < vh - 80 ? vh / 2 - (lay.y[k] + heights[k] / 2) * cam.k : 48 - lay.y[k] * cam.k
    userMoved = true
    applyCam()
  }

  // Keep box `k` in view (keyboard moves).
  function inView(k) {
    if (!lay) return
    const [vw, vh] = viewSize()
    const sx = lay.x[k] * cam.k + cam.x, sy = lay.y[k] * cam.k + cam.y
    if (sx < 0 || sy < 40 || sx + W * cam.k > vw || sy + Math.min(heights[k] * cam.k, vh / 2) > vh) {
      const i = items[k]
      if (i >= 0) center(i, cam.k)
    }
  }

  function zoomAt(f, sx, sy) {
    if (sx == null) [sx, sy] = viewSize().map((v) => v / 2)
    const wx = (sx - cam.x) / cam.k, wy = (sy - cam.y) / cam.k
    cam.k = clamp(cam.k * f)
    cam.x = sx - wx * cam.k
    cam.y = sy - wy * cam.k
    userMoved = true
    applyCam([sx, sy])
  }

  // ---- selection and the keyboard ----
  function setCurrent(e, focus = false) {
    if (current && current !== e) current.tabIndex = -1
    current = e
    if (!e) return
    e.tabIndex = 0
    if (focus) {
      e.focus({ preventScroll: true })
      const box = e.closest(".schema-box")
      if (box) inView(Number(box.dataset.item))
    }
  }

  // Show `i` as selected (and row `row` lit) without telling the view.
  function mark(i, row = -1) {
    if (selected >= 0) boxEl[itemOf[selected]]?.classList.remove("is-selected")
    world.querySelector(".schema-row.is-lit")?.classList.remove("is-lit")
    selected = model && i >= 0 && i < model.classes.length ? i : -1
    lit = selected >= 0 ? row : -1
    const box = selected >= 0 ? boxEl[itemOf[selected]] : null
    box?.classList.add("is-selected")
    if (box && lit >= 0) box.querySelector(`.schema-row[data-r="${lit}"]`)?.classList.add("is-lit")
    if (box) setCurrent(box)
  }

  /** Select class `i` (-1: none), light row `row`, centre it, move the focus to it; the view is told. */
  function select(i, { row = -1, center: c = false, focus = false } = {}) {
    mark(i, row)
    if (c && selected >= 0) center(selected)
    if (focus && selected >= 0) setCurrent(boxEl[itemOf[selected]], true)
    on.select(selected)
  }

  const visibleRows = (box) => [...box.querySelectorAll(".schema-row")].filter((r) => r.offsetParent !== null)
  // Boxes in reading order: top to bottom, then left to right.
  const reading = () => boxEl.map((_, k) => k).sort((a, b) => lay.y[a] - lay.y[b] || lay.x[a] - lay.x[b])

  viewport.addEventListener("keydown", (e) => {
    const row = e.target.closest?.(".schema-row")
    const box = e.target.closest?.(".schema-box")
    if (e.key === "+" || e.key === "=") return e.preventDefault(), zoomAt(1.25)
    if (e.key === "-") return e.preventDefault(), zoomAt(0.8)
    if (e.key === "0") return e.preventDefault(), fit()
    if (!box || !lay) return
    const k = Number(box.dataset.item)
    const i = items[k]
    const order = reading()
    const step = (d) => boxEl[order[order.indexOf(k) + d]]
    let go = null
    if (row) {
      const rows = visibleRows(box)
      const at = rows.indexOf(row)
      if (e.key === "ArrowDown") go = rows[at + 1] ?? step(1)
      else if (e.key === "ArrowUp") go = rows[at - 1] ?? box
      else if (e.key === "ArrowRight") go = step(1)
      else if (e.key === "ArrowLeft") go = step(-1)
      else if (e.key === "Enter" || e.key === " ") {
        e.preventDefault()
        const r = model.classes[i].rows[Number(row.dataset.r)]
        if (r?.link && r.target >= 0 && e.key === "Enter") return select(r.target, { center: true, focus: true })
        mark(i, Number(row.dataset.r))
        on.select(selected)
        on.row?.(i, Number(row.dataset.r))
        return
      } else return
    } else {
      if (e.key === "ArrowDown") go = visibleRows(box)[0] ?? step(1)
      else if (e.key === "ArrowUp") go = i >= 0 && lay.first[k] >= 0 ? boxEl[lay.first[k]] : step(-1)
      else if (e.key === "ArrowRight") go = step(1)
      else if (e.key === "ArrowLeft") go = step(-1)
      else if (e.key === "Home") go = boxEl[order[0]]
      else if (e.key === "End") go = boxEl[order[order.length - 1]]
      else if (e.key === "Enter" || e.key === " ") {
        e.preventDefault()
        if (i < 0) return on.imports(!imports)
        if (e.key === "Enter" && selected === i) return on.open(i)
        return select(i)
      } else return
    }
    e.preventDefault()
    if (go) setCurrent(go, true)
  })

  // ---- pointer: pan, zoom, click, hover ----
  let drag = null
  let dragged = false
  viewport.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return
    drag = { x: e.clientX, y: e.clientY, cx: cam.x, cy: cam.y, moved: false, id: e.pointerId }
    dragged = false
  })
  viewport.addEventListener("pointermove", (e) => {
    if (!drag) return
    const dx = e.clientX - drag.x, dy = e.clientY - drag.y
    if (!drag.moved && Math.abs(dx) + Math.abs(dy) <= 3) return
    if (!drag.moved) {
      drag.moved = true
      viewport.setPointerCapture?.(drag.id)
      viewport.classList.add("is-dragging")
      hideArrow()
    }
    cam.x = drag.cx + dx
    cam.y = drag.cy + dy
    userMoved = true
    world.style.transform = `translate(${cam.x}px,${cam.y}px) scale(${cam.k})`
  })
  const endDrag = () => {
    dragged = Boolean(drag?.moved)
    drag = null
    viewport.classList.remove("is-dragging")
  }
  viewport.addEventListener("pointerup", endDrag)
  viewport.addEventListener("pointercancel", endDrag)
  viewport.addEventListener("click", (e) => {
    if (dragged) return void (dragged = false)
    if (e.target.closest("[data-act='imports']")) return on.imports(!imports)
    const chip = e.target.closest(".schema-chip")
    if (chip) {
      const t = Number(chip.dataset.t)
      if (t >= 0) {
        if (itemOf[t] < 0) return on.imports(true)
        return select(t, { center: true, focus: true })
      }
      return
    }
    const box = e.target.closest(".schema-box")
    if (!box) return
    const i = items[Number(box.dataset.item)]
    if (i < 0) return
    const row = e.target.closest(".schema-row")
    mark(i, row ? Number(row.dataset.r) : -1)
    setCurrent(row ?? box)
    on.select(selected)
  })
  viewport.addEventListener("dblclick", (e) => {
    const box = e.target.closest(".schema-box")
    const i = box ? items[Number(box.dataset.item)] : -1
    if (i >= 0 && !e.target.closest(".schema-chip")) on.open(i)
  })
  viewport.addEventListener("pointerover", (e) => {
    const chip = e.target.closest?.(".schema-chip")
    if (chip && !drag?.moved) showArrow(chip)
  })
  viewport.addEventListener("pointerout", (e) => {
    const chip = e.target.closest?.(".schema-chip")
    if (chip && !chip.contains(e.relatedTarget)) hideArrow()
  })
  viewport.addEventListener("focusin", (e) => {
    if (e.target.classList?.contains("schema-row") || e.target.classList?.contains("schema-box")) setCurrent(e.target)
  })
  viewport.addEventListener("wheel", (e) => {
    e.preventDefault()
    const b = viewport.getBoundingClientRect()
    zoomAt(Math.exp(-e.deltaY * 0.0015), e.clientX - b.left, e.clientY - b.top)
  }, { passive: false })

  problemsButton.addEventListener("click", () => {
    setProblemsOnly(!problemsOnly)
    on.problems(problemsOnly)
  })
  importsButton.addEventListener("click", () => on.imports(!imports))
  legendButton.addEventListener("click", () => {
    const open = legend.hidden
    legend.hidden = !open
    legendButton.setAttribute("aria-expanded", String(open))
  })
  el.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return
    e.preventDefault()
    escape()
  })

  /** Escape: close the legend if it is open, else clear the selection and put the focus on the box that was selected. */
  function escape() {
    if (!legend.hidden) {
      legend.hidden = true
      legendButton.setAttribute("aria-expanded", "false")
      return
    }
    const box = selected >= 0 ? boxEl[itemOf[selected]] : null
    select(-1)
    if (box && box.offsetParent !== null) setCurrent(box, true)
  }

  function setProblemsOnly(v) {
    problemsOnly = v === true
    problemsButton.setAttribute("aria-pressed", String(problemsOnly))
    el.classList.toggle("schema-problems-only", problemsOnly)
    if (model) relayout()
  }

  return {
    show,
    select,
    escape,
    /** The last show built the boxes anew (false: it updated them in place). */
    get rebuilt() {
      return rebuilt
    },
    /** The class shown when none is selected (the narrow inspector): the answer's focus, else the first class; -1: none. */
    get focusClass() {
      return focusClass()
    },
    /** The selected class (-1: none), its lit row, the answer as checked. */
    get selected() {
      return selected
    },
    get lit() {
      return lit
    },
    get model() {
      return model
    },
    get selectedId() {
      return selected >= 0 ? model.classes[selected].id : null
    },
    fit,
    /** The view changed size: the first view again unless the user moved the camera. */
    resize() {
      if (model && !userMoved) firstView()
    },
    setProblemsOnly,
    /** Classes whose name, label or IRI has `q`: exact first, then prefix, then the rest; at most `max`. */
    search(q, max = 50) {
      const s = String(q ?? "").trim().toLowerCase()
      if (!s || !model) return []
      const scored = []
      model.classes.forEach((c, i) => {
        const names = [c.label, c.name, c.name.slice(c.name.indexOf(":") + 1), c.id].map((v) => v.toLowerCase())
        const score = names.some((v) => v === s) ? 0 : names.some((v) => v.startsWith(s)) ? 1 : names.some((v) => v.includes(s)) ? 2 : -1
        if (score >= 0) scored.push([score, i])
      })
      return scored.sort((a, b) => a[0] - b[0] || a[1] - b[1]).slice(0, max).map(([, i]) => i)
    },
  }
}
