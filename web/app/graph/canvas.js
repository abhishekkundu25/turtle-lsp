// The 2D canvas renderer: one frame of a scene (positions, edges, styles)
// under a camera, sized for the device pixel ratio. Edges go in one path,
// nodes in one path per colour, each kind its own shape. Nothing here
// decides what to draw; the view hands it a scene.

/**
 * @typedef {object} Scene
 * @property {Float64Array} x world positions, by node index
 * @property {Float64Array} y
 * @property {ArrayLike<number>} nodes the node indices to draw
 * @property {Uint8Array} kind by node index (model.js KINDS, 6 literal)
 * @property {Uint8Array} slot palette slot by node index
 * @property {Int32Array | number[]} edges node index pairs, flat
 * @property {Int32Array | number[]} [strong] edges drawn in the focus colour, flat
 * @property {{ a: number, b: number, text: string }[]} [edgeLabels]
 * @property {number[]} [circles] radii of guide circles around the origin (focus rings)
 * @property {number} selected node index or -1
 * @property {number} hover node index or -1
 * @property {number[]} matches search matches
 * @property {Uint8Array} marks diagnostic severity by node index (0 none)
 * @property {number[]} labels node indices to label
 * @property {(i: number) => string} text a node's label
 */

const TAU = Math.PI * 2

export function createRenderer(canvas) {
  const g = canvas.getContext("2d")
  let w = 1, h = 1, dpr = 1

  /** Match the canvas to its box (CSS pixels) and the device pixel ratio. */
  function size(width, height, ratio = 1) {
    w = Math.max(1, width)
    h = Math.max(1, height)
    dpr = ratio > 0 ? ratio : 1
    canvas.width = Math.round(w * dpr)
    canvas.height = Math.round(h * dpr)
    canvas.style.width = `${w}px`
    canvas.style.height = `${h}px`
  }

  /** The node radius at zoom `k`, in CSS pixels. */
  const radius = (k) => Math.max(2.5, Math.min(9, 4.5 * Math.sqrt(k)))

  function shape(kind, x, y, r) {
    switch (kind) {
      case 0: // class
        g.moveTo(x + r, y)
        g.arc(x, y, r, 0, TAU)
        break
      case 1: // property: diamond
        g.moveTo(x + r, y)
        g.lineTo(x, y + r)
        g.lineTo(x - r, y)
        g.lineTo(x, y - r)
        g.closePath()
        break
      case 3: // shape: square
        g.rect(x - r * 0.85, y - r * 0.85, r * 1.7, r * 1.7)
        break
      case 4: // concept: triangle
        g.moveTo(x, y - r)
        g.lineTo(x + r, y + r * 0.8)
        g.lineTo(x - r, y + r * 0.8)
        g.closePath()
        break
      case 6: // literal: small square
        g.rect(x - r * 0.45, y - r * 0.45, r * 0.9, r * 0.9)
        break
      default: // individual, other: smaller circles
        g.moveTo(x + r * 0.65, y)
        g.arc(x, y, r * (kind === 2 ? 0.75 : 0.6), 0, TAU)
    }
  }

  /**
   * Draw `s` under the camera `cam` ({ x, y, k }: the world point at the
   * centre and the zoom) in `colors` (colors.js readColors). Returns the time taken, ms.
   */
  function draw(s, cam, colors) {
    const t0 = performance.now()
    g.setTransform(dpr, 0, 0, dpr, 0, 0)
    g.fillStyle = colors.bg
    g.fillRect(0, 0, w, h)
    const k = cam.k, ox = w / 2 - cam.x * k, oy = h / 2 - cam.y * k
    const X = (i) => s.x[i] * k + ox
    const Y = (i) => s.y[i] * k + oy
    if (s.circles?.length) {
      g.strokeStyle = colors.edge
      g.globalAlpha = 0.35
      g.setLineDash([4, 4])
      g.beginPath()
      for (const r of s.circles) g.moveTo(ox + r * k, oy), g.arc(ox, oy, r * k, 0, TAU)
      g.stroke()
      g.setLineDash([])
    }
    const line = (list) => {
      g.beginPath()
      for (let q = 0; q < list.length; q += 2) g.moveTo(X(list[q]), Y(list[q])), g.lineTo(X(list[q + 1]), Y(list[q + 1]))
      g.stroke()
    }
    g.lineWidth = 1
    g.strokeStyle = colors.edge
    g.globalAlpha = s.edges.length > 10000 ? 0.2 : 0.45
    line(s.edges)
    g.globalAlpha = 1
    if (s.strong?.length) {
      g.strokeStyle = colors.focus
      g.lineWidth = 1.5
      line(s.strong)
    }
    // Nodes: one path per colour, off-screen ones skipped.
    const r = radius(k)
    const groups = new Map()
    const pad = r + 2
    for (const i of s.nodes) {
      const x = X(i), y = Y(i)
      if (!(x > -pad && y > -pad && x < w + pad && y < h + pad)) continue
      const c = s.slot[i]
      let list = groups.get(c)
      if (!list) groups.set(c, (list = []))
      list.push(i, x, y)
    }
    for (const [c, list] of groups) {
      g.fillStyle = colors.palette[c] ?? colors.neutral
      g.beginPath()
      for (let q = 0; q < list.length; q += 3) shape(s.kind[list[q]], list[q + 1], list[q + 2], r)
      g.fill()
    }
    // Rings: diagnostics, search matches, hover, selection.
    const ring = (i, rr, width) => {
      g.lineWidth = width
      g.beginPath()
      g.arc(X(i), Y(i), rr, 0, TAU)
      g.stroke()
    }
    for (const list of groups.values())
      for (let q = 0; q < list.length; q += 3) {
        const m = s.marks[list[q]]
        if (!m) continue
        g.strokeStyle = m === 1 ? colors.error : m === 2 ? colors.warning : colors.info
        ring(list[q], r + 2.5, 2)
      }
    g.strokeStyle = colors.match
    for (const i of s.matches) ring(i, r + 4.5, 2)
    if (s.hover >= 0) (g.strokeStyle = colors.fg), ring(s.hover, r + 3, 1.5)
    if (s.selected >= 0) (g.strokeStyle = colors.focus), ring(s.selected, r + 5, 2.5)
    // Text: edge labels, then node labels with a halo in the background colour.
    // A label that would cover one already drawn is left out (earlier ones are the important ones).
    const placed = []
    const free = (x, y, tw) => {
      const x0 = x - 1, x1 = x + tw + 1, y0 = y - 7, y1 = y + 7
      for (let q = 0; q < placed.length; q += 4) if (x0 < placed[q + 2] && x1 > placed[q] && y0 < placed[q + 3] && y1 > placed[q + 1]) return false
      placed.push(x0, y0, x1, y1)
      return true
    }
    g.font = "11px system-ui, -apple-system, sans-serif"
    g.textBaseline = "middle"
    g.lineJoin = "round"
    g.lineWidth = 3
    g.strokeStyle = colors.bg
    if (s.edgeLabels?.length) {
      g.textAlign = "center"
      g.fillStyle = colors.muted
      for (const { a, b, text } of s.edgeLabels) {
        const x = (X(a) + X(b)) / 2, y = (Y(a) + Y(b)) / 2
        const tw = g.measureText(text).width
        if (!free(x - tw / 2, y, tw)) continue
        g.strokeText(text, x, y)
        g.fillText(text, x, y)
      }
    }
    g.textAlign = "left"
    g.fillStyle = colors.fg
    for (const i of s.labels) {
      const x = X(i) + r + 4, y = Y(i)
      if (x > w + 4 || y < -8 || y > h + 8) continue
      const text = s.text(i)
      if (!free(x, y, g.measureText(text).width)) continue
      g.strokeText(text, x, y)
      g.fillText(text, x, y)
    }
    return performance.now() - t0
  }

  return { size, draw, radius, get width() { return w }, get height() { return h } }
}
