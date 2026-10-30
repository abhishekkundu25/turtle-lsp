// The view's state (filters, mode, colouring, Follow cursor, the Schema
// toggles) and the `turtle/graph` and `turtle/schema` request params made
// from it. State read back from the store is checked here, once;
// what leaves is always a valid request. Pure: no DOM.

import { KINDS } from "./model.js"

export const MODES = ["graph", "focus", "schema"]
export const COLOR_BY = ["file", "kind", "namespace"]
export const CAPS = [500, 2000, 5000, 20000]
export const DEPTHS = [0, 1, 2, 3, 4]
/** Layout steps for a first layout, and for continuing one from earlier positions. */
export const FIRST_ITERATIONS = 300
export const CONTINUE_ITERATIONS = 100
/** Schema asks for this many classes at most (the server's default). */
export const SCHEMA_CLASSES = 300

/**
 * Nothing chosen yet: rdf:type edges and literals hidden, 2,000 nodes, the mode decided on the first answer.
 * `filters.files`: null is every file, a list is those files, an empty list is none.
 */
export function defaultState() {
  return {
    mode: null,
    focus: null,
    colorBy: "file",
    // Follow cursor: off until the user turns it on.
    follow: false,
    // Schema: "Problems only", and imported classes listed one by one.
    schema: { problems: false, imports: false },
    filters: {
      files: null,
      kinds: null,
      namespaces: { include: [], exclude: [] },
      predicates: { include: [], exclude: [] },
      literals: false,
      types: false,
      depth: 0,
      maxNodes: 2000,
    },
  }
}

const strings = (v) => (Array.isArray(v) ? [...new Set(v.filter((s) => typeof s === "string"))] : [])
const pair = (v) => ({ include: strings(v?.include), exclude: strings(v?.exclude) })

/** A state from whatever the store held: unknown or broken fields take their default. */
/** `raw` (a stored state) checked; a node cap over `maxCap` falls back to the default. */
export function normalizeState(raw, maxCap = CAPS[CAPS.length - 1]) {
  const s = defaultState()
  s.filters.maxNodes = Math.min(s.filters.maxNodes, maxCap)
  if (!raw || typeof raw !== "object") return s
  // Schema replaced the Hierarchy mode: a state stored before opens Schema.
  const mode = raw.mode === "hierarchy" ? "schema" : raw.mode
  if (MODES.includes(mode)) s.mode = mode
  s.follow = raw.follow === true
  s.schema.problems = raw.schema?.problems === true
  s.schema.imports = raw.schema?.imports === true
  if (typeof raw.focus === "string") s.focus = raw.focus
  if (COLOR_BY.includes(raw.colorBy)) s.colorBy = raw.colorBy
  const f = raw.filters
  if (!f || typeof f !== "object") return s
  // A stored empty list is "none"; a list that held only broken entries is broken, so every file.
  if (Array.isArray(f.files) && (!f.files.length || strings(f.files).length)) s.filters.files = strings(f.files)
  if (Array.isArray(f.kinds)) {
    const k = strings(f.kinds).filter((v) => KINDS.includes(v))
    s.filters.kinds = k.length === KINDS.length ? null : k
  }
  s.filters.namespaces = pair(f.namespaces)
  s.filters.predicates = pair(f.predicates)
  s.filters.literals = f.literals === true
  s.filters.types = f.types === true
  if (DEPTHS.includes(f.depth)) s.filters.depth = f.depth
  if (CAPS.includes(f.maxNodes) && f.maxNodes <= maxCap) s.filters.maxNodes = f.maxNodes
  return s
}

const round = (v) => Math.round(v * 100) / 100

/** No file is selected: there is nothing to ask for. */
export const selectsNothing = (filters) => Array.isArray(filters?.files) && filters.files.length === 0

/**
 * The `turtle/graph` params for `filters`; null when they select no file
 * (an empty `files` would read as no filter to a server).
 * @param {object} filters
 * @param {object} [o]
 * @param {string | null} [o.focus] a node id; sent only with a depth above 0
 * @param {number} [o.depth] overrides `filters.depth`
 * @param {Map<string, [number, number]>} [o.positions] earlier positions, by node id
 * @param {number} [o.iterations] layout steps (0: no layout)
 * @param {number} [o.maxNodes] overrides `filters.maxNodes`
 */
export function toParams(filters, { focus = null, depth, positions, iterations = FIRST_ITERATIONS, maxNodes } = {}) {
  if (selectsNothing(filters)) return null
  const p = { literals: filters.literals === true, types: filters.types === true, maxNodes: maxNodes ?? filters.maxNodes }
  if (filters.files) p.files = [...filters.files]
  if (filters.kinds) p.kinds = [...filters.kinds]
  for (const key of ["namespaces", "predicates"]) {
    const v = filters[key]
    if (v?.include?.length || v?.exclude?.length) p[key] = { include: [...v.include], exclude: [...v.exclude] }
  }
  const d = depth ?? filters.depth
  if (focus && d > 0) {
    p.focus = focus
    p.depth = d
  }
  p.layout = { iterations }
  if (positions?.size) {
    const out = {}
    for (const [id, [x, y]] of positions) out[id] = [round(x), round(y)]
    p.layout.positions = out
  }
  return p
}

/**
 * The `turtle/schema` params: the Files filter, the focus class, the class
 * cap and whether imported classes are listed; null when no file is selected.
 * @param {object} filters
 * @param {object} [o]
 * @param {string | null} [o.focus] a class or property IRI
 * @param {{ uri: string, position: { line: number, character: number } } | null} [o.at] the term at a document position (Follow cursor)
 * @param {boolean} [o.imports]
 */
export function schemaParams(filters, { focus = null, at = null, imports = false } = {}) {
  if (selectsNothing(filters)) return null
  const p = { maxClasses: SCHEMA_CLASSES, imports: imports === true }
  if (filters.files) p.files = [...filters.files]
  if (focus) p.focus = focus
  if (at) p.at = at
  return p
}

/** The params that ask for the facets alone (no nodes), whatever files are selected: the files to choose from. */
export const facetParams = (filters) => toParams({ ...filters, files: null, depth: 0 }, { iterations: 0, maxNodes: 0 })

/** Set `value`'s rule in an include/exclude pair: "include", "exclude" or "" (neither). */
export function setRule(pairValue, value, rule) {
  pairValue.include = pairValue.include.filter((v) => v !== value)
  pairValue.exclude = pairValue.exclude.filter((v) => v !== value)
  if (rule === "include") pairValue.include.push(value)
  if (rule === "exclude") pairValue.exclude.push(value)
}

/** The rule `value` has in a pair. */
export const ruleOf = (pairValue, value) => (pairValue.include.includes(value) ? "include" : pairValue.exclude.includes(value) ? "exclude" : "")
