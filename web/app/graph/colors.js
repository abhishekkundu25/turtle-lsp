// Colours: which palette slot each node takes (by file, kind or namespace),
// and the theme's colours read from the --graph-… CSS variables (graph.css
// gives each a default; a host may set them).

import { LITERAL } from "./model.js"

export const PALETTE_SIZE = 8
/** The slot of a node with no file, namespace or a literal. */
export const NEUTRAL = PALETTE_SIZE

/** The namespace facet that `id` falls in: the longest facet IRI it starts with; -1 when none. */
export function namespaceOf(namespaces, id) {
  let best = -1
  namespaces.forEach((ns, k) => {
    if (id.startsWith(ns.iri) && (best < 0 || ns.iri.length > namespaces[best].iri.length)) best = k
  })
  return best
}

/**
 * Each node's palette slot (0 … PALETTE_SIZE-1, or NEUTRAL).
 * @param {ReturnType<import("./model.js").buildModel>} model
 * @param {"file" | "kind" | "namespace"} by
 * @param {{ files: {uri: string}[], namespaces: {iri: string}[] }} facets keeps slots stable across answers
 */
export function colorSlots(model, by, facets = model.facets) {
  const out = new Uint8Array(model.n)
  const files = new Map(facets.files.map((f, k) => [f.uri, k]))
  for (let i = 0; i < model.n; i++) {
    const d = model.nodes[i]
    let k = -1
    if (model.kind[i] === LITERAL) k = -1
    else if (by === "kind") k = model.kind[i]
    else if (by === "namespace") k = namespaceOf(facets.namespaces, d.id)
    else k = files.get(d.uri) ?? -1
    out[i] = k < 0 ? NEUTRAL : k % PALETTE_SIZE
  }
  return out
}

const TOKENS = ["bg", "fg", "muted", "edge", "focus", "match", "error", "warning", "info", "neutral"]

/** The theme's colours as the canvas needs them (resolved rgb strings), read through a probe inside `el`. */
export function readColors(el) {
  const probe = el.ownerDocument.createElement("span")
  probe.className = "graph-probe"
  el.append(probe)
  const read = (name) => {
    probe.style.color = ""
    probe.style.color = `var(--graph-${name})`
    return getComputedStyle(probe).color
  }
  const colors = Object.fromEntries(TOKENS.map((t) => [t, read(t)]))
  colors.palette = Array.from({ length: PALETTE_SIZE }, (_, k) => read(`c${k}`))
  colors.palette.push(colors.neutral)
  probe.remove()
  return colors
}
