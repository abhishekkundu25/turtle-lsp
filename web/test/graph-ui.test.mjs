// The shared graph view (web/app/graph/): its pure parts in Node (hit grid,
// rings, hierarchy, request params, default mode, labels, diagnostics), then
// the whole view in the installed Google Chrome, driven by Playwright, on a
// test page under a webview-strict policy with a fake `turtle/graph` server.
// Each test names the defect it catches.
import { test, before, after, describe } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium } from "playwright-core"
import { listen } from "./static-server.mjs"
import { createHitGrid } from "../app/graph/hitgrid.js"
import { RING_CAPS, buildHierarchy, pathTo, ringLayout } from "../app/graph/rings.js"
import { buildModel, chooseDefault, diagnosticMarks, overlaps, searchNodes, triplesOf, bestNode } from "../app/graph/model.js"
import { defaultState, hierarchyParams, normalizeState, setRule, toParams } from "../app/graph/filters.js"
import { pickLabels, truncate } from "../app/graph/labels.js"
import { NEUTRAL, colorSlots } from "../app/graph/colors.js"
import { A, B, EX, SUBCLASS, bigGraph, smallGraph } from "./graph-fixture.mjs"

const WEB = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const PORT = 4176

// ---- pure parts, in Node -----------------------------------------------------------------

describe("hit grid", () => {
  test("finds the nearest node within the radius and nothing outside it (catches: clicks selecting a far node, or none)", () => {
    const x = [0, 10, 100, NaN], y = [0, 0, 100, 5]
    const grid = createHitGrid(x, y, [0, 1, 2, 3])
    assert.equal(grid.nearest(2, 1, 5), 0)
    assert.equal(grid.nearest(8, 0, 5), 1)
    assert.equal(grid.nearest(50, 50, 5), -1)
    assert.equal(grid.nearest(0, 5, 1), -1, "a node with no position (NaN) is never hit")
    assert.equal(grid.nearest(5, 0, 6), 0, "a tie goes to the lower index")
    assert.equal(createHitGrid(x, y, [1, 2]).nearest(0, 0, 3), -1, "only the listed nodes are hit")
    assert.equal(createHitGrid([], [], []).nearest(0, 0, 10), -1)
  })

  test("agrees with a linear scan on 2,000 scattered nodes (catches: a cell border losing the nearest node)", () => {
    const g = buildModel(bigGraph(2000))
    const ids = Array.from({ length: g.n }, (_, i) => i)
    const grid = createHitGrid(g.x, g.y, ids)
    let seed = 3
    const rnd = () => ((seed = (seed * 16807) % 2147483647) / 2147483647)
    for (let t = 0; t < 500; t++) {
      const px = (rnd() - 0.5) * 2000, py = (rnd() - 0.5) * 2000, r = 5 + rnd() * 40
      let best = -1, bd = r * r
      for (const i of ids) {
        const d = (g.x[i] - px) ** 2 + (g.y[i] - py) ** 2
        if (d < bd || (d === bd && (best < 0 || i < best))) (bd = d), (best = i)
      }
      assert.equal(grid.nearest(px, py, r), best, `at ${px}, ${py} within ${r}`)
    }
  })
})

describe("focus rings and hierarchy", () => {
  test("rings cap each ring and count the rest as +N more (catches: hairball rings, a lost or wrong +N count)", () => {
    const m = buildModel(smallGraph())
    const pump = m.index.get(`${EX}Pump`)
    const r = ringLayout(m, pump, { hops: 2 })
    assert.deepEqual(r.rings[0], [pump])
    assert.equal(r.rings[1].length, RING_CAPS[1])
    // Pump has 53 neighbours: Device, Valve, PumpShape, 50 sensors.
    assert.equal(r.more[1], 53 - RING_CAPS[1])
    assert.ok(r.moreAt[1], "the +N more has a place on the ring")
    for (const j of r.rings[1]) assert.ok(Math.abs(Math.hypot(r.x[j], r.y[j]) - r.radius[1]) < 1e-6, "each node sits on its ring")
    assert.ok(r.nodes.every((i) => Number.isFinite(r.x[i])))
    assert.equal(r.nodes.length, new Set(r.nodes).size, "a node is placed once")
    assert.ok(Number.isNaN(r.x[r.nodes.length > 0 ? m.n - 1 : 0]) || r.nodes.includes(m.n - 1), "unplaced nodes have no position")
    // Ring 1 is grouped by predicate: outgoing first.
    const groups = r.rings[1].map((j) => m.edges[r.parentEdge.get(j)]).map((e) => (e.source === pump ? "0" : "1") + e.name)
    assert.deepEqual(groups, [...groups].sort())
    const wider = ringLayout(m, pump, { hops: 1, caps: [1, 100] })
    assert.equal(wider.rings[1].length, 53)
    assert.equal(wider.more[1], 0)
    assert.equal(ringLayout(m, -1).nodes.length, 0)
  })

  test("the class hierarchy has its roots, sorted children, sizes and survives a cycle (catches: missing classes, a hang on A ⊑ B ⊑ A)", () => {
    const m = buildModel(smallGraph())
    const h = buildHierarchy(m)
    const id = (i) => m.nodes[i].id.slice(EX.length)
    assert.equal(id(h.roots[0]), "Device")
    assert.deepEqual(h.children.get(h.roots[0]).map(id), ["Pump", "Valve"])
    assert.equal(h.size[h.roots[0]], 2)
    assert.deepEqual(pathTo(h, m.index.get(`${EX}Valve`)).map(id), ["Device", "Valve"])
    const cycle = buildModel({
      nodes: ["a", "b", "c"].map((s) => ({ id: s, kind: "other" })),
      edges: [{ source: 0, target: 1, predicate: SUBCLASS }, { source: 1, target: 0, predicate: SUBCLASS }, { source: 2, target: 2, predicate: SUBCLASS }],
    })
    const hc = buildHierarchy(cycle)
    assert.deepEqual(hc.roots, [0], "a cycle with no way in gets a root; a self loop is ignored")
    assert.deepEqual(pathTo(hc, 1), [0, 1])
  })
})

describe("filters and request params", () => {
  test("the defaults hide rdf:type and literals, cap at 2,000 and send no focus without a depth (catches: a first request that floods the page)", () => {
    const s = defaultState()
    assert.deepEqual(toParams(s.filters), { literals: false, types: false, maxNodes: 2000, layout: { iterations: 300 } })
    assert.deepEqual(toParams(s.filters, { focus: "x" }).focus, undefined)
    const f = { ...s.filters, depth: 2, files: [A], kinds: ["class"], namespaces: { include: [], exclude: [EX] } }
    const p = toParams(f, { focus: `${EX}Pump`, positions: new Map([[`${EX}Pump`, [1.234, -5.678]]]), iterations: 100 })
    assert.deepEqual(p, {
      literals: false, types: false, maxNodes: 2000, files: [A], kinds: ["class"],
      namespaces: { include: [], exclude: [EX] }, focus: `${EX}Pump`, depth: 2,
      layout: { iterations: 100, positions: { [`${EX}Pump`]: [1.23, -5.68] } },
    })
    const hp = hierarchyParams(f)
    assert.deepEqual(hp.predicates, { include: [SUBCLASS], exclude: [] })
    assert.equal(hp.focus, undefined)
    assert.equal(hp.maxNodes, 20000)
    assert.equal(hp.layout.iterations, 0, "the hierarchy needs no layout")
  })

  test("a stored state is checked: broken fields take their default (catches: a bad store value reaching the server)", () => {
    const s = normalizeState({ mode: "graph", colorBy: "nope", filters: { files: [A, 3, A], kinds: ["class", "bogus"], depth: 9, maxNodes: 7, literals: "yes", types: true, predicates: { include: "x", exclude: [SUBCLASS] } } })
    assert.equal(s.mode, "graph")
    assert.equal(s.colorBy, "file")
    assert.deepEqual(s.filters.files, [A])
    assert.deepEqual(s.filters.kinds, ["class"])
    assert.equal(s.filters.depth, 0)
    assert.equal(s.filters.maxNodes, 2000)
    assert.equal(s.filters.literals, false)
    assert.equal(s.filters.types, true)
    assert.deepEqual(s.filters.predicates, { include: [], exclude: [SUBCLASS] })
    assert.deepEqual(normalizeState("garbage"), defaultState())
    const pair = { include: ["a"], exclude: [] }
    setRule(pair, "a", "exclude")
    assert.deepEqual(pair, { include: [], exclude: ["a"] })
  })

  test("over 2,000 nodes the view opens in focus mode on the best-connected class with 2 hops (catches: a 17,000-node hairball first)", () => {
    assert.deepEqual(chooseDefault(smallGraph()), { mode: "graph" })
    const big = bigGraph(300, 17000)
    const choice = chooseDefault(big)
    assert.equal(choice.mode, "focus")
    assert.equal(choice.depth, 2)
    const m = buildModel(big)
    const best = m.index.get(choice.focus)
    assert.equal(m.kind[best], 0, "a class")
    for (let i = 0; i < m.n; i++) if (m.kind[i] === 0) assert.ok(m.deg[i] <= m.deg[best])
    // No class at all: the best-connected node.
    const plain = buildModel({ nodes: [{ id: "b", kind: "other" }, { id: "a", kind: "other" }, { id: "c", kind: "individual" }], edges: [{ source: 2, target: 0 }, { source: 2, target: 1 }] })
    assert.equal(plain.nodes[bestNode(plain)].id, "c")
  })
})

describe("labels, model, colours and diagnostics", () => {
  test("labels: the selection and neighbours always, the rest only zoomed in and at most a few hundred (catches: thousands of labels per frame)", () => {
    const inView = Array.from({ length: 1000 }, (_, i) => i)
    const deg = Array.from({ length: 1000 }, (_, i) => i % 7)
    assert.deepEqual(pickLabels({ always: [5, 6, 5], inView, zoom: 0.5, deg }), [5, 6])
    const zoomed = pickLabels({ always: [5], inView, zoom: 2, deg })
    assert.equal(zoomed.length, 300)
    assert.equal(zoomed[0], 5)
    assert.ok(zoomed.slice(1).every((i, k, a) => k === 0 || deg[a[k - 1]] >= deg[i]), "best connected first")
    assert.ok(zoomed.slice(1, 100).every((i) => deg[i] === 6))
    assert.equal(pickLabels({ inView: [1, 2, 3], zoom: 0.1 }).length, 3, "a few nodes in view are all labelled")
    assert.equal(truncate("x".repeat(40)).length, 32)
  })

  test("the model drops edges to missing nodes, lists triples both ways and ranks search hits (catches: a crash on a bad answer, a wrong info panel)", () => {
    const m = buildModel({ nodes: [{ id: "a", name: "ex:a" }, { id: "b", label: "Bee" }], edges: [{ source: 0, target: 1, predicate: "p", name: "ex:p" }, { source: 0, target: 9 }, { source: "x", target: 0 }], positions: [1, 2] })
    assert.equal(m.m, 1)
    assert.deepEqual([m.x[1], m.y[1]], [0, 0], "a missing position is the origin")
    assert.deepEqual(triplesOf(m, 0), { out: [{ predicate: "p", name: "ex:p", node: 1 }], in: [] })
    assert.deepEqual(triplesOf(m, 1).in, [{ predicate: "p", name: "ex:p", node: 0 }])
    assert.equal(buildModel(null).n, 0)
    const s = buildModel(smallGraph())
    assert.equal(s.nodes[searchNodes(s, "pump")[0]].id, `${EX}Pump`, "the exact match first")
    assert.deepEqual(searchNodes(s, "  "), [])
  })

  test("colour slots stay with their file across answers; literals are neutral (catches: colours changing meaning after a filter)", () => {
    const m = buildModel({ nodes: [{ id: "a", uri: B }, { id: "b", uri: A }, { id: "c", literal: true, uri: A }, { id: `${EX}d`, kind: "shape" }] })
    assert.deepEqual([...colorSlots(m, "file", { files: [{ uri: A }, { uri: B }], namespaces: [] })], [1, 0, NEUTRAL, NEUTRAL])
    assert.deepEqual([...colorSlots(m, "kind", m.facets)], [5, 5, NEUTRAL, 3])
    assert.equal(colorSlots(m, "namespace", { files: [], namespaces: [{ iri: "http://" }, { iri: EX }] })[3], 1, "the longest namespace wins")
  })

  test("a node is marked with the worst diagnostic overlapping its range, each document asked once (catches: marks on the wrong node, a slow redraw)", () => {
    const r = (l0, c0, l1, c1) => ({ start: { line: l0, character: c0 }, end: { line: l1, character: c1 } })
    assert.ok(overlaps(r(1, 0, 1, 5), r(1, 5, 2, 0)), "touching counts")
    assert.ok(!overlaps(r(1, 0, 1, 4), r(1, 5, 2, 0)))
    const asked = []
    const nodes = [{ id: "a", uri: A, range: r(3, 0, 3, 6) }, { id: "b", uri: A, range: r(9, 0, 9, 3) }, { id: "c", uri: B, range: r(0, 0, 0, 1) }, { id: "d" }]
    const marks = diagnosticMarks(nodes, (uri) => {
      asked.push(uri)
      if (uri === B) throw new Error("gone")
      return [{ range: r(3, 2, 3, 3), severity: 2 }, { range: r(3, 4, 4, 0), severity: 1 }, { range: r(20, 0, 20, 1), severity: 1 }]
    })
    assert.deepEqual([...marks], [1, 0, 0, 0])
    assert.deepEqual(asked, [A, B])
    assert.deepEqual([...diagnosticMarks(nodes)], [0, 0, 0, 0])
  })
})

// ---- the whole view, in Chrome -------------------------------------------------------------

describe("the view in Chrome", () => {
  let server, browser
  before(async () => {
    server = await listen({ root: WEB, port: PORT })
    browser = await chromium.launch({ channel: "chrome", headless: true })
  })
  after(async () => {
    await browser?.close()
    await server?.close()
  })

  async function open(mount = {}, viewport = { width: 1280, height: 800 }) {
    const ctx = await browser.newContext({ viewport })
    const page = await ctx.newPage()
    page.setDefaultTimeout(10_000)
    const errors = []
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
    page.on("pageerror", (e) => errors.push(String(e)))
    await page.goto(`${server.origin}/test/graph-ui.html`)
    await page.locator("body[data-ready='true']").waitFor()
    await page.evaluate((o) => window.__graph.mount(o), mount)
    await page.locator(".graph[data-busy='false'] .graph-status", { hasText: /Showing/ }).waitFor()
    const canvas = page.locator("canvas")
    return {
      page,
      errors,
      canvas,
      calls: () => page.evaluate(() => window.__graph.calls),
      center: async () => {
        const b = await canvas.boundingBox()
        return [b.x + b.width / 2, b.y + b.height / 2]
      },
      close: async () => {
        assert.deepEqual(await page.evaluate(() => window.__violations), [], "no policy violation")
        assert.deepEqual(errors, [], "no console error")
        await ctx.close()
      },
    }
  }

  test("draws the graph on the canvas, says showing N of M, under a strict policy (catches: a blank canvas, a CSP-breaking inline style or handler)", async () => {
    const v = await open()
    try {
      assert.ok((await v.page.evaluate(() => window.__graph.inked())) > 500, "the canvas has drawn nodes and edges")
      assert.match(await v.page.locator(".graph-status").textContent(), /Showing 55 of 55 nodes/)
      const calls = await v.calls()
      // A first answer without layout decides the mode, then the layout request.
      assert.deepEqual(calls.map((c) => [c.method, c.params.layout.iterations]), [["turtle/graph", 0], ["turtle/graph", 300]])
      assert.equal(calls[1].params.types, false)
      assert.equal(calls[1].params.literals, false)
      // The canvas is sized for the device pixel ratio.
      const [w, cw] = await v.page.evaluate(() => [document.querySelector("canvas").getBoundingClientRect().width, document.querySelector("canvas").width])
      assert.equal(cw, Math.round(w * (await v.page.evaluate(() => devicePixelRatio))))
      await v.page.getByRole("tab", { name: "Graph" }).waitFor()
      assert.equal(await v.page.getByRole("tab", { name: "Graph" }).getAttribute("aria-selected"), "true")
      assert.match(await v.page.locator(".graph-legend").textContent(), /a\.ttl.*b\.ttl/, "the colour key names the files")
    } finally {
      await v.close()
    }
  })

  test("a click selects the node under it; the panel lists its triples and Go to definition reveals it (catches: wrong hit transform, panel off the response's edges, a broken reveal)", async () => {
    const v = await open()
    try {
      const [x, y] = await v.center()
      await v.page.mouse.click(x, y)
      const info = v.page.getByRole("region", { name: "Selected node" })
      await info.getByRole("heading", { name: "Pump" }).waitFor()
      const text = await info.textContent()
      assert.match(text, /Out \(2\)/)
      assert.match(text, /rdfs:subClassOf\s*ex:Device/)
      assert.match(text, /ex:hasPart\s*Valve/)
      assert.match(text, /In \(51\)/)
      assert.match(text, /← sh:targetClass\s*ex:PumpShape/)
      await info.getByRole("button", { name: "Go to definition" }).click()
      const reveals = await v.page.evaluate(() => window.__graph.reveals)
      assert.deepEqual(reveals, [{ uri: A, range: { start: { line: 3, character: 0 }, end: { line: 3, character: 6 } } }])
      // Double-click and Enter on the canvas reveal too.
      await v.page.mouse.dblclick(x, y)
      await v.canvas.focus()
      await v.page.keyboard.press("Enter")
      assert.equal((await v.page.evaluate(() => window.__graph.reveals)).length, 3)
      // A triple's object selects that node.
      await info.getByRole("button", { name: /ex:hasPart\s*Valve/ }).click()
      await info.getByRole("heading", { name: "Valve" }).waitFor()
      // A click on empty canvas clears the selection.
      await v.page.mouse.click(x + 40, y + 40)
      await info.waitFor({ state: "hidden" })
    } finally {
      await v.close()
    }
  })

  test("a filter change asks again with the previous positions, so the layout stays put (catches: lost positions, filters not in the request)", async () => {
    const v = await open()
    try {
      const before = await v.calls()
      await v.page.locator("summary", { hasText: "Predicates" }).click()
      await v.page.getByRole("checkbox", { name: "Literals" }).check()
      await v.page.getByRole("combobox", { name: "ex:feeds: any, only or hide" }).selectOption("exclude")
      await v.page.waitForFunction((n) => window.__graph.calls.length > n && document.querySelector(".graph").dataset.busy === "false", before.length)
      const calls = await v.calls()
      assert.equal(calls.length, before.length + 1, "two changes within the debounce make one request")
      const p = calls.at(-1).params
      assert.equal(p.literals, true)
      assert.deepEqual(p.predicates, { include: [], exclude: [`${EX}feeds`] })
      assert.equal(p.layout.iterations, 100, "the layout continues instead of starting over")
      const first = smallGraph()
      assert.equal(Object.keys(p.layout.positions).length, first.nodes.length)
      first.nodes.forEach((d, i) => assert.deepEqual(p.layout.positions[d.id], [first.positions[2 * i], first.positions[2 * i + 1]], d.id))
      const stored = await v.page.evaluate(() => window.__graph.stored.graph)
      assert.equal(stored.filters.literals, true, "the filters are kept in the store")
      // Kinds: unchecking one sends the rest.
      await v.page.getByRole("checkbox", { name: "shape" }).uncheck()
      await v.page.waitForFunction((n) => window.__graph.calls.length > n, calls.length)
      assert.deepEqual((await v.calls()).at(-1).params.kinds, ["class", "property", "individual", "concept", "other"])
    } finally {
      await v.close()
    }
  })

  test("focus mode puts the selection on rings with a +N more, asks with focus and depth and no layout (catches: no cap on rings, a force layout in mode C)", async () => {
    const v = await open()
    try {
      const [x, y] = await v.center()
      await v.page.mouse.click(x, y)
      await v.page.getByRole("heading", { name: "Pump" }).waitFor()
      const n = (await v.calls()).length
      await v.page.getByRole("tab", { name: "Focus" }).click()
      const more = v.page.getByRole("button", { name: "Show 13 more on ring 1" })
      await more.waitFor()
      assert.equal(await more.textContent(), "+13 more")
      const p = (await v.calls())[n].params
      assert.equal(p.focus, `${EX}Pump`)
      assert.equal(p.depth, 2)
      assert.equal(p.layout.iterations, 0)
      assert.match(await v.page.locator(".graph-status").textContent(), /Showing 41 of 55 nodes/)
      assert.ok((await v.page.evaluate(() => window.__graph.inked())) > 300)
      await more.click()
      await more.waitFor({ state: "detached" })
      assert.match(await v.page.locator(".graph-status").textContent(), /Showing 54 of 55 nodes/, "+N more shows them")
      // The focus is in the middle: a click there selects it.
      await v.page.mouse.click(x, y)
      await v.page.getByRole("heading", { name: "Pump" }).waitFor()
      // The hierarchy tab shows the subClassOf tree.
      await v.page.getByRole("tab", { name: "Hierarchy" }).click()
      const tree = v.page.getByRole("tree", { name: "Class hierarchy" })
      await tree.getByRole("treeitem", { name: /ex:Device/ }).waitFor()
      assert.deepEqual(await tree.getByRole("treeitem", { name: /ex:Device/ }).getByRole("treeitem").locator(".graph-tree-name").allTextContents(), ["Pump", "Valve"])
      assert.deepEqual((await v.calls()).at(-1).params.predicates, { include: [SUBCLASS], exclude: [] })
    } finally {
      await v.close()
    }
  })

  test("search highlights matches, and Enter selects and centres the first (catches: search that finds but does not focus)", async () => {
    const v = await open()
    try {
      const search = v.page.getByRole("searchbox", { name: "Search nodes" })
      await search.fill("valve")
      await v.page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /Valve/ }).waitFor()
      await search.press("Enter")
      await v.page.getByRole("heading", { name: "Valve" }).waitFor()
      // Centred: Escape clears, and a click at the centre finds Valve there.
      await v.page.keyboard.press("Escape")
      await v.page.getByRole("region", { name: "Selected node" }).waitFor({ state: "hidden" })
      const [x, y] = await v.center()
      await v.page.mouse.click(x, y)
      await v.page.getByRole("heading", { name: "Valve" }).waitFor()
    } finally {
      await v.close()
    }
  })

  test("a big workspace opens in focus mode on its best-connected class (catches: drawing 17,000 nodes first)", async () => {
    const v = await open({ size: 300, total: 17000 })
    try {
      assert.equal(await v.page.getByRole("tab", { name: "Focus" }).getAttribute("aria-selected"), "true")
      const calls = await v.calls()
      assert.equal(calls.length, 2)
      const choice = chooseDefault(bigGraph(300, 17000))
      assert.equal(calls[1].params.focus, choice.focus)
      assert.equal(calls[1].params.depth, 2)
      assert.match(await v.page.locator(".graph-status").textContent(), /of 17,000 nodes/)
    } finally {
      await v.close()
    }
  })

  test("a node with a diagnostic is marked and says so (catches: diagnostics(uri) never consulted)", async () => {
    const diag = [{ uri: A, range: { start: { line: 3, character: 2 }, end: { line: 3, character: 4 } }, severity: 1 }]
    const v = await open({ diagnostics: diag })
    try {
      const [x, y] = await v.center()
      await v.page.mouse.click(x, y)
      await v.page.getByText("Has error diagnostics").waitFor()
    } finally {
      await v.close()
    }
  })

  test("the theme follows color-scheme and setTheme; a narrow host stacks the filters above the drawing (catches: a dark page with a white canvas, a side bar that overflows)", async () => {
    const v = await open({ width: 380 }, { width: 380, height: 800 })
    try {
      const light = await v.page.evaluate(() => window.__graph.corner())
      assert.ok(light.every((c) => c > 200), `light background ${light}`)
      await v.page.evaluate(() => {
        document.documentElement.style.colorScheme = "dark"
        window.__graph.view.setTheme()
      })
      const dark = await v.page.evaluate(() => window.__graph.corner())
      assert.ok(dark.every((c) => c < 80), `dark background ${dark}`)
      await v.page.evaluate(() => window.__graph.view.setTheme({ "--graph-bg": "rgb(10, 200, 30)" }))
      assert.deepEqual(await v.page.evaluate(() => window.__graph.corner()), [10, 200, 30])
      const [f, s] = await v.page.evaluate(() => [".graph-filters", ".graph-stage"].map((q) => document.querySelector(q).getBoundingClientRect().toJSON()))
      assert.ok(f.bottom <= s.top + 1, "the filters are above the stage")
      assert.ok(s.height > 200, "the drawing keeps room")
      assert.equal(await v.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true, "no horizontal scroll")
    } finally {
      await v.close()
    }
  })

  test("draws a 2,000-node answer within a frame (catches: a renderer too slow for the cap)", async () => {
    const v = await open({ size: 2000 })
    try {
      assert.match(await v.page.locator(".graph-status").textContent(), /Showing 2,000 of 2,000 nodes/)
      const { draws, gaps } = await v.page.evaluate(() => window.__graph.frames(60))
      const median = (a) => [...a].sort((p, q) => p - q)[Math.floor(a.length / 2)]
      const drawMs = median(draws), frameMs = median(gaps)
      console.log(`2,000 nodes: draw ${drawMs.toFixed(2)} ms median (max ${Math.max(...draws).toFixed(2)}), frame ${frameMs.toFixed(1)} ms median`)
      assert.ok(drawMs < 16, `draw ${drawMs} ms`)
    } finally {
      await v.close()
    }
  })
})
