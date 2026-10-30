// The editor's graph tab (graph/view.js shown by editor.js in an editor
// group) in the installed Google Chrome, driven by Playwright: the button the
// server's capability brings, the tab it opens (one, in the editor area, with
// one side bar), the starter workspace drawn, Go to definition beside the
// graph, the split, closing, the reload, a refresh after an edit (and none
// while the tab is hidden), the theme, and how long turtle/graph keeps the
// engine's worker from answering the editor. Run `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { chromium } from "playwright-core"
import { listen } from "./static-server.mjs"

let server, origin, browser
before(async () => {
  server = await listen()
  origin = server.origin
  browser = await chromium.launch({ channel: "chrome", headless: true })
})
after(async () => {
  await browser?.close()
  await server?.close()
})

// Times every turtle/graph request and every diagnostics publish at the
// worker's door (window.__wt), so a test can tell how long the engine was busy.
// With sessionStorage "no-graph" set, the server's answer to initialize loses
// its turtle/graph capability (a server without it).
function instrument() {
  const wt = (window.__wt = { graph: [], diags: [], sent: new Map(), changes: [] })
  const noGraph = sessionStorage.getItem("no-graph") === "1"
  const Base = window.Worker
  window.Worker = class extends Base {
    constructor(...a) {
      super(...a)
      this.addEventListener("message", ({ data }) => {
        const t = performance.now()
        if (noGraph && data?.type === "response" && data.result?.capabilities?.experimental) delete data.result.capabilities.experimental["turtle/graph"]
        if (data?.type === "response" && wt.sent.has(data.id)) {
          const s = wt.sent.get(data.id)
          wt.sent.delete(data.id)
          wt.graph.push({ ms: t - s.t, iterations: s.iterations, focus: s.focus, nodes: data.result?.nodes?.length ?? null, total: data.result?.total?.nodes ?? null, error: data.error ?? null })
        } else if (data?.type === "diagnostics") wt.diags.push({ uri: data.uri, t })
      })
    }
    postMessage(m, ...rest) {
      if (m?.kind === "request" && m.method === "turtle/graph") wt.sent.set(m.id, { t: performance.now(), iterations: m.params?.layout?.iterations, focus: m.params?.focus })
      if (m?.kind === "notify" && m.method === "textDocument/didChange") wt.changes.push({ uri: m.params.textDocument.uri, t: performance.now() })
      super.postMessage(m, ...rest)
    }
  }
}

const ready = (page) => page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })

async function open({ files, width = 1440 } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height: 900 } })
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  await page.addInitScript(instrument)
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/index.html")
  await ready(page)
  if (files) {
    await page.locator("#files-input").setInputFiles(Object.entries(files).map(([name, body]) => ({ name, mimeType: "text/plain", buffer: Buffer.from(body) })))
    await page.locator("#tree").getByText(Object.keys(files)[0], { exact: true }).waitFor()
    await idle(page)
  }
  return { page, errors, close: () => ctx.close() }
}

const idle = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 60_000 })
const host = (page) => page.locator(".graph-host")
const status = (page) => page.locator(".graph-host .graph-status")
const graphCalls = (page) => page.evaluate(() => window.__wt.graph.length)
const groups = (page) => page.locator("#groups > .group")
// An editor tab (not the view's own Graph / Focus / Hierarchy tabs), in group `g` or any.
const tab = (page, name, g) => (g === undefined ? page.locator("#groups > .group > .tabs") : groups(page).nth(g).locator("> .tabs")).getByRole("tab", { name, exact: true })
// A group's tabs by name, the selected one marked with *.
const tabNames = (page, g) => groups(page).nth(g).locator("> .tabs [role=tab]").evaluateAll((ts) => ts.map((t) => `${t.getAttribute("aria-label")}${t.getAttribute("aria-selected") === "true" ? "*" : ""}`))
// Wait until group `g`'s tabs are `want` (the tabs bar is drawn on the next frame).
async function tabsAre(page, g, want) {
  let got
  for (const end = Date.now() + 5000; Date.now() < end; await page.waitForTimeout(25)) {
    got = await tabNames(page, g)
    if (JSON.stringify(got) === JSON.stringify(want)) return
  }
  assert.deepEqual(got, want, `group ${g}'s tabs`)
}
const openGraph = async (page) => {
  await page.locator("#act-graph").click()
  return shown(page)
}
// Wait until the view has answered and says "Showing N of M nodes"; returns [N, M].
async function shown(page) {
  await page.locator(".graph-host .graph[data-busy='false'] .graph-status", { hasText: /Showing/ }).waitFor({ timeout: 60_000 })
  const m = (await status(page).textContent()).match(/Showing ([\d,]+) of ([\d,]+) nodes/)
  return [Number(m[1].replaceAll(",", "")), Number(m[2].replaceAll(",", ""))]
}
// Pixels of the graph's canvas that differ from its top-left corner (its background), and that corner.
const ink = (page) =>
  page.evaluate(() => {
    const c = document.querySelector(".graph-host canvas")
    const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data
    let n = 0
    for (let i = 4; i < d.length; i += 4) if (d[i] !== d[0] || d[i + 1] !== d[1] || d[i + 2] !== d[2]) n++
    return { inked: n, bg: [d[0], d[1], d[2]] }
  })
// Whether box `inner` lies inside box `outer`.
const inside = (inner, outer) => inner.x >= outer.x - 1 && inner.y >= outer.y - 1 && inner.x + inner.width <= outer.x + outer.width + 1 && inner.y + inner.height <= outer.y + outer.height + 1

async function append(page, uri, text) {
  await page.evaluate(
    ([uri, text]) => {
      const m = window.monaco.editor.getModel(window.monaco.Uri.parse(uri))
      const end = m.getFullModelRange().getEndPosition()
      m.pushEditOperations([], [{ range: new window.monaco.Range(end.lineNumber, end.column, end.lineNumber, end.column), text }], () => null)
    },
    [uri, text]
  )
}

// Select ex:Person in the view (by search, then a click at the canvas centre); returns its info panel.
async function selectPerson(page) {
  const search = page.getByRole("searchbox", { name: "Search nodes" })
  await search.fill("Person")
  await page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /ex:Person|Person/ }).first().waitFor()
  await search.press("Enter")
  const info = page.getByRole("region", { name: "Selected node" })
  await info.waitFor()
  // Centred by the search: a click at the canvas centre selects it too.
  await page.keyboard.press("Escape")
  await info.waitFor({ state: "hidden" })
  const b = await page.locator(".graph-host canvas").boundingBox()
  await page.mouse.click(b.x + b.width / 2, b.y + b.height / 2)
  await info.waitFor()
  assert.match(await info.textContent(), /Person/)
  return info
}

const CORE = "file:///work/ontology/core.ttl"
const START = "file:///work/start-here.ttl"

test("the Graph button opens a Graph tab in the editor area, drawn, with one side bar and one filter panel; the status bar has no file; a second click focuses the same tab (catches: the graph opening in the side bar, the old side bar section or note left behind, a blank canvas, a second graph tab or view, file status shown over the graph)", async () => {
  const { page, errors, close } = await open()
  try {
    const button = page.getByRole("button", { name: "Graph", exact: true })
    await button.waitFor()
    assert.equal(await host(page).count(), 0, "no view before the tab first shows")
    assert.equal(await graphCalls(page), 0, "nothing is asked before the tab is opened")
    await button.focus()
    await page.keyboard.press("Enter")
    const [n, total] = await shown(page)
    assert.ok(n > 0 && n <= total, `Showing ${n} of ${total}`)
    await tabsAre(page, 0, ["start-here.ttl", "Graph*"])
    // In the editor area, in place of the editor; the side bar is as it was.
    const center = await page.locator("main.center").boundingBox()
    const box = await host(page).boundingBox()
    assert.ok(inside(box, center) && box.width > center.width - 4, "fills the editor group's width")
    assert.equal(await page.locator("#groups > .group > .editor-host").isHidden(), true, "the editor is hidden under the graph")
    assert.equal(await page.locator("#view-explorer").isVisible(), true, "the side bar keeps the explorer")
    assert.equal(await page.locator(".side .graph-host, #view-graph, #graph-expand").count(), 0, "no graph section in the side bar")
    assert.equal(await page.getByText(/covers the editor area|Back to the side bar/).count(), 0)
    assert.equal(await page.locator(".graph-filters").count(), 1)
    assert.equal(await page.locator(".graph-filters").isVisible(), true, "the filters beside the drawing (the wide layout)")
    assert.equal(await page.getByRole("button", { name: "Filters" }).isHidden(), true)
    assert.equal(await page.locator("#status-lang").textContent(), "")
    assert.equal(await page.locator("#status-pos").textContent(), "")
    const dark = await ink(page)
    assert.ok(dark.inked > 300, `the canvas has nodes and edges drawn (${dark.inked} pixels)`)

    // A file tab shows the editor again, and its status.
    const calls = await graphCalls(page)
    await tab(page, "start-here.ttl").click()
    assert.equal(await host(page).isHidden(), true)
    assert.equal(await page.locator("#groups > .group > .editor-host").isVisible(), true)
    await page.waitForFunction(() => document.querySelector("#status-lang").textContent === "Turtle")
    // The button again: the same tab, the same view, nothing asked (nothing changed).
    await button.click()
    await shown(page)
    await tabsAre(page, 0, ["start-here.ttl", "Graph*"])
    await button.click()
    await tabsAre(page, 0, ["start-here.ttl", "Graph*"])
    assert.equal(await host(page).count(), 1)
    assert.equal(await graphCalls(page), calls)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("the graph tab follows the theme (catches: a theme the canvas never reads)", async () => {
  const { page, errors, close } = await open()
  try {
    await openGraph(page)
    const dark = await ink(page)
    await page.locator("#act-settings").click()
    await page.getByRole("region", { name: "Settings" }).getByLabel("Appearance: Color Theme", { exact: true }).selectOption("light")
    await page.waitForFunction(() => document.documentElement.dataset.theme === "light")
    await page.waitForFunction((bg) => {
      const d = document.querySelector(".graph-host canvas").getContext("2d").getImageData(0, 0, 1, 1).data
      return d[0] !== bg[0] || d[1] !== bg[1] || d[2] !== bg[2]
    }, dark.bg)
    const light = await ink(page)
    assert.ok(light.bg[0] > dark.bg[0] + 100, `light background ${light.bg} after dark ${dark.bg}`)
    await page.getByRole("button", { name: "Close Settings" }).click()
    assert.equal(await host(page).isVisible(), true, "the graph tab shows again under the settings")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Go to definition from the graph with one group opens the file in a new second group; the graph stays in its group (catches: the file replacing the graph, the graph tab copied into the new group, reveal not mapped from the node's URI to the workspace file, the range off by one)", async () => {
  const { page, errors, close } = await open()
  try {
    await openGraph(page)
    assert.equal(await groups(page).count(), 1)
    const info = await selectPerson(page)
    await info.getByRole("button", { name: "Go to definition" }).click()
    await tab(page, "core.ttl", 1).waitFor()
    assert.equal(await groups(page).count(), 2)
    await tabsAre(page, 0, ["start-here.ttl", "Graph*"])
    await tabsAre(page, 1, ["core.ttl*"])
    assert.equal(await groups(page).nth(0).locator("> .graph-host").isVisible(), true, "the graph stays visible in the first group")
    assert.equal(await groups(page).nth(1).locator("> .editor-host").isVisible(), true)
    const sel = await page.evaluate((u) => {
      const second = document.querySelectorAll("#groups > .group")[1]
      const ed = window.monaco.editor.getEditors().find((e) => e.getContainerDomNode().closest(".group") === second)
      return { uri: ed.getModel().uri.toString(), text: ed.getModel().getValueInRange(ed.getSelection()), line: ed.getSelection().startLineNumber }
    }, CORE)
    assert.equal(sel.uri, CORE)
    assert.match(sel.text, /Person/)
    const lines = (await page.evaluate((u) => window.monaco.editor.getModel(window.monaco.Uri.parse(u)).getValue(), CORE)).split("\n")
    assert.match(lines[sel.line - 1], /^ex:Person\b/, `line ${sel.line} defines ex:Person`)
    await page.waitForFunction(() => document.querySelector("#status-lang").textContent === "Turtle", null, { timeout: 5000 })
    // Once more with two groups: the same second group, no third.
    await (await selectPerson(page)).getByRole("button", { name: "Go to definition" }).click()
    assert.equal(await groups(page).count(), 2)
    await tabsAre(page, 1, ["core.ttl*"])
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Split with the graph tab active moves it to the other group, drawn, without asking again; split again moves it back (catches: a second view, a graph tab in both groups, a blank canvas after the move, the group it left vanishing)", async () => {
  const { page, errors, close } = await open()
  try {
    await openGraph(page)
    const calls = await graphCalls(page)
    await page.keyboard.press("ControlOrMeta+Backslash")
    await tab(page, "Graph", 1).waitFor()
    await tabsAre(page, 0, ["start-here.ttl*"])
    await tabsAre(page, 1, ["Graph*"])
    assert.equal(await groups(page).nth(1).locator("> .graph-host").isVisible(), true)
    assert.equal(await groups(page).nth(0).locator("> .editor-host").isVisible(), true, "the group it left shows its file")
    await page.waitForFunction(() => {
      const c = document.querySelector(".graph-host canvas")
      return c.getBoundingClientRect().width > 300 && c.width > 300
    })
    assert.ok((await ink(page)).inked > 300, "drawn at its new place")
    assert.equal(await host(page).count(), 1)
    // Split again, from the graph's group: back to the first group, and the empty group closes.
    await page.keyboard.press("ControlOrMeta+Backslash")
    await page.waitForFunction(() => document.querySelectorAll("#groups > .group").length === 1)
    await tabsAre(page, 0, ["start-here.ttl", "Graph*"])
    assert.equal(await host(page).isVisible(), true)
    assert.equal(await graphCalls(page), calls, "moving asks nothing")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("the graph tab closes by its close button, a middle click and the keyboard (Delete on the focused tab), and arrows move between tabs (catches: a tab only the mouse closes, the editor left hidden after the graph closes, the view made again on reopening)", async () => {
  const { page, errors, close } = await open()
  try {
    const gone = async () => {
      await tab(page, "Graph").waitFor({ state: "detached" })
      await tabsAre(page, 0, ["start-here.ttl*"])
      assert.equal(await page.locator("#groups > .group > .editor-host").isVisible(), true)
      await page.waitForFunction(() => document.querySelector("#status-lang").textContent === "Turtle")
    }
    await openGraph(page)
    const calls = await graphCalls(page)
    await tab(page, "Graph").locator("[data-close]").click()
    await gone()
    await openGraph(page)
    await tab(page, "Graph").click({ button: "middle" })
    await gone()
    await openGraph(page)
    // The keyboard: the selected tab is the one Tab reaches.
    const graphTab = tab(page, "Graph")
    assert.equal(await graphTab.getAttribute("tabindex"), "0")
    assert.equal(await tab(page, "start-here.ttl").getAttribute("tabindex"), "-1")
    await graphTab.focus()
    await page.keyboard.press("ArrowLeft")
    await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "start-here.ttl" && document.activeElement.getAttribute("aria-selected") === "true")
    assert.equal(await host(page).isHidden(), true)
    await page.keyboard.press("ArrowRight")
    await page.waitForFunction(() => document.activeElement?.getAttribute("aria-label") === "Graph" && document.activeElement.getAttribute("aria-selected") === "true")
    await page.keyboard.press("Delete")
    await gone()
    assert.equal(await graphCalls(page), calls, "reopened, the view is the same one: nothing asked")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("after a reload the graph tab is back where it was, and drawn; a server without the graph drops it silently (catches: the graph tab not kept, kept as a file that cannot open, or an error when the server cannot draw it)", async () => {
  const { page, errors, close } = await open()
  try {
    await openGraph(page)
    await page.keyboard.press("ControlOrMeta+Backslash")
    await tab(page, "Graph", 1).waitFor()
    await page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })
    await page.reload()
    await ready(page)
    await shown(page)
    await tabsAre(page, 0, ["start-here.ttl*"])
    await tabsAre(page, 1, ["Graph*"])
    assert.equal(await groups(page).nth(1).locator("> .graph-host").isVisible(), true)

    await page.evaluate(() => sessionStorage.setItem("no-graph", "1"))
    await page.reload()
    await ready(page)
    assert.equal(await page.locator("#act-graph").isHidden(), true)
    assert.equal(await groups(page).count(), 1, "the group that held only the graph is gone")
    await tabsAre(page, 0, ["start-here.ttl*"])
    assert.equal(await host(page).isHidden(), true)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("the Hierarchy mode lists the classes of the starter workspace, which declare no rdfs:subClassOf (catches: a tree that needs a subClassOf link before it shows a class)", async () => {
  const { page, errors, close } = await open()
  try {
    await openGraph(page)
    await host(page).getByRole("tab", { name: "Hierarchy" }).click()
    const rows = page.locator('.graph-host [role="treeitem"]')
    await rows.first().waitFor({ timeout: 10_000 })
    const names = await rows.allTextContents()
    assert.ok(names.some((n) => /Person/.test(n)), names.join(" | "))
    assert.ok(names.some((n) => /Organisation/.test(n)), names.join(" | "))
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("an edit refreshes the shown graph tab once, after the typing stops; a hidden graph tab asks nothing until shown, then once (catches: a request per keystroke, no refresh, refreshes while hidden)", async () => {
  const { page, errors, close } = await open()
  try {
    const [, total] = await openGraph(page)
    await idle(page)
    const calls = await graphCalls(page)
    // Three quick edits, as typing would make: one refresh after the last.
    for (const line of ["\nex:Robot a owl:Class ;", ' rdfs:label "Robot" ;', " rdfs:subClassOf ex:Person .\n"]) {
      await append(page, CORE, line)
      await page.waitForTimeout(100)
    }
    assert.equal(await graphCalls(page), calls, "nothing asked while the edits come")
    await page.waitForFunction((t) => /Showing [\d,]+ of ([\d,]+) nodes/.exec(document.querySelector(".graph-host .graph-status").textContent)?.[1].replaceAll(",", "") > t, total, { timeout: 10_000 })
    assert.equal(await graphCalls(page), calls + 1, "one refresh for the three edits")
    const search = page.getByRole("searchbox", { name: "Search nodes" })
    await search.fill("Robot")
    await page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /Robot/ }).waitFor()
    await search.fill("")

    // Hidden behind a file tab: edits ask nothing, even long after; showing the tab asks once.
    await tab(page, "start-here.ttl").click()
    await append(page, CORE, "ex:Drone a owl:Class .\n")
    await page.waitForTimeout(200)
    await append(page, CORE, "ex:Rover a owl:Class .\n")
    await page.waitForTimeout(800)
    assert.equal(await graphCalls(page), calls + 1)
    await tab(page, "Graph").click()
    await page.waitForFunction((n) => window.__wt.graph.length === n, calls + 2)
    await shown(page)
    await search.fill("Drone")
    await page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /Drone/ }).waitFor()
    await page.waitForTimeout(600)
    assert.equal(await graphCalls(page), calls + 2, "one request for both hidden edits")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

// ---- how long the worker is busy -------------------------------------------------------------------------

// Open start-here.ttl in the editor.
async function openStart(page) {
  await page.locator("#tree").getByText("start-here.ttl", { exact: true }).click()
  await page.getByRole("tab", { name: "start-here.ttl" }).waitFor()
}
// The graph tab beside start-here.ttl: opened in the first group, then split to the second.
async function graphBeside(page) {
  await openGraph(page)
  await page.keyboard.press("ControlOrMeta+Backslash")
  await tab(page, "Graph", 1).waitFor()
  // The group the graph left shows its neighbour tab: back to start-here.ttl.
  await tab(page, "start-here.ttl", 0).click()
  await page.locator('#groups > .group:first-child .tab[aria-selected="true"][aria-label="start-here.ttl"]').waitFor()
}

// Type `count` characters at the end of start-here.ttl (the file shown in the
// first group), `gap` ms apart, and return each one's time from didChange to
// the next diagnostics publish of the file.
async function keystrokes(page, count, gap) {
  await page.evaluate((u) => {
    const ed = window.monaco.editor.getEditors()[0]
    const m = ed.getModel()
    if (m.uri.toString() !== u) throw new Error(`the editor shows ${m.uri}`)
    const end = m.getFullModelRange().getEndPosition()
    ed.setPosition(end)
    ed.focus()
  }, START)
  await idle(page)
  const out = []
  for (let i = 0; i < count; i++) {
    const from = await page.evaluate(() => window.__wt.changes.length)
    await page.keyboard.type(i % 2 ? "\n" : "#")
    const ms = await page.evaluate(
      ([u, from]) =>
        new Promise((resolve) => {
          const tick = () => {
            const c = window.__wt.changes.slice(from).find((x) => x.uri === u)
            const d = c && window.__wt.diags.find((x) => x.uri === u && x.t >= c.t)
            if (d) resolve(d.t - c.t)
            else setTimeout(tick, 5)
          }
          tick()
        }),
      [START, from]
    )
    out.push(ms)
    await page.waitForTimeout(gap)
  }
  return out
}
const median = (a) => [...a].sort((x, y) => x - y)[Math.floor(a.length / 2)]
const fmt = (a) => `median ${median(a).toFixed(0)} ms, max ${Math.max(...a).toFixed(0)} ms (${a.map((x) => x.toFixed(0)).join(", ")})`

// Wide enough that the graph's group, half the editor area, keeps its filters beside the drawing.
const WIDE = 1680

test("typing beside the graph tab stays responsive: keystroke to diagnostics, with and without the graph (catches: a graph request per keystroke, or a refresh that keeps the worker for seconds)", async (t) => {
  const { page, errors, close } = await open({ width: WIDE })
  try {
    await openStart(page)
    const closed = await keystrokes(page, 6, 500)
    await graphBeside(page)
    // 500 ms apart: each keystroke lands just after the refresh the one before it asked for.
    const opened = await keystrokes(page, 6, 500)
    const graph = await page.evaluate(() => window.__wt.graph)
    t.diagnostic(`keystroke → diagnostics, graph closed: ${fmt(closed)}`)
    t.diagnostic(`keystroke → diagnostics, graph beside: ${fmt(opened)}`)
    t.diagnostic(`turtle/graph on the starter workspace: ${graph.map((g) => `${g.ms.toFixed(0)} ms (iterations ${g.iterations}, ${g.nodes} of ${g.total})`).join("; ")}`)
    assert.ok(graph.length >= 2, "the edits refreshed the graph")
    assert.ok(Math.max(...opened) < 1000, `a keystroke is answered within a second with the graph beside: ${fmt(opened)}`)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

/** N classes, each with a label, a definition and a subClassOf link (a tree of fan-out 8, plus a second parent for every 10th). */
function vocabulary(n) {
  let t = "@prefix v: <http://example.org/vocab#> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n\n"
  for (let i = 0; i < n; i++) {
    const sup = i ? [`v:C${Math.floor((i - 1) / 8)}`] : []
    if (i > 50 && i % 10 === 0) sup.push(`v:C${Math.floor(((i * 0.6180339887) % 1) * i)}`)
    t += `v:C${i} a owl:Class ;\n    rdfs:label "Class ${i}" ;\n    skos:definition "The class number ${i}, generated."${sup.length ? ` ;\n    rdfs:subClassOf ${sup.join(", ")}` : ""} .\n`
  }
  return t
}

test("a 17,000-class vocabulary opens in focus mode, and a keystroke waits for the graph request at most (catches: drawing 17,000 nodes first; reports how long the worker is blocked)", async (t) => {
  const { page, errors, close } = await open({ files: { "vocabulary.ttl": vocabulary(17000) }, width: WIDE })
  try {
    await openStart(page)
    const closed = await keystrokes(page, 4, 300)
    await graphBeside(page)
    const [n, total] = await shown(page)
    assert.ok(n > 0 && n <= total, `Showing ${n} of ${total}`)
    assert.match(await page.locator(".graph-host .graph-filters").textContent(), /vocabulary\.ttl17,000/, "the files facet counts the whole vocabulary")
    assert.equal(await host(page).getByRole("tab", { name: "Focus" }).getAttribute("aria-selected"), "true")
    const first = await page.evaluate(() => window.__wt.graph)
    // A keystroke in start-here.ttl makes a refresh of the whole graph; the next keystroke waits for it.
    const opened = await keystrokes(page, 4, 500)
    const graph = await page.evaluate(() => window.__wt.graph)
    t.diagnostic(`turtle/graph, 17,000 classes, opening: ${first.map((g) => `${g.ms.toFixed(0)} ms (iterations ${g.iterations}${g.focus ? ", focus" : ""}, ${g.nodes} of ${g.total})`).join("; ")}`)
    t.diagnostic(`turtle/graph, 17,000 classes, refreshes: ${graph.slice(first.length).map((g) => `${g.ms.toFixed(0)} ms (${g.nodes} of ${g.total})`).join("; ")}`)
    t.diagnostic(`keystroke → diagnostics, 17,000 classes, graph closed: ${fmt(closed)}`)
    t.diagnostic(`keystroke → diagnostics, 17,000 classes, graph beside: ${fmt(opened)}`)

    // The whole graph (mode A) at the default cap, then at the largest: each
    // request lays the graph out on the worker. Keystrokes 600 ms apart land
    // while the refresh the one before asked for runs.
    const graphMode = async (label, change) => {
      const from = await page.evaluate(() => window.__wt.graph.length)
      await change()
      await page.waitForFunction((n) => window.__wt.graph.length > n, from, { timeout: 60_000 })
      await shown(page)
      const typed = await keystrokes(page, 4, 600)
      await page.waitForTimeout(200)
      await shown(page)
      const all = await page.evaluate(() => window.__wt.graph)
      t.diagnostic(`turtle/graph, 17,000 classes, ${label}: ${all.slice(from).map((g) => `${g.ms.toFixed(0)} ms (iterations ${g.iterations}, ${g.nodes} of ${g.total})`).join("; ")}`)
      t.diagnostic(`keystroke → diagnostics, 17,000 classes, ${label}: ${fmt(typed)}`)
      return typed
    }
    await graphMode("graph mode, cap 2,000", () => host(page).getByRole("tab", { name: "Graph" }).click())
    await graphMode("graph mode, cap 5,000", () => page.getByRole("combobox", { name: "Node cap" }).selectOption("5000"))
    // The playground offers no cap above 5,000 (its layout runs on the editor's worker).
    const caps = await page.getByRole("combobox", { name: "Node cap" }).locator("option").allTextContents()
    assert.deepEqual(caps, ["500", "2,000", "5,000"])
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
