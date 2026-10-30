// The workspace graph view, shared by the playground and the VS Code webview.
// Three modes on one filter panel: the graph (A: the server's force layout on
// a 2D canvas), focus (C: the selected node on rings, N hops, capped per ring
// with "+N more") and the schema (classes as boxes with their properties,
// schema.js). Follow cursor shows the term at the editor's cursor in each.
// It talks to its host only through the functions it is given: `request` for
// `turtle/graph` and `turtle/schema`, `reveal` to open a definition,
// `diagnostics` to mark nodes, `store` to keep the filters; the host calls
// `follow` with the cursor. No globals, no network, no inline handlers or styles.

import { h, kids } from "./el.js"
import { BIG, KINDS, LITERAL, bestNode, buildModel, chooseDefault, commonRoot, diagnosticMarks, fileLabel, middleCut, nameOf, neighbours, searchNodes, triplesOf } from "./model.js"
import { CAPS, CONTINUE_ITERATIONS, FIRST_ITERATIONS, MODES, facetParams, normalizeState, schemaParams, selectsNothing, toParams } from "./filters.js"
import { createHitGrid } from "./hitgrid.js"
import { RING_CAPS, ringLayout } from "./rings.js"
import { pickLabels, truncate } from "./labels.js"
import { PALETTE_SIZE, colorSlots, readColors } from "./colors.js"
import { createRenderer } from "./canvas.js"
import { classInspector, createFilterPanel } from "./panel.js"
import { SCHEMA_METHOD, createSchemaView, displayName, mergePlaces, needsPlaces } from "./schema.js"

export const STORE_KEY = "graph"
export const METHOD = "turtle/graph"
const DEBOUNCE_MS = 150
const HIT_PX = 8
const ZOOM = [0.005, 20]
const TITLES = { graph: "Graph", focus: "Focus", schema: "Schema" }
const TRIPLES_SHOWN = 50
const SEVERITY = ["", "error", "warning", "information", "hint"]
// File names, in characters, cut in the middle (middleCut): a legend entry
// shares a line with up to seven others; the info panel's line is about 320
// px of 11 px monospace.
const LEGEND_CHARS = 32
const INFO_CHARS = 48
const NO_FILES = "No files selected"
// The inspector alone, no boxes, under this width (graph.css has the same).
const NARROW = 700

/**
 * Mount the graph view in `container`.
 * @param {object} o
 * @param {HTMLElement} o.container
 * @param {(method: string, params: object) => Promise<any>} o.request sends `turtle/graph` or `turtle/schema` to the language server
 * @param {(uri: string, range: object) => void} [o.reveal] opens a definition
 * @param {(uri: string) => { range: object, severity?: number }[]} [o.diagnostics] a document's diagnostics
 * @param {Record<string, string>} [o.theme] `--graph-…` CSS variables to set on the view
 * @param {{ get(key: string): any, set(key: string, value: any): void }} [o.store] keeps the filters and mode
 * @returns {{ refresh(): Promise<void>, resize(): void, destroy(): void, setTheme(theme?: Record<string, string>): void, follow(at: { uri: string, position: { line: number, character: number } } | null): void }}
 *
 * `follow(at)`: the editor's cursor moved to `at` (null: no cursor). With
 * Follow cursor on (a toggle in the bar, kept in the store, off by default)
 * and the view visible, the view asks, once the cursor has rested 150 ms,
 * for the term there: Schema asks `turtle/schema` with `at` and selects the
 * class (and lights the row); Focus asks `turtle/graph` with `at` and walks
 * to the node; Graph asks `turtle/graph` with `at` (one node, no layout)
 * and selects and centres the node the answer's `focus` names. One request
 * is in flight at a time; an answer for an older cursor is dropped, and an
 * answer with no focus (the cursor on no term) leaves the view as it is but
 * for Schema's lit row, which goes. A Schema answer that lists the classes
 * shown updates the selection in place (schema.js show).
 */
// `maxNodes`: the largest node cap offered (a host whose layout runs on the
// thread that answers its editor keeps it low).
// Gives each view's filter region its own id (aria-controls).
let views = 0

export function createGraphView({ container, request, reveal, diagnostics, theme, store, maxNodes = CAPS[CAPS.length - 1] } = {}) {
  if (!container?.ownerDocument) throw new TypeError("createGraphView: a container element is required")
  if (typeof request !== "function") throw new TypeError("createGraphView: a request function is required")
  const doc = container.ownerDocument
  const win = doc.defaultView
  const nodeCaps = CAPS.filter((c) => c <= maxNodes)
  const state = normalizeState(readStore(store), nodeCaps[nodeCaps.length - 1])

  // ---- the elements ----
  const tabs = MODES.map((m) => h(doc, "button", { type: "button", role: "tab", class: "graph-tab", "data-mode": m, text: TITLES[m] }))
  const fitButton = h(doc, "button", { type: "button", class: "graph-btn graph-fit", text: "Fit", title: "Fit the graph to the view (0)" })
  const followButton = h(doc, "button", { type: "button", class: "graph-btn graph-follow", "aria-pressed": String(state.follow), text: "Follow cursor", title: "Show the term at the cursor of the editor beside this view" })
  const status = h(doc, "span", { class: "graph-status", role: "status", "aria-live": "polite" })
  const filtersEl = h(doc, "div", { class: "graph-filters", id: `graph-filters-${++views}`, role: "region", "aria-label": "Graph filters" })
  // Narrow (a side bar): the filters fold away so the drawing has the height.
  const filtersButton = h(doc, "button", { type: "button", class: "graph-btn graph-filters-toggle", text: "Filters", "aria-expanded": "false", "aria-controls": filtersEl.id })
  const canvas = h(doc, "canvas", { class: "graph-canvas", tabindex: "0", role: "application", "aria-roledescription": "graph", "aria-label": "Workspace graph. Click or search to select a node; arrow keys pan, plus and minus zoom, 0 fits, Enter opens the selected node's definition." })
  const moreLayer = h(doc, "div", { class: "graph-more-layer" })
  const tip = h(doc, "div", { class: "graph-tip", hidden: true, "aria-hidden": "true" })
  const legend = h(doc, "div", { class: "graph-legend", role: "list", "aria-label": "Colours" })
  const schemaBox = h(doc, "div", { class: "graph-schemabox", hidden: true })
  const info = h(doc, "section", { class: "graph-info", hidden: true, "aria-label": "Selected node" })
  // The inspector's file:line buttons and fix buttons: their locations.
  let infoSources = []
  const notice = h(doc, "p", { class: "graph-notice", hidden: true, text: `${NO_FILES}. Choose files under Files, or All.` })
  const stage = h(doc, "div", { class: "graph-stage" }, canvas, moreLayer, schemaBox, notice, legend, tip, info)
  const root = h(doc, "div", { class: "graph", "data-busy": "false" },
    h(doc, "div", { class: "graph-bar" }, h(doc, "div", { class: "graph-tabs", role: "tablist", "aria-label": "Graph mode" }, tabs), fitButton, followButton, filtersButton, status),
    h(doc, "div", { class: "graph-body" }, filtersEl, stage))
  container.replaceChildren(root)
  filtersButton.addEventListener("click", () => {
    const open = filtersButton.getAttribute("aria-expanded") !== "true"
    filtersButton.setAttribute("aria-expanded", String(open))
    root.classList.toggle("graph-filters-open", open)
  })

  const renderer = createRenderer(canvas)
  applyTheme(theme)
  let colors = readColors(root)

  // ---- what is shown ----
  let mode = state.mode ?? "graph"
  let model = null
  /** @type {null | { x: Float64Array, y: Float64Array, nodes: Int32Array | number[], kind: Uint8Array, edges: Int32Array, circles?: number[], ringLabels?: object[] }} */
  let scene = null
  let grid = null
  let rings = null
  let caps = RING_CAPS.slice()
  let marks = new Uint8Array(0)
  let slots = new Uint8Array(0)
  const known = new Map() // node id -> [x, y]: earlier positions, sent back so the layout stays put
  const order = { files: [], namespaces: [] } // first-seen facets, so colours keep their slot
  let cam = { x: 0, y: 0, k: 1 }
  let fitPending = true
  // The user panned or zoomed: new positions no longer refit the view.
  let userMoved = false
  let selected = -1
  let selId = null
  let near = new Set()
  let hover = -1
  let matches = []
  let seq = 0
  let timer = 0
  let frame = 0
  let destroyed = false
  let failed = false
  // The filters select no file: nothing is asked or drawn.
  let nothing = false

  const panel = createFilterPanel(filtersEl, state, {
    change(what) {
      save()
      if (what === "color") return recolor(), schedule()
      clearTimeout(timer)
      timer = setTimeout(load, DEBOUNCE_MS)
    },
    search: runSearch,
    async enter(q) {
      if (mode === "schema") {
        const found = await schemaSearch(q, true)
        if (found?.length && mode === "schema") pickMatch(found[0])
        return
      }
      runSearch(q)
      if (matches.length) focusNode(matches[0])
    },
    pick: focusNode,
  }, nodeCaps)
  const schema = createSchemaView(schemaBox, {
    select: () => renderInfo(),
    open: openClass,
    row: () => renderInfo(),
    imports(show) {
      state.schema.imports = show
      save()
      load()
    },
    problems(on) {
      state.schema.problems = on
      save()
    },
  })
  schema.setProblemsOnly(state.schema.problems)

  // ---- talking to the host ----
  function save() {
    try {
      store?.set?.(STORE_KEY, { mode: state.mode, focus: state.focus, colorBy: state.colorBy, follow: state.follow, schema: state.schema, filters: state.filters })
    } catch {}
  }

  async function ask(params, method = METHOD) {
    const my = ++seq
    root.dataset.busy = "true"
    try {
      const res = await request(method, params)
      if (my !== seq || destroyed) return null
      failed = false
      return res ?? {}
    } catch (e) {
      if (my === seq && !destroyed) {
        failed = true
        status.textContent = `The graph could not be loaded: ${e?.message ?? e}`
      }
      return null
    } finally {
      if (my === seq && !destroyed) root.dataset.busy = "false"
    }
  }

  const hops = () => state.filters.depth || 2

  /** Ask for the current mode's graph and show it. `changed`: the host said
   * the workspace changed (`refresh`). */
  async function load({ changed = false } = {}) {
    clearTimeout(timer)
    if (destroyed) return
    if (selectsNothing(state.filters)) return showNothing(changed)
    if (nothing) {
      nothing = false
      notice.hidden = true
      schemaBox.hidden = mode !== "schema"
    }
    if (mode === "schema") {
      // The Files filter lists the files of a graph answer: ask for them first.
      if (!model?.facets.files.length || changed) {
        const f = await ask(facetParams(state.filters))
        if (!f) return
        setModel({ facets: buildModel(f).facets, total: f.total })
      }
      const params = schemaParams(state.filters, { focus: schema.selectedId, imports: state.schema.imports })
      const res = await ask(params, SCHEMA_METHOD)
      if (!res || mode !== "schema") return
      showSchema(res, {}, params)
      scene = null
    } else if (mode === "focus") {
      if (!state.focus && !(await pickFocus())) return
      const res = await ask(toParams(state.filters, { focus: state.focus, depth: hops(), iterations: 0 }))
      if (!res) return
      setModel(res)
      if (!model.index.has(state.focus) && model.n) state.focus = model.nodes[bestNode(model)].id
      layoutRings()
    } else {
      const focus = state.filters.depth > 0 ? selId : null
      const res = await ask(toParams(state.filters, { focus, positions: known, iterations: known.size ? CONTINUE_ITERATIONS : FIRST_ITERATIONS }))
      if (!res) return
      setModel(res)
      for (let i = 0; i < model.n; i++) known.set(model.nodes[i].id, [model.x[i], model.y[i]])
      // The layout moved the nodes: keep them in view until the user takes over.
      if (!userMoved) fitPending = true
      layoutGraph()
    }
    showStatus()
    renderInfo()
    if (fitPending && scene && renderer.width > 1) fit()
    draw()
  }

  /** No file selected: no request, an empty drawing and the notice; the files stay listed to choose from. */
  async function showNothing(changed = false) {
    // An answer still on its way is for files no longer selected.
    seq++
    root.dataset.busy = "false"
    failed = false
    let facets = model?.facets
    if (!facets || changed) {
      // Opened with none selected (a stored choice), or the workspace changed:
      // ask for the files to list (no nodes).
      const res = await ask(facetParams(state.filters))
      if (!res || !selectsNothing(state.filters)) return
      facets = buildModel(res).facets
    }
    nothing = true
    setModel({ facets, total: { nodes: 0, edges: 0 } })
    scene = null
    rings = null
    moreLayer.replaceChildren()
    notice.hidden = false
    schemaBox.hidden = true
    showStatus()
    renderInfo()
    draw()
  }

  /** Focus mode with no focus yet: the selection, else the best-connected class of a first answer. */
  async function pickFocus() {
    if (selId) return (state.focus = selId), true
    let m = model
    if (!m?.n) {
      const res = await ask(toParams(state.filters, { iterations: 0 }))
      if (!res) return false
      m = buildModel(res)
    }
    const best = bestNode(m)
    if (best < 0) {
      setModel({})
      scene = null
      showStatus()
      draw()
      return false
    }
    state.focus = m.nodes[best].id
    return true
  }

  function setModel(res) {
    model = buildModel(res)
    marks = diagnosticMarks(model.nodes, diagnostics)
    for (const f of model.facets.files) if (!order.files.includes(f.uri)) order.files.push(f.uri)
    for (const ns of model.facets.namespaces) if (!order.namespaces.some((o) => o.iri === ns.iri)) order.namespaces.push(ns)
    panel.update(model.facets)
    recolor()
    selected = selId != null ? model.index.get(selId) ?? -1 : -1
    near = selected >= 0 ? neighbours(model, selected, 60) : new Set()
    runSearch(panel.search, false)
  }

  function recolor() {
    if (!model) return
    slots = colorSlots(model, state.colorBy, { files: order.files.map((uri) => ({ uri })), namespaces: order.namespaces })
    renderLegend()
  }

  // ---- the scene: positions, edges and the hit grid ----
  function layoutGraph() {
    const edges = new Int32Array(model.m * 2)
    model.edges.forEach((e, k) => ((edges[2 * k] = e.source), (edges[2 * k + 1] = e.target)))
    const nodes = new Int32Array(model.n)
    for (let i = 0; i < model.n; i++) nodes[i] = i
    rings = null
    moreLayer.replaceChildren()
    scene = { x: model.x, y: model.y, nodes, kind: model.kind, edges }
    grid = createHitGrid(scene.x, scene.y, nodes)
  }

  function layoutRings() {
    const focus = model.index.get(state.focus) ?? -1
    rings = ringLayout(model, focus, { hops: hops(), caps })
    const edges = []
    const ringLabels = []
    let lastGroup = null
    for (const j of rings.nodes) {
      const e = rings.parentEdge.get(j)
      if (e === undefined) continue
      edges.push(model.edges[e].source, model.edges[e].target)
    }
    // One predicate label per group on ring 1.
    for (const j of rings.rings[1] ?? []) {
      const e = model.edges[rings.parentEdge.get(j)]
      const group = (e.source === focus ? "" : "← ") + (e.name || e.predicate)
      if (group !== lastGroup) ringLabels.push({ a: focus, b: j, text: truncate(group, 24) })
      lastGroup = group
    }
    scene = { x: rings.x, y: rings.y, nodes: rings.nodes, kind: model.kind, edges: Int32Array.from(edges), circles: rings.radius.slice(1), ringLabels }
    grid = createHitGrid(scene.x, scene.y, rings.nodes)
    moreLayer.replaceChildren(
      ...rings.more.map((n, d) => (n && rings.moreAt[d] ? h(doc, "button", { type: "button", class: "graph-more", "data-ring": d, "aria-label": `Show ${n} more on ring ${d}`, text: `+${n.toLocaleString("en")} more` }) : null)).filter(Boolean)
    )
  }

  function fit() {
    if (!scene) return
    let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity
    for (const i of scene.nodes) {
      const x = scene.x[i], y = scene.y[i]
      if (!Number.isFinite(x) || !Number.isFinite(y)) continue
      if (x < x0) x0 = x
      if (x > x1) x1 = x
      if (y < y0) y0 = y
      if (y > y1) y1 = y
    }
    if (x0 === Infinity) return
    const w = renderer.width, hgt = renderer.height
    // Room for the labels to the right of the nodes: the nodes take the
    // rest, and the view centres nodes plus label room, not the nodes alone.
    const labels = Math.min(220, w * 0.4)
    const k = clamp(Math.min(x1 > x0 || y1 > y0 ? Math.min((w - labels - 16) / (x1 - x0 || 1), Math.max(hgt - 50, hgt * 0.8) / (y1 - y0 || 1)) : 1.5, 4))
    cam = { x: (x0 + x1) / 2 + labels / 2 / k, y: (y0 + y1) / 2, k }
    fitPending = false
    schedule()
  }
  const clamp = (k) => Math.min(ZOOM[1], Math.max(ZOOM[0], k))

  // ---- drawing ----
  function schedule() {
    if (!frame && !destroyed) frame = win.requestAnimationFrame(draw)
  }

  const toScreen = (i) => [(scene.x[i] - cam.x) * cam.k + renderer.width / 2, (scene.y[i] - cam.y) * cam.k + renderer.height / 2]
  const drawn = (i) => i >= 0 && scene && Number.isFinite(scene.x[i])

  function draw() {
    if (frame) win.cancelAnimationFrame(frame)
    frame = 0
    if (destroyed || mode === "schema") return
    const empty = { x: new Float64Array(0), y: new Float64Array(0), nodes: [], kind: new Uint8Array(0), edges: [] }
    const s = scene ?? empty
    const inView = []
    const w = renderer.width, hgt = renderer.height
    if (scene)
      for (const i of s.nodes) {
        const [x, y] = toScreen(i)
        if (x > -20 && y > -20 && x < w + 20 && y < hgt + 20) inView.push(i)
      }
    const always = [selected, hover, ...near, ...matches.slice(0, 50)].filter(drawn)
    if (rings) for (const i of rings.rings.slice(0, 2).flat()) always.push(i)
    const labels = scene ? pickLabels({ always, inView, zoom: cam.k, deg: model.deg, few: rings ? 150 : undefined }) : []
    const strong = []
    const edgeLabels = rings ? [...(s.ringLabels ?? [])] : []
    if (selected >= 0 && scene) {
      const incident = []
      for (let q = model.adjStart[selected]; q < model.adjStart[selected + 1]; q++) {
        const e = model.edges[model.adj[q]]
        if (drawn(e.source) && drawn(e.target)) strong.push(e.source, e.target), incident.push(e)
      }
      if (!rings && incident.length <= 24) for (const e of incident) edgeLabels.push({ a: e.source, b: e.target, text: truncate(e.name || e.predicate, 24) })
    }
    const ms = renderer.draw({ ...s, slot: slots, marks, strong, edgeLabels, selected: drawn(selected) ? selected : -1, hover: drawn(hover) ? hover : -1, matches: matches.filter(drawn), labels, text: (i) => truncate(nameOf(model.nodes[i])) }, cam, colors)
    root.dataset.drawMs = ms.toFixed(2)
    for (const b of moreLayer.children) {
      const at = rings?.moreAt[Number(b.dataset.ring)]
      if (!at) continue
      b.style.left = `${(at[0] - cam.x) * cam.k + w / 2}px`
      b.style.top = `${(at[1] - cam.y) * cam.k + hgt / 2}px`
    }
  }

  function showStatus() {
    if (failed) return
    if (nothing) return void (status.textContent = NO_FILES)
    if (mode === "schema") {
      const m = schema.model
      const f = (v) => v.toLocaleString("en")
      const shown = m?.classes.length ?? 0, total = m?.total ?? 0
      status.textContent = `Showing ${f(shown)} of ${f(total)} classes` + (shown < total ? " · search or Follow cursor reach the others" : "")
      return
    }
    const shown = mode === "focus" ? rings?.nodes.length ?? 0 : model?.n ?? 0
    const total = model?.total.nodes ?? 0
    const f = (v) => v.toLocaleString("en")
    status.textContent = `Showing ${f(shown)} of ${f(total)} nodes` + (mode === "graph" && total > BIG && shown < total ? " · narrow with filters or depth" : "")
  }

  function renderLegend() {
    const entries = []
    if (state.colorBy === "kind") KINDS.forEach((k, slot) => entries.push([slot, k]))
    else if (state.colorBy === "namespace") order.namespaces.slice(0, PALETTE_SIZE).forEach((ns, slot) => entries.push([slot, ns.prefix ? `${ns.prefix}:` : ns.iri]))
    else {
      const root = commonRoot(order.files)
      order.files.slice(0, PALETTE_SIZE).forEach((uri, slot) => entries.push([slot, middleCut(fileLabel(uri, root), LEGEND_CHARS), uri]))
    }
    const used = new Set(slots)
    legend.replaceChildren(...entries.filter(([slot]) => used.has(slot)).map(([slot, text, title]) => h(doc, "span", { class: "graph-key", role: "listitem", title }, h(doc, "span", { class: `graph-swatch graph-c${slot}`, "aria-hidden": "true" }), text)))
    legend.hidden = mode === "schema" || !legend.children.length
  }

  // ---- selection and the info panel ----
  function select(i, { center = false, user = true } = {}) {
    if (!model) return
    selected = i >= 0 && i < model.n ? i : -1
    selId = selected >= 0 ? model.nodes[selected].id : null
    near = selected >= 0 ? neighbours(model, selected, 60) : new Set()
    if (center && drawn(selected)) {
      cam.x = scene.x[selected]
      cam.y = scene.y[selected]
      userMoved = true
      cam.k = Math.max(cam.k, 1.5)
    }
    renderInfo()
    schedule()
    if (user && mode === "graph" && state.filters.depth > 0) {
      clearTimeout(timer)
      timer = setTimeout(load, DEBOUNCE_MS)
    }
  }

  function renderInfo() {
    infoSources = []
    if (mode === "schema") return renderClass()
    root.dataset.info = selected >= 0 && model ? "open" : ""
    if (selected < 0 || !model) {
      info.hidden = true
      info.replaceChildren()
      return
    }
    const d = model.nodes[selected]
    const { out, in: inc } = triplesOf(model, selected)
    const row = (t, dir) =>
      h(doc, "li", {}, h(doc, "button", { type: "button", class: "graph-triple", "data-i": t.node, title: model.nodes[t.node].id },
        h(doc, "span", { class: "graph-pred", text: dir === "out" ? t.name : `← ${t.name}` }), " ",
        h(doc, "span", { class: "graph-obj", text: nameOf(model.nodes[t.node]) })))
    const list = (title, rows, dir) => [
      h(doc, "h4", { class: "graph-info-sec", text: `${title} (${rows.length})` }),
      rows.length ? h(doc, "ul", { class: "graph-triples" }, rows.slice(0, TRIPLES_SHOWN).map((t) => row(t, dir))) : null,
      rows.length > TRIPLES_SHOWN ? h(doc, "p", { class: "graph-none", text: `${rows.length - TRIPLES_SHOWN} more` }) : null,
    ]
    const canReveal = typeof reveal === "function" && d.uri && d.range
    info.replaceChildren(...kids(
      h(doc, "div", { class: "graph-info-hd" },
        h(doc, "h3", { class: "graph-info-name", text: nameOf(d) }),
        h(doc, "span", { class: "graph-badge", text: d.literal ? "literal" : d.kind || "other" }),
        h(doc, "button", { type: "button", class: "graph-x", "data-act": "close", "aria-label": "Close", title: "Close", text: "×" })),
      h(doc, "div", { class: "graph-info-id", text: d.id }),
      d.uri ? h(doc, "div", { class: "graph-info-file", title: d.uri, text: middleCut(fileLabel(d.uri, commonRoot(order.files)), INFO_CHARS) }) : null,
      marks[selected] ? h(doc, "div", { class: `graph-info-diag graph-sev${marks[selected]}`, text: `Has ${SEVERITY[marks[selected]]} diagnostics` }) : null,
      h(doc, "div", { class: "graph-actions" },
        h(doc, "button", { type: "button", class: "graph-btn", "data-act": "reveal", disabled: !canReveal, text: "Go to definition" }),
        mode !== "focus" || state.focus !== d.id ? h(doc, "button", { type: "button", class: "graph-btn", "data-act": "focus", disabled: model.kind[selected] === LITERAL, text: "Focus here" }) : null),
      list("Out", out, "out"),
      list("In", inc, "in")))
    info.hidden = false
  }

  // The class the inspector shows (-1: none): the selected one; narrow, with none selected, the focus class.
  let shownClass = -1
  let narrow = false
  const isNarrow = () => root.clientWidth > 0 && root.clientWidth < NARROW

  // Schema: the selected class in the inspector.
  function renderClass() {
    narrow = isNarrow()
    const i = (shownClass = schema.selected >= 0 ? schema.selected : narrow && !nothing ? schema.focusClass : -1)
    root.dataset.info = i >= 0 && !nothing ? "open" : ""
    if (i < 0 || nothing) {
      info.hidden = true
      info.replaceChildren()
      delete info.dataset.cls
      return
    }
    // A new answer for the same class keeps its open lists and the focused place.
    const id = schema.model.classes[i]?.id
    const same = info.dataset.cls === id
    const open = same ? [...info.querySelectorAll("details[data-key][open]")].map((d) => d.dataset.key) : []
    const focused = same && info.contains(doc.activeElement) ? doc.activeElement.dataset?.key : null
    const loading = askPlaces(i)
    const { nodes, sources } = classInspector(doc, schema.model, i, { lit: schema.lit, canReveal: typeof reveal === "function", loading })
    infoSources = sources
    info.replaceChildren(...nodes)
    info.dataset.cls = id
    for (const d of info.querySelectorAll("details[data-key]")) if (open.includes(d.dataset.key)) d.open = true
    if (focused) info.querySelector(`[data-key="${CSS.escape(focused)}"]`)?.focus()
    info.setAttribute("aria-label", "Selected class")
    info.hidden = false
  }

  // ---- the selected class's places ----
  // An answer lists the places of one class's problems (`examples`): the
  // focus's. Selecting another class with problems asks for its places with
  // the params of the answer shown plus `examples`, so the classes listed
  // stay; the places merge into the model shown and only the inspector is
  // drawn again. One request at a time; an answer for a class no longer
  // selected, or for a model no longer shown, is dropped.
  let shownParams = null
  let placesAsking = false
  // Per model shown: class id -> "asking" or "done".
  const placesOf = new WeakMap()

  /** Ask for class `i`'s places (the one shown) when it needs them; true while they are on their way. */
  function askPlaces(i) {
    const m = schema.model
    const c = m?.classes[i]
    if (!shownParams || !needsPlaces(c)) return false
    let asked = placesOf.get(m)
    if (!asked) placesOf.set(m, (asked = new Map()))
    if (asked.get(c.id) === "done") return false
    if (!asked.has(c.id) && !placesAsking) fetchPlaces(m, c.id, asked)
    return true
  }

  async function fetchPlaces(m, id, asked) {
    placesAsking = true
    asked.set(id, "asking")
    let res = null, failed = false
    try {
      res = await request(SCHEMA_METHOD, { ...shownParams, examples: id })
    } catch {
      failed = true
    }
    placesAsking = false
    if (destroyed) return
    const i = shownClass
    const current = schema.model === m && m.classes[i]?.id === id
    if (!current) asked.delete(id)
    else {
      asked.set(id, "done")
      if (!failed) mergePlaces(m, i, res)
    }
    // Draw the answer, or ask for the class selected meanwhile.
    if (mode === "schema") renderInfo()
  }

  function showSchema(res, o = {}, params = null) {
    // The inspector takes a column when a class is selected: make room
    // before the boxes are fitted into the rest.
    const before = root.dataset.info
    root.dataset.info = schema.selected >= 0 || Number.isInteger(res?.focus) ? "open" : ""
    // The params that make this answer, the focus for `at` (the cursor
    // may have moved on): asked again with `examples`, they list the same.
    const f = Number.isInteger(res?.focus) ? res.classes?.[res.focus] : null
    const focus = Number.isInteger(res?.focusRow) ? f?.rows?.[res.focusRow]?.property : f?.id
    if (params?.at) {
      params = { ...params }
      delete params.at
      if (typeof focus === "string") params.focus = focus
    }
    shownParams = params
    schema.show(res, { imports: state.schema.imports, ...o })
    const guess = root.dataset.info
    runSearch(panel.search, false)
    showStatus()
    renderInfo()
    if (root.dataset.info !== guess || root.dataset.info !== before) schema.resize()
  }

  function openClass(i) {
    const c = schema.model?.classes[i]
    if (c?.uri && c.range && typeof reveal === "function") reveal(c.uri, c.range)
  }

  function openDefinition(i) {
    const d = model?.nodes[i]
    if (d?.uri && d.range && typeof reveal === "function") reveal(d.uri, d.range)
  }

  /** Walk to node `i` in focus mode: it becomes the centre of the rings. */
  function focusOn(i) {
    if (!model || i < 0) return
    selected = i
    selId = model.nodes[i].id
    state.focus = selId
    caps = RING_CAPS.slice()
    fitPending = true
    setMode("focus", true)
  }

  /** Search's choice: select and centre it (graph, schema), walk to it (focus). In Schema `i` is a place in the results. */
  function focusNode(i) {
    if (mode === "schema") return void (schemaFound[i] && pickMatch(schemaFound[i]))
    if (!model || i < 0 || i >= model.n) return
    if (mode === "focus") return focusOn(i)
    select(i, { center: true })
    canvas.focus({ preventScroll: true })
  }

  function runSearch(q, redraw = true) {
    if (mode === "schema") {
      matches = []
      // A new answer (redraw false) leaves the results: each is found by its IRI when chosen.
      if (redraw) schemaSearch(q)
      return
    }
    matches = model ? searchNodes(model, q, 50) : []
    panel.results(matches.slice(0, 8).map((i) => ({ index: i, name: truncate(nameOf(model.nodes[i]), 40), kind: model.kind[i] === LITERAL ? "literal" : KINDS[model.kind[i]] })))
    if (redraw) schedule()
  }

  // ---- Schema search ----
  // The server searches every class, listed or not (`search`, with the
  // params of the answer shown, so a match's `index` is into it): asked
  // 150 ms after the last keystroke, or at once on Enter. One answer per
  // query; an answer for an older query is dropped. A match not listed is
  // asked for as the focus of a new answer.
  let schemaFound = []
  let searchSeq = 0
  let searchTimer = 0
  let searching = null // { q, asked, promise }: the query asked, or about to be

  /** Ask for the classes `q` finds (debounced unless `now`); resolves to the matches shown, null if dropped. */
  function schemaSearch(q, now = false) {
    const s = String(q ?? "").trim()
    if (searching?.q === s && (searching.asked || !now)) return searching.promise
    clearTimeout(searchTimer)
    const my = ++searchSeq
    if (!s) {
      searching = null
      schemaFound = []
      panel.results([])
      return Promise.resolve([])
    }
    let go
    const entry = { q: s, asked: false, promise: null }
    entry.promise = new Promise((resolve) => (go = resolve)).then(async () => {
      entry.asked = true
      const base = shownParams ?? schemaParams(state.filters, { imports: state.schema.imports })
      if (!base) return []
      let res = null
      try {
        res = await request(SCHEMA_METHOD, { ...base, search: s })
      } catch {}
      if (my !== searchSeq || destroyed) return null
      const listed = (i) => ({ id: schema.model.classes[i].id, name: schema.model.classes[i].name, label: schema.model.classes[i].label, index: i })
      // No answer from the server: the classes listed.
      const found = Array.isArray(res?.matches) ? res.matches.filter((m) => m && typeof m.id === "string") : schema.model ? schema.search(s, 8).map(listed) : []
      showMatches(found)
      return schemaFound
    })
    searching = entry
    if (now) go()
    else searchTimer = setTimeout(go, DEBOUNCE_MS)
    return entry.promise
  }

  function showMatches(found) {
    schemaFound = found.slice(0, 20)
    panel.results(schemaFound.map((m, k) => {
      const name = displayName(m.name, m.id)
      const listed = schema.model?.index.has(m.id)
      return { index: k, name: truncate(m.label && m.label !== name ? `${m.label} · ${name}` : name, 40), kind: listed ? "class" : "not shown", title: m.id }
    }))
  }

  /** A match chosen: select it when listed, else ask for an answer centred on it, then select it. */
  async function pickMatch(m) {
    const i = schema.model?.index.get(m.id)
    if (i != null) return schema.select(i, { center: true, focus: true })
    const params = schemaParams(state.filters, { focus: m.id, imports: state.schema.imports })
    if (!params) return
    const res = await ask(params, SCHEMA_METHOD)
    if (!res || mode !== "schema") return
    showSchema(res, { focus: true }, params)
    const j = schema.model.index.get(m.id)
    if (j != null) schema.select(j, { center: true, focus: true })
  }

  // ---- modes ----
  function syncTabs() {
    root.dataset.mode = mode
    for (const t of tabs) {
      const on = t.dataset.mode === mode
      t.setAttribute("aria-selected", String(on))
      t.tabIndex = on ? 0 : -1
    }
    canvas.hidden = mode === "schema"
    moreLayer.hidden = mode !== "focus"
    schemaBox.hidden = mode !== "schema" || nothing
    legend.hidden = mode === "schema" || !legend.children.length
    tip.hidden = true
    info.setAttribute("aria-label", "Selected node")
  }

  function setMode(m, force = false) {
    if (!MODES.includes(m) || (m === mode && !force)) return
    if (m === "focus" && mode !== "focus") (caps = RING_CAPS.slice()), (fitPending = true)
    if (m === "graph" && mode !== "graph") fitPending = true
    mode = m
    state.mode = m
    if (m === "focus" && !force && selId) state.focus = selId
    save()
    rings = null
    scene = null
    syncTabs()
    renderInfo()
    load()
  }

  // ---- events ----
  root.querySelector(".graph-tabs").addEventListener("click", (e) => {
    const t = e.target.closest("[data-mode]")
    if (t) setMode(t.dataset.mode)
  })
  root.querySelector(".graph-tabs").addEventListener("keydown", (e) => {
    const at = tabs.indexOf(e.target)
    if (at < 0 || (e.key !== "ArrowRight" && e.key !== "ArrowLeft")) return
    const next = tabs[(at + (e.key === "ArrowRight" ? 1 : tabs.length - 1)) % tabs.length]
    next.focus()
    setMode(next.dataset.mode)
    e.preventDefault()
  })
  const refit = () => (mode === "schema" ? schema.fit() : ((userMoved = false), fit()))
  fitButton.addEventListener("click", refit)
  info.addEventListener("click", (e) => {
    const b = e.target.closest("[data-act]")
    const act = b?.dataset.act
    if (mode === "schema") {
      if (act === "close") return schema.select(-1)
      if (act === "reveal") return openClass(shownClass)
      if (act === "src") {
        const s = infoSources[Number(b.dataset.src)]
        if (s && typeof reveal === "function") reveal(s.uri, s.range)
        return
      }
      if (act === "row") return schema.select(shownClass, { row: Number(b.dataset.row) })
      if (act === "class") return schema.select(Number(b.dataset.c), { center: true })
      return
    }
    if (act === "close") return select(-1)
    if (act === "reveal") return openDefinition(selected)
    if (act === "focus") return focusOn(selected)
    const t = e.target.closest("[data-i]")
    if (!t) return
    select(Number(t.dataset.i), { center: true })
  })
  // Escape in the inspector: as in the drawing (the legend, then the selection).
  info.addEventListener("keydown", (e) => {
    if (mode !== "schema" || e.key !== "Escape") return
    e.preventDefault()
    schema.escape()
  })
  followButton.addEventListener("click", () => {
    state.follow = !state.follow
    followButton.setAttribute("aria-pressed", String(state.follow))
    save()
    if (!state.follow) {
      clearTimeout(followTimer)
      cursor++
    }
  })
  moreLayer.addEventListener("click", (e) => {
    const b = e.target.closest("[data-ring]")
    if (!b || !rings) return
    const d = Number(b.dataset.ring)
    caps[d] = (caps[d] ?? caps[caps.length - 1]) + RING_CAPS[Math.min(d, RING_CAPS.length - 1)]
    layoutRings()
    showStatus()
    draw()
  })

  // Screen point to the nearest drawn node.
  function hit(e) {
    if (!grid || !scene) return -1
    const b = canvas.getBoundingClientRect()
    const wx = (e.clientX - b.left - renderer.width / 2) / cam.k + cam.x
    const wy = (e.clientY - b.top - renderer.height / 2) / cam.k + cam.y
    return grid.nearest(wx, wy, (HIT_PX + renderer.radius(cam.k)) / cam.k)
  }
  let drag = null
  canvas.addEventListener("pointerdown", (e) => {
    if (e.button !== 0) return
    drag = { x: e.clientX, y: e.clientY, cx: cam.x, cy: cam.y, moved: false }
    canvas.setPointerCapture?.(e.pointerId)
  })
  canvas.addEventListener("pointermove", (e) => {
    if (drag) {
      const dx = e.clientX - drag.x, dy = e.clientY - drag.y
      if (Math.abs(dx) + Math.abs(dy) > 3) drag.moved = true
      if (!drag.moved) return
      cam.x = drag.cx - dx / cam.k
      cam.y = drag.cy - dy / cam.k
      userMoved = true
      tip.hidden = true
      return schedule()
    }
    const i = hit(e)
    if (i === hover) return
    hover = i
    tip.hidden = i < 0
    if (i >= 0) {
      const d = model.nodes[i]
      tip.textContent = d.label && d.name && d.label !== d.name ? `${d.label} · ${d.name}` : nameOf(d)
      const b = canvas.getBoundingClientRect()
      tip.style.left = `${e.clientX - b.left + 12}px`
      tip.style.top = `${e.clientY - b.top + 12}px`
    }
    schedule()
  })
  canvas.addEventListener("pointerup", (e) => {
    const was = drag
    drag = null
    if (was && !was.moved) select(hit(e))
  })
  canvas.addEventListener("pointercancel", () => (drag = null))
  canvas.addEventListener("pointerleave", () => {
    if (hover < 0) return
    hover = -1
    tip.hidden = true
    schedule()
  })
  canvas.addEventListener("dblclick", (e) => {
    const i = hit(e)
    if (i < 0) return
    if (mode === "focus") focusOn(i)
    else openDefinition(i)
  })
  canvas.addEventListener("wheel", (e) => {
    e.preventDefault()
    const b = canvas.getBoundingClientRect()
    zoomAt(Math.exp(-e.deltaY * 0.0015), e.clientX - b.left, e.clientY - b.top)
  }, { passive: false })
  function zoomAt(factor, sx = renderer.width / 2, sy = renderer.height / 2) {
    const mx = sx - renderer.width / 2, my = sy - renderer.height / 2
    const wx = mx / cam.k + cam.x, wy = my / cam.k + cam.y
    userMoved = true
    cam.k = clamp(cam.k * factor)
    cam.x = wx - mx / cam.k
    cam.y = wy - my / cam.k
    schedule()
  }
  canvas.addEventListener("keydown", (e) => {
    const step = 60 / cam.k
    if (e.key.startsWith("Arrow")) userMoved = true
    if (e.key === "ArrowLeft") cam.x -= step
    else if (e.key === "ArrowRight") cam.x += step
    else if (e.key === "ArrowUp") cam.y -= step
    else if (e.key === "ArrowDown") cam.y += step
    else if (e.key === "+" || e.key === "=") return e.preventDefault(), zoomAt(1.25)
    else if (e.key === "-") return e.preventDefault(), zoomAt(0.8)
    else if (e.key === "0") return e.preventDefault(), refit()
    else if (e.key === "Enter") return e.preventDefault(), selected >= 0 && (mode === "focus" && e.shiftKey ? focusOn(selected) : openDefinition(selected))
    else if (e.key === "Escape") return select(-1)
    else return
    e.preventDefault()
    schedule()
  })

  // ---- size and theme ----
  function resize() {
    if (destroyed) return
    const b = stage.getBoundingClientRect()
    renderer.size(b.width, b.height, win.devicePixelRatio || 1)
    if ((fitPending || !userMoved) && scene) fit()
    if (mode === "schema") {
      schema.resize()
      if (isNarrow() !== narrow) renderInfo()
    }
    draw()
  }
  const observer = win.ResizeObserver ? new win.ResizeObserver(() => resize()) : null
  observer?.observe(stage)

  function applyTheme(t) {
    if (!t || typeof t !== "object") return
    for (const [k, v] of Object.entries(t)) if (k.startsWith("--graph-") && typeof v === "string") root.style.setProperty(k, v)
  }

  // ---- Follow cursor ----
  // `cursor` counts the cursor moves: an answer asked for an older one is dropped.
  let cursor = 0
  let followAt = null
  let followTimer = 0
  let following = false
  let followAgain = false
  const visible = () => doc.visibilityState !== "hidden" && root.isConnected && root.getClientRects().length > 0
  const validAt = (at) => typeof at?.uri === "string" && Number.isInteger(at.position?.line) && Number.isInteger(at.position?.character)

  function follow(at) {
    cursor++
    clearTimeout(followTimer)
    if (destroyed || !state.follow || !validAt(at) || !visible()) return
    followAt = { uri: at.uri, position: { line: at.position.line, character: at.position.character } }
    followTimer = setTimeout(followNow, DEBOUNCE_MS)
  }

  async function followNow() {
    if (destroyed || !state.follow || nothing || !visible()) return
    if (following) return void (followAgain = true)
    const my = cursor, at = followAt, m = mode
    let params, method = METHOD
    if (m === "schema") (params = schemaParams(state.filters, { at, imports: state.schema.imports })), (method = SCHEMA_METHOD)
    else if (m === "focus") params = toParams(state.filters, { focus: state.focus, depth: hops(), iterations: 0 })
    else params = toParams({ ...state.filters, depth: 0 }, { iterations: 0, maxNodes: 1 })
    if (!params) return
    if (m !== "schema") params.at = at
    if (m === "graph") params.depth = 0
    following = true
    try {
      const res = await request(method, params)
      if (my === cursor && m === mode && !destroyed && state.follow && !nothing) followed(m, res, params)
    } catch {
      // A failed follow leaves the view as it is.
    } finally {
      following = false
      if (followAgain) {
        followAgain = false
        if (my !== cursor) followNow()
      }
    }
  }

  // An answer for the latest cursor. null: no term there, the view stays.
  // A turtle/graph answer lists the term's node first (breadth-first from
  // the focus the server resolved `at` to).
  function followed(m, res, params) {
    if (m === "schema") {
      // No term there: the class stays, its lit row goes.
      if (!Number.isInteger(res?.focus) || res.focus < 0) {
        if (schema.lit >= 0) schema.select(schema.selected)
        return
      }
      const t0 = win.performance.now()
      showSchema(res, { focus: true }, params)
      root.dataset.followMs = (win.performance.now() - t0).toFixed(2)
      root.dataset.followRebuilt = String(schema.rebuilt)
      return
    }
    if (res == null) return
    const id = res.nodes?.[0]?.id
    if (typeof id !== "string") return
    if (m === "focus") {
      if (!res.nodes?.some((d) => d?.id === id)) return
      state.focus = id
      selId = id
      save()
      setModel(res)
      caps = RING_CAPS.slice()
      layoutRings()
      fit()
      showStatus()
      renderInfo()
      draw()
      return
    }
    const i = model?.index.get(id)
    if (i !== undefined) select(i, { center: true, user: false })
  }

  // ---- start ----
  async function start() {
    syncTabs()
    resize()
    if (!state.mode && !selectsNothing(state.filters)) {
      // Nothing chosen: a first answer without layout decides the mode.
      const res = await ask(toParams(state.filters, { iterations: 0 }))
      if (!res) return
      const choice = chooseDefault(res)
      mode = choice.mode
      if (choice.mode === "focus") state.focus = choice.focus
      syncTabs()
    }
    await load()
  }
  const started = start()

  return {
    /** Ask again (a file changed), keeping positions, filters and the selection. */
    async refresh() {
      await started
      await load({ changed: true })
    },
    /** The container changed size. */
    resize,
    destroy() {
      destroyed = true
      clearTimeout(timer)
      clearTimeout(followTimer)
      clearTimeout(searchTimer)
      if (frame) win.cancelAnimationFrame(frame)
      observer?.disconnect()
      container.replaceChildren()
    },
    follow,
    /** Re-read the colours (after a theme change), optionally setting `--graph-…` variables first. */
    setTheme(t) {
      applyTheme(t)
      colors = readColors(root)
      draw()
    },
  }
}

function readStore(store) {
  try {
    const v = store?.get?.(STORE_KEY)
    return typeof v === "string" ? JSON.parse(v) : v
  } catch {
    return null
  }
}

