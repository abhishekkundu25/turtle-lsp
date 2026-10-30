// The graph view's test page (graph-ui.html): mounts createGraphView with a
// fake `request` that answers from graph-fixture.mjs (and `turtle/schema`
// from the answer the test hands over) and records every call, a `reveal`
// and a `store` that record too. As the server would, it resolves a Follow
// cursor `at` by its line, answers a Schema `search` with `matches` over the
// classes listed and the `unlisted` ones, and lists an unlisted class when
// it is the `focus`. The test drives it through window.__graph.

import { createGraphView } from "../app/graph/view.js"
import { smallGraph, bigGraph, bigSchema } from "./graph-fixture.mjs"

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
  // With `hold` on, requests wait in `held` until the test calls release(k).
  hold: false,
  held: [],
  release(k = 0) {
    const [r] = T.held.splice(k, 1)
    r?.()
    return Boolean(r)
  },
  /**
   * Mount a fresh view. `size`: a big graph of that many nodes instead of
   * the small one; `total`: its node count before the cap; `width`: the host's width.
   */
  mount({ size = 0, total, width, diagnostics = [], stored, schema, schemaSize = 0, unlisted = [], atClass = {}, atNode = {} } = {}) {
    view?.destroy()
    T.calls = []
    T.reveals = []
    T.hold = false
    T.held = []
    T.stored = stored ? { graph: stored } : {}
    // The Schema answer: the test may swap it (setSchema) to model an edit.
    T.schema = schema
    T.setSchema = (next) => (T.schema = next)
    const big = schemaSize ? JSON.stringify(bigSchema(schemaSize)) : null
    host.style.height = "100vh"
    host.style.width = width ? `${width}px` : ""
    // `turtle/schema`: the handed answer (or `schemaSize` generated classes);
    // `at` on a line of `atClass` focuses that class; on any other line
    // there is no term, and the server answers null. As the server, only
    // one class's rows keep their places (`examples`): the class `examples`
    // names when listed, else the focus (`focus` when listed, else the
    // answer's own).
    const schemaAnswer = (params) => {
      const res = JSON.parse(big ?? JSON.stringify(T.schema ?? { classes: [], total: { classes: 0 }, imported: 0 }))
      // `unlisted`: classes past the cap ({ id, name, label?, instances? }).
      res.total.classes = res.classes.length + unlisted.length
      const far = unlisted.findIndex((c) => c.id === params.focus)
      if (far >= 0) {
        // As the server: the class asked for is listed, the last listed one makes room.
        const c = unlisted[far]
        res.classes.pop()
        for (const k of res.classes) for (const r of k.rows) if (r.target >= res.classes.length) delete r.target
        for (const k of res.classes) k.parents = k.parents.filter((p) => p < res.classes.length)
        res.classes.push({ id: c.id, name: c.name, label: c.label, kind: "class", parents: [], instances: c.instances ?? 0, problems: 0, rows: [], moreRows: 0 })
        res.focus = res.classes.length - 1
        delete res.focusRow
      }
      if (typeof params.search === "string") res.matches = search(params.search, res, unlisted)
      if (params.at) {
        const hit = atClass[params.at.position.line]
        if (!hit) return null
        delete res.focusRow
        res.focus = hit[0]
        if (hit[1] != null) res.focusRow = hit[1]
      }
      const ix = (id) => (typeof id === "string" ? res.classes.findIndex((c) => c.id === id) : -1)
      const asked = ix(params.examples)
      const focus = ix(params.focus) >= 0 ? ix(params.focus) : res.focus
      const keep = asked >= 0 ? asked : focus
      res.classes.forEach((c, i) => i !== keep && c.rows.forEach((r) => delete r.examples))
      return res
    }
    // Best first: the exact local name, then a name or label it starts, then
    // one it is in (or the IRI); ties by instances, then IRI. At most 20.
    const search = (q, res, extra) => {
      const s = q.toLowerCase()
      const all = [...res.classes.map((c, index) => ({ ...c, index })), ...extra.filter((c) => !res.classes.some((k) => k.id === c.id))]
      const local = (n) => n.slice(n.indexOf(":") + 1).toLowerCase()
      const rank = (c) => {
        const names = [c.name ?? "", c.label ?? ""].map((v) => v.toLowerCase())
        if (local(c.name ?? "") === s) return 0
        if (names.some((v) => v.startsWith(s) || local(v).startsWith(s))) return 1
        if ([...names, c.id.toLowerCase()].some((v) => v.includes(s))) return 2
        return -1
      }
      return all
        .map((c) => [rank(c), c])
        .filter(([r]) => r >= 0)
        .sort(([a, x], [b, y]) => a - b || (y.instances ?? 0) - (x.instances ?? 0) || (x.id < y.id ? -1 : 1))
        .slice(0, 20)
        .map(([, c]) => ({ id: c.id, name: c.name, ...(c.label ? { label: c.label } : {}), ...(c.index != null ? { index: c.index } : {}) }))
    }
    const answer = (method, params) => {
      if (method === "turtle/schema") return schemaAnswer(params)
      const res = size ? bigGraph(size, total ?? size) : smallGraph()
      // `at` on a line of `atNode` resolves to that node, which the answer
      // lists first (breadth-first from the focus); any other line: null.
      if (params.at) {
        const id = atNode[params.at.position.line]
        const i = id ? res.nodes.findIndex((d) => d.id === id) : -1
        if (i < 0) return null
        res.nodes.unshift(...res.nodes.splice(i, 1))
      }
      // As the server: earlier positions are where the layout continues from.
      const old = params.layout?.positions
      if (old) res.nodes.forEach((d, i) => old[d.id] && ((res.positions[2 * i] = old[d.id][0]), (res.positions[2 * i + 1] = old[d.id][1])))
      return res
    }
    view = createGraphView({
      container: host,
      request: async (method, params) => {
        T.calls.push({ method, params: JSON.parse(JSON.stringify(params)) })
        if (T.hold) await new Promise((r) => T.held.push(r))
        else await new Promise((r) => setTimeout(r, 5))
        return answer(method, params)
      },
      reveal: (uri, range) => T.reveals.push({ uri, range }),
      diagnostics: (uri) => diagnostics.filter((d) => d.uri === uri),
      store: { get: (k) => T.stored[k], set: (k, v) => (T.stored[k] = JSON.parse(JSON.stringify(v))) },
    })
    T.view = view
    return true
  },
  /** Redraw the Schema answer `times` times; the draw times (ms) the view reports. */
  async schemaDraws(times = 5) {
    const out = []
    const box = host.querySelector(".graph-schemabox")
    for (let k = 0; k < times; k++) {
      const n = T.calls.length
      await view.refresh()
      if (T.calls.length === n) throw new Error("refresh asked nothing")
      out.push(Number(box.dataset.drawMs))
      await new Promise((r) => requestAnimationFrame(r))
    }
    return out
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
