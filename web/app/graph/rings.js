// Mode C's two layouts, both direct (no force layout): the focus rings (the
// focus in the middle, nodes N hops away on ring N, each ring capped with a
// "+N more") and the class hierarchy (rdfs:subClassOf as a tree). Pure: no DOM.

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

/**
 * The class hierarchy from `predicate` edges (child → parent). Roots: nodes
 * with no parent that have children or are classes, largest first; nodes
 * only in cycles get a root too, so every node of the hierarchy is reachable.
 * @returns {{ roots: number[], children: Map<number, number[]>, parents: Map<number, number[]>, size: Int32Array }}
 *   `size`: the nodes under each (a node under two parents counts twice)
 */
export function buildHierarchy(model, predicate = SUBCLASS) {
  const children = new Map()
  const parents = new Map()
  const add = (map, k, v) => {
    const l = map.get(k)
    if (!l) map.set(k, [v])
    else if (!l.includes(v)) l.push(v)
  }
  for (const e of model.edges) {
    if (e.predicate !== predicate || e.source === e.target) continue
    add(children, e.target, e.source)
    add(parents, e.source, e.target)
  }
  const byName = (a, b) => nameOf(model.nodes[a]).localeCompare(nameOf(model.nodes[b])) || a - b
  for (const l of children.values()) l.sort(byName)
  // Subtree sizes, iteratively; an edge back into the current path counts nothing.
  const size = new Int32Array(model.n)
  const state = new Uint8Array(model.n) // 0 new, 1 on the path, 2 done
  for (let s = 0; s < model.n; s++) {
    if (state[s]) continue
    const stack = [[s, 0]]
    state[s] = 1
    while (stack.length) {
      const top = stack[stack.length - 1]
      const kids = children.get(top[0]) ?? []
      if (top[1] < kids.length) {
        const c = kids[top[1]++]
        if (!state[c]) (state[c] = 1), stack.push([c, 0])
        continue
      }
      let total = 0
      for (const c of kids) if (state[c] === 2) total += 1 + size[c]
      size[top[0]] = total
      state[top[0]] = 2
      stack.pop()
    }
  }
  const roots = []
  for (let i = 0; i < model.n; i++) if (!parents.has(i) && (children.has(i) || model.kind[i] === 0)) roots.push(i)
  // Cycles with no way in: the first node of each becomes a root.
  const reached = new Uint8Array(model.n)
  const reach = (r) => {
    const stack = [r]
    reached[r] = 1
    while (stack.length) for (const c of children.get(stack.pop()) ?? []) if (!reached[c]) (reached[c] = 1), stack.push(c)
  }
  roots.forEach(reach)
  for (const i of [...children.keys()].sort((a, b) => a - b)) if (!reached[i]) roots.push(i), reach(i)
  roots.sort((a, b) => size[b] - size[a] || byName(a, b))
  return { roots, children, parents, size }
}

/** The path from a root down to `i` (first parent each step), for revealing a node in the tree. */
export function pathTo(hierarchy, i) {
  const path = [i]
  const seen = new Set(path)
  for (let p = hierarchy.parents.get(i)?.[0]; p !== undefined && !seen.has(p); p = hierarchy.parents.get(p)?.[0]) path.unshift(p), seen.add(p)
  return path
}
