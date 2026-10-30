// Mode C's layout, direct (no force layout): the focus rings (the focus in
// the middle, nodes N hops away on ring N, each ring capped with a "+N
// more"). Pure: no DOM.

import { nameOf, other, SUBCLASS } from "./model.js"

/** Nodes per ring: the focus, then rings 1, 2, 3; deeper rings take the last. */
export const RING_CAPS = [1, 40, 120, 200]

/**
 * The focus rings around node `focus`.
 * @param {ReturnType<import("./model.js").buildModel>} model
 * @param {number} focus a node index
 * @param {object} [o]
 * @param {number} [o.hops] rings
 * @param {number[]} [o.caps] nodes per ring (index = ring)
 * @param {number} [o.step] distance between rings
 * @param {number} [o.spacing] the least arc between two nodes on a ring
 * @returns {{ nodes: number[], x: Float64Array, y: Float64Array, rings: number[][], more: number[], radius: number[], moreAt: ([number, number] | null)[], parentEdge: Map<number, number> }}
 */
export function ringLayout(model, focus, { hops = 2, caps = RING_CAPS, step = 170, spacing = 28 } = {}) {
  const x = new Float64Array(model.n).fill(NaN)
  const y = new Float64Array(model.n).fill(NaN)
  const empty = { nodes: [], x, y, rings: [], more: [], radius: [], moreAt: [], parentEdge: new Map() }
  if (!(focus >= 0 && focus < model.n)) return empty
  const cap = (d) => caps[Math.min(d, caps.length - 1)]
  const seen = new Uint8Array(model.n)
  seen[focus] = 1
  const parentEdge = new Map()
  const rings = [[focus]]
  const more = [0]
  let frontier = [focus]
  for (let d = 1; d <= hops && frontier.length; d++) {
    const next = []
    let over = 0
    for (const i of frontier)
      for (let q = model.adjStart[i]; q < model.adjStart[i + 1]; q++) {
        const e = model.adj[q]
        const j = other(model, e, i)
        if (seen[j]) continue
        seen[j] = 1
        if (next.length >= cap(d)) {
          over++
          continue
        }
        parentEdge.set(j, e)
        next.push(j)
      }
    if (!next.length && !over) break
    rings.push(next)
    more.push(over)
    frontier = next
  }
  // Ring 1 grouped by predicate (outgoing first), then by name; deeper rings follow their parent's angle.
  const pred = (j) => {
    const e = model.edges[parentEdge.get(j)]
    return (e.source === focus ? "0" : "1") + (e.name || e.predicate || "")
  }
  if (rings[1]) rings[1].sort((a, b) => pred(a).localeCompare(pred(b)) || nameOf(model.nodes[a]).localeCompare(nameOf(model.nodes[b])) || a - b)
  const angle = new Float64Array(model.n)
  const radius = [0]
  const moreAt = [null]
  x[focus] = 0
  y[focus] = 0
  for (let d = 1; d < rings.length; d++) {
    const list = rings[d]
    if (d > 1) {
      const order = new Map(list.map((j, k) => [j, k]))
      const parentAngle = (j) => angle[other(model, parentEdge.get(j), j)]
      list.sort((a, b) => parentAngle(a) - parentAngle(b) || order.get(a) - order.get(b))
    }
    const slots = list.length + (more[d] ? 1 : 0)
    const r = Math.max(step * d, radius[d - 1] + step, (slots * spacing) / (2 * Math.PI))
    radius.push(r)
    const at = (k) => (k / Math.max(1, slots)) * Math.PI * 2 - Math.PI / 2
    list.forEach((j, k) => {
      angle[j] = at(k)
      x[j] = Math.cos(angle[j]) * r
      y[j] = Math.sin(angle[j]) * r
    })
    moreAt.push(more[d] ? [Math.cos(at(list.length)) * r, Math.sin(at(list.length)) * r] : null)
  }
  return { nodes: rings.flat(), x, y, rings, more, radius, moreAt, parentEdge }
}
