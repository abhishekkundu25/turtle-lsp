// The graph as the view holds it: one `turtle/graph` response turned into
// arrays and an undirected adjacency, plus the questions the view asks of it
// (a node's triples, search, the default mode, diagnostics on a node). Pure:
// no DOM, so the tests run it in Node.

export const KINDS = ["class", "property", "individual", "shape", "concept", "other"]
/** Kind code of a literal node (after the six of KINDS). */
export const LITERAL = 6
export const RDF_TYPE = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type"
export const SUBCLASS = "http://www.w3.org/2000/01/rdf-schema#subClassOf"
/** Over this many nodes (after the filters) the view opens in focus mode. */
export const BIG = 2000

/** What a node is called on screen: its label, else its prefixed name, else its id. */
export const nameOf = (node) => node?.label || node?.name || node?.id || ""

/**
 * The response as arrays. Edges that name a node outside `nodes` are dropped;
 * a missing or short `positions` leaves the node at the origin.
 * @param {any} res a `turtle/graph` response
 */
export function buildModel(res) {
  const nodes = Array.isArray(res?.nodes) ? res.nodes.filter((d) => d && typeof d.id === "string") : []
  const n = nodes.length
  const ok = (v) => Number.isInteger(v) && v >= 0 && v < n
  const edges = (Array.isArray(res?.edges) ? res.edges : []).filter((e) => e && ok(e.source) && ok(e.target))
  const m = edges.length
  const index = new Map()
  nodes.forEach((d, i) => index.set(d.id, i))
  const x = new Float64Array(n)
  const y = new Float64Array(n)
  const p = Array.isArray(res?.positions) ? res.positions : []
  for (let i = 0; i < n; i++) {
    x[i] = Number.isFinite(p[2 * i]) ? p[2 * i] : 0
    y[i] = Number.isFinite(p[2 * i + 1]) ? p[2 * i + 1] : 0
  }
  const kind = new Uint8Array(n)
  for (let i = 0; i < n; i++) {
    const k = KINDS.indexOf(nodes[i].kind)
    kind[i] = nodes[i].literal ? LITERAL : k < 0 ? 5 : k
  }
  const deg = new Int32Array(n)
  for (const e of edges) deg[e.source]++, deg[e.target]++
  const adjStart = new Int32Array(n + 1)
  for (let i = 0; i < n; i++) adjStart[i + 1] = adjStart[i] + deg[i]
  const adj = new Int32Array(2 * m)
  const fill = adjStart.slice(0, n)
  edges.forEach((e, k) => ((adj[fill[e.source]++] = k), (adj[fill[e.target]++] = k)))
  const total = {
    nodes: Number.isFinite(res?.total?.nodes) ? res.total.nodes : n,
    edges: Number.isFinite(res?.total?.edges) ? res.total.edges : m,
  }
  const f = res?.facets ?? {}
  const facets = {
    files: Array.isArray(f.files) ? f.files.filter((v) => typeof v?.uri === "string") : [],
    namespaces: Array.isArray(f.namespaces) ? f.namespaces.filter((v) => typeof v?.iri === "string") : [],
    predicates: Array.isArray(f.predicates) ? f.predicates.filter((v) => typeof v?.iri === "string") : [],
  }
  return { nodes, edges, n, m, index, x, y, kind, deg, adjStart, adj, total, facets }
}

/** The node at the other end of edge `e` from `i`. */
export const other = (model, e, i) => (model.edges[e].source === i ? model.edges[e].target : model.edges[e].source)

/** The distinct neighbours of node `i`, at most `limit`. */
export function neighbours(model, i, limit = Infinity) {
  const out = new Set()
  for (let q = model.adjStart[i]; q < model.adjStart[i + 1] && out.size < limit; q++) {
    const j = other(model, model.adj[q], i)
    if (j !== i) out.add(j)
  }
  return out
}

/** Node `i`'s triples: `out` where it is the subject, `in` where it is the object; each `{ predicate, name, node }`. */
export function triplesOf(model, i) {
  const out = []
  const inc = []
  for (let q = model.adjStart[i]; q < model.adjStart[i + 1]; q++) {
    const e = model.edges[model.adj[q]]
    const row = (node) => ({ predicate: e.predicate, name: e.name || e.predicate, node })
    if (e.source === i) out.push(row(e.target))
    // A self loop is listed once, as outgoing.
    else inc.push(row(e.source))
  }
  return { out, in: inc }
}

/** Nodes whose label, name or id contains `query` (case-insensitive): exact matches first, then prefixes, then by degree. */
export function searchNodes(model, query, limit = 20) {
  const q = String(query ?? "").trim().toLowerCase()
  if (!q || !model) return []
  const hits = []
  for (let i = 0; i < model.n; i++) {
    const d = model.nodes[i]
    const keys = [d.label, d.name, d.id].filter(Boolean).map((s) => String(s).toLowerCase())
    let rank = -1
    for (const k of keys) rank = Math.max(rank, k === q ? 2 : k.startsWith(q) ? 1 : k.includes(q) ? 0 : -1)
    if (rank >= 0) hits.push([rank, i])
  }
  hits.sort((a, b) => b[0] - a[0] || model.deg[b[1]] - model.deg[a[1]] || a[1] - b[1])
  return hits.slice(0, limit).map((h) => h[1])
}

/** The best-connected node, a class if there is one (ties by id); -1 when empty. */
export function bestNode(model) {
  let best = -1
  for (let i = 0; i < model.n; i++) {
    if (best < 0) {
      best = i
      continue
    }
    const ci = model.kind[i] === 0, cb = model.kind[best] === 0
    if (ci !== cb) {
      if (ci) best = i
      continue
    }
    if (model.deg[i] > model.deg[best] || (model.deg[i] === model.deg[best] && model.nodes[i].id < model.nodes[best].id)) best = i
  }
  return best
}

/**
 * The mode to open in when nothing was chosen: over BIG nodes, focus mode on
 * the best-connected class with 2 hops; else the graph.
 * @returns {{ mode: "graph" } | { mode: "focus", focus: string, depth: number }}
 */
export function chooseDefault(res, threshold = BIG) {
  const model = buildModel(res)
  if (model.total.nodes <= threshold) return { mode: "graph" }
  const best = bestNode(model)
  return best < 0 ? { mode: "graph" } : { mode: "focus", focus: model.nodes[best].id, depth: 2 }
}

const before = (a, b) => a.line < b.line || (a.line === b.line && a.character <= b.character)
const validRange = (r) => r && Number.isFinite(r.start?.line) && Number.isFinite(r.end?.line)
/** Whether two LSP ranges share a position (touching counts). */
export const overlaps = (a, b) => validRange(a) && validRange(b) && before(a.start, b.end) && before(b.start, a.end)

/**
 * The worst diagnostic severity on each node (1 error … 4 hint; 0 none): a
 * diagnostic of the node's document whose range overlaps the node's range.
 * `diagnostics(uri)` is asked once per document.
 */
export function diagnosticMarks(nodes, diagnostics) {
  const marks = new Uint8Array(nodes.length)
  if (typeof diagnostics !== "function") return marks
  const byUri = new Map()
  nodes.forEach((d, i) => {
    if (!d.uri || !validRange(d.range)) return
    let list = byUri.get(d.uri)
    if (list === undefined) {
      try {
        list = diagnostics(d.uri)
      } catch {
        list = null
      }
      list = Array.isArray(list) ? list : []
      byUri.set(d.uri, list)
    }
    for (const diag of list) {
      if (!overlaps(diag?.range, d.range)) continue
      const s = Number.isInteger(diag.severity) && diag.severity >= 1 && diag.severity <= 4 ? diag.severity : 1
      if (!marks[i] || s < marks[i]) marks[i] = s
    }
  })
  return marks
}

/** A short name for a file URI: its last two path segments, decoded. */
/** The folder every one of `uris` is in, with its trailing '/' ("" when they share none). */
export function commonRoot(uris) {
  if (!uris.length) return ""
  let root = uris[0].slice(0, uris[0].lastIndexOf("/") + 1)
  for (const u of uris) while (root && !u.startsWith(root)) root = root.slice(0, root.lastIndexOf("/", root.length - 2) + 1)
  return /^[a-z]+:\/*$/i.test(root) ? "" : root
}

/** A file's name for the panel: its path below `root`, else its last two path segments. */
export function fileLabel(uri, root = "") {
  const path = String(uri).startsWith(root) ? String(uri).slice(root.length) : String(uri)
  const parts = path.replace(/^[a-z]+:\/*/i, "").split("/").filter(Boolean)
  const tail = root ? parts.join("/") : parts.slice(-2).join("/")
  try {
    return decodeURIComponent(tail)
  } catch {
    return tail
  }
}
