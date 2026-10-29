// The graph view's test page (graph-ui.html): mounts createGraphView with a
// fake `request` that answers from graph-fixture.mjs and records every call,
// a `reveal` and a `store` that record too. The test drives it through
// window.__graph.

import { createGraphView } from "../app/graph/view.js"
import { smallGraph, bigGraph } from "./graph-fixture.mjs"

const host = document.getElementById("host")
document.documentElement.style.height = "100%"
document.body.style.margin = "0"
document.body.style.height = "100%"
window.__violations = []
document.addEventListener("securitypolicyviolation", (e) => window.__violations.push(`${e.violatedDirective} ${e.blockedURI}`))

let view = null
const T = (window.__graph = {
  calls: [],
  reveals: [],
  stored: {},
  /**
   * Mount a fresh view. `size`: a big graph of that many nodes instead of
   * the small one; `total`: its node count before the cap; `width`: the host's width.
   */
  mount({ size = 0, total, width, diagnostics = [], stored } = {}) {
    view?.destroy()
    T.calls = []
    T.reveals = []
    T.stored = stored ? { graph: stored } : {}
    host.style.height = "100vh"
    host.style.width = width ? `${width}px` : ""
    const answer = (params) => {
      const res = size ? bigGraph(size, total ?? size) : smallGraph()
      // As the server: earlier positions are where the layout continues from.
      const old = params.layout?.positions
      if (old) res.nodes.forEach((d, i) => old[d.id] && ((res.positions[2 * i] = old[d.id][0]), (res.positions[2 * i + 1] = old[d.id][1])))
      return res
    }
    view = createGraphView({
      container: host,
      request: async (method, params) => {
        T.calls.push({ method, params: JSON.parse(JSON.stringify(params)) })
        await new Promise((r) => setTimeout(r, 5))
        return answer(params)
      },
      reveal: (uri, range) => T.reveals.push({ uri, range }),
      diagnostics: (uri) => diagnostics.filter((d) => d.uri === uri),
      store: { get: (k) => T.stored[k], set: (k, v) => (T.stored[k] = JSON.parse(JSON.stringify(v))) },
    })
    T.view = view
    return true
  },
  /** Pixels of the canvas that differ from its top-left pixel (the background). */
  inked() {
    const c = host.querySelector("canvas")
    const g = c.getContext("2d")
    const d = g.getImageData(0, 0, c.width, c.height).data
    let n = 0
    for (let p = 0; p < d.length; p += 4) if (Math.abs(d[p] - d[0]) + Math.abs(d[p + 1] - d[1]) + Math.abs(d[p + 2] - d[2]) > 30) n++
    return n
  },
  /** The canvas background at its top-left pixel, [r, g, b]. */
  corner() {
    const c = host.querySelector("canvas")
    return [...c.getContext("2d").getImageData(0, 0, 1, 1).data.slice(0, 3)]
  },
  /** Zoom and pan for `frames` animation frames; the draw times (ms) and the frame intervals. */
  async frames(frames = 60) {
    const c = host.querySelector("canvas")
    const b = c.getBoundingClientRect()
    const draws = [], gaps = []
    let last = performance.now()
    for (let k = 0; k < frames; k++) {
      c.dispatchEvent(new WheelEvent("wheel", { deltaY: k % 20 < 10 ? -40 : 40, clientX: b.left + b.width / 2 + Math.sin(k) * 50, clientY: b.top + b.height / 2, bubbles: true, cancelable: true }))
      // The view's frame was asked first, so it has drawn when this one runs.
      await new Promise((r) => requestAnimationFrame(r))
      c.getContext("2d").getImageData(0, 0, 1, 1)
      const now = performance.now()
      gaps.push(now - last)
      last = now
      draws.push(Number(host.querySelector(".graph").dataset.drawMs))
    }
    return { draws, gaps }
  },
})
document.body.dataset.ready = "true"
