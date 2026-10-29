// The editor's Graph view (graph/view.js mounted by editor.js) in the
// installed Google Chrome, driven by Playwright: the button the server's
// capability brings, the starter workspace drawn, a node opened in the
// editor, a refresh after an edit (and none while hidden), the theme, the
// expanded view, and how long turtle/graph keeps the engine's worker from
// answering the editor. Run `npm run web:build` first.
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
function instrument() {
  const wt = (window.__wt = { graph: [], diags: [], sent: new Map(), changes: [] })
  const Base = window.Worker
  window.Worker = class extends Base {
    constructor(...a) {
      super(...a)
      this.addEventListener("message", ({ data }) => {
        const t = performance.now()
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

async function open({ files } = {}) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  await page.addInitScript(instrument)
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  if (files) {
    await page.locator("#files-input").setInputFiles(Object.entries(files).map(([name, body]) => ({ name, mimeType: "text/plain", buffer: Buffer.from(body) })))
    await page.locator("#tree").getByText(Object.keys(files)[0], { exact: true }).waitFor()
    await idle(page)
  }
  return { page, errors, close: () => ctx.close() }
}

const idle = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 60_000 })
const status = (page) => page.locator("#view-graph .graph-status")
const graphCalls = (page) => page.evaluate(() => window.__wt.graph.length)
// Wait until the view has answered and says "Showing N of M nodes"; returns [N, M].
async function shown(page) {
  await page.locator("#view-graph .graph[data-busy='false'] .graph-status", { hasText: /Showing/ }).waitFor({ timeout: 60_000 })
  const m = (await status(page).textContent()).match(/Showing ([\d,]+) of ([\d,]+) nodes/)
  return [Number(m[1].replaceAll(",", "")), Number(m[2].replaceAll(",", ""))]
}
// Pixels of the graph's canvas that differ from its top-left corner (its background), and that corner.
const ink = (page) =>
  page.evaluate(() => {
    const c = document.querySelector("#view-graph canvas, .graph-host canvas")
    const d = c.getContext("2d").getImageData(0, 0, c.width, c.height).data
    let n = 0
    for (let i = 4; i < d.length; i += 4) if (d[i] !== d[0] || d[i + 1] !== d[1] || d[i + 2] !== d[2]) n++
    return { inked: n, bg: [d[0], d[1], d[2]] }
  })

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

const CORE = "file:///work/ontology/core.ttl"
const START = "file:///work/start-here.ttl"

test("the Graph button opens the view on the starter workspace: drawn, with Showing N of M, themed, and expandable over the editor (catches: button not wired to the capability, a blank canvas, a theme the canvas never reads, an expand that does not return)", async () => {
  const { page, errors, close } = await open()
  try {
    const button = page.getByRole("button", { name: "Graph", exact: true })
    await button.waitFor()
    assert.equal(await page.locator("#view-graph").isHidden(), true)
    assert.equal(await graphCalls(page), 0, "nothing is asked before the view is opened")
    await button.focus()
    await page.keyboard.press("Enter")
    assert.equal(await page.locator("#act-graph").getAttribute("class"), "act on")
    const [n, total] = await shown(page)
    assert.ok(n > 0 && n <= total, `Showing ${n} of ${total}`)
    const dark = await ink(page)
    assert.ok(dark.inked > 300, `the canvas has nodes and edges drawn (${dark.inked} pixels)`)

    // Light theme from Settings: the canvas is redrawn on a light background.
    await page.locator(".group .monaco-editor textarea").first().focus()
    await page.keyboard.press("ControlOrMeta+Comma")
    await page.getByRole("region", { name: "Settings" }).getByLabel("Appearance: Color Theme", { exact: true }).selectOption("light")
    await page.waitForFunction(() => document.documentElement.dataset.theme === "light")
    await page.waitForFunction((bg) => {
      const c = document.querySelector("#graph-host canvas")
      const d = c.getContext("2d").getImageData(0, 0, 1, 1).data
      return d[0] !== bg[0] || d[1] !== bg[1] || d[2] !== bg[2]
    }, dark.bg)
    const light = await ink(page)
    assert.ok(light.bg[0] > dark.bg[0] + 100, `light background ${light.bg} after dark ${dark.bg}`)
    await page.keyboard.press("Escape")

    // Expanded, the view covers the editor area and the panel; then back to the side bar.
    const box = async (sel) => page.locator(sel).boundingBox()
    const before = await box("#graph-host")
    await page.getByRole("button", { name: "Expand to Editor Area" }).click()
    const center = await box("main.center")
    const panel = await box(".panel")
    const wide = await box("#graph-host")
    assert.ok(Math.abs(wide.x - center.x) < 2 && Math.abs(wide.width - center.width) < 2, "covers the editor area's width")
    assert.ok(Math.abs(wide.y - center.y) < 2 && Math.abs(wide.y + wide.height - (panel.y + panel.height)) < 2, "covers the editor and the panel")
    assert.equal(await page.locator("#graph-away").isVisible(), true)
    await page.waitForFunction(() => document.querySelector("#graph-host canvas").getBoundingClientRect().width > 700)
    assert.ok((await ink(page)).inked > 300, "drawn at its new size")
    await page.getByRole("button", { name: "Back to the side bar" }).first().click()
    const back = await box("#graph-host")
    assert.deepEqual([Math.round(back.x), Math.round(back.width)], [Math.round(before.x), Math.round(before.width)])
    assert.equal(await page.locator("#graph-away").isHidden(), true)
    // Another view hides it; Graph shows it again without asking (nothing changed).
    const calls = await graphCalls(page)
    await page.locator("#act-explorer").click()
    assert.equal(await page.locator("#graph-host").isHidden(), true)
    await page.locator("#act-graph").click()
    await shown(page)
    assert.equal(await graphCalls(page), calls)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("a node clicked in the view opens its definition in the editor with Go to definition (catches: reveal not mapped from the node's URI to the workspace file, or the range off by one)", async () => {
  const { page, errors, close } = await open()
  try {
    await page.locator("#act-graph").click()
    await shown(page)
    const search = page.getByRole("searchbox", { name: "Search nodes" })
    await search.fill("Person")
    await page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /ex:Person|Person/ }).first().waitFor()
    await search.press("Enter")
    const info = page.getByRole("region", { name: "Selected node" })
    await info.waitFor()
    // Centred by the search: a click at the canvas centre selects it too.
    await page.keyboard.press("Escape")
    await info.waitFor({ state: "hidden" })
    const b = await page.locator("#graph-host canvas").boundingBox()
    await page.mouse.click(b.x + b.width / 2, b.y + b.height / 2)
    await info.waitFor()
    assert.match(await info.textContent(), /Person/)
    await info.getByRole("button", { name: "Go to definition" }).click()
    const sel = await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      return { uri: ed.getModel().uri.toString(), text: ed.getModel().getValueInRange(ed.getSelection()), line: ed.getSelection().startLineNumber, focused: ed.hasTextFocus() }
    })
    assert.equal(sel.uri, CORE)
    assert.match(sel.text, /Person/)
    const t = await page.evaluate((u) => window.monaco.editor.getModel(window.monaco.Uri.parse(u)).getValue(), CORE)
    const lines = t.split("\n")
    assert.match(lines[sel.line - 1], /^ex:Person\b/, `line ${sel.line} defines ex:Person`)
    assert.equal(await page.getByRole("tab", { name: "core.ttl" }).getAttribute("aria-selected"), "true")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("the Hierarchy tab lists the classes of the starter workspace, which declare no rdfs:subClassOf (catches: a tree that needs a subClassOf link before it shows a class)", async () => {
  const { page, errors, close } = await open()
  try {
    await page.locator("#act-graph").click()
    await shown(page)
    await page.getByRole("tab", { name: "Hierarchy" }).click()
    const rows = page.locator('#graph-host [role="treeitem"]')
    await rows.first().waitFor({ timeout: 10_000 })
    const names = await rows.allTextContents()
    assert.ok(names.some((n) => /Person/.test(n)), names.join(" | "))
    assert.ok(names.some((n) => /Organisation/.test(n)), names.join(" | "))
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("an edit refreshes the open view once, after the typing stops; a hidden view asks nothing until shown (catches: a request per keystroke, no refresh, refreshes while hidden)", async () => {
  const { page, errors, close } = await open()
  try {
    await page.locator("#act-graph").click()
    const [, total] = await shown(page)
    await idle(page)
    const calls = await graphCalls(page)
    // Three quick edits, as typing would make: one refresh after the last.
    for (const line of ["\nex:Robot a owl:Class ;", ' rdfs:label "Robot" ;', " rdfs:subClassOf ex:Person .\n"]) {
      await append(page, CORE, line)
      await page.waitForTimeout(100)
    }
    assert.equal(await graphCalls(page), calls, "nothing asked while the edits come")
    await page.waitForFunction((t) => /Showing [\d,]+ of ([\d,]+) nodes/.exec(document.querySelector("#view-graph .graph-status").textContent)?.[1].replaceAll(",", "") > t, total, { timeout: 10_000 })
    assert.equal(await graphCalls(page), calls + 1, "one refresh for the three edits")
    const search = page.getByRole("searchbox", { name: "Search nodes" })
    await search.fill("Robot")
    await page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /Robot/ }).waitFor()
    await search.fill("")

    // Hidden: an edit asks nothing, even long after; showing the view asks once.
    await page.locator("#act-explorer").click()
    await append(page, CORE, "ex:Drone a owl:Class .\n")
    await page.waitForTimeout(800)
    assert.equal(await graphCalls(page), calls + 1)
    await page.locator("#act-graph").click()
    await page.waitForFunction((n) => window.__wt.graph.length === n, calls + 2)
    await shown(page)
    await search.fill("Drone")
    await page.getByRole("group", { name: "Search results" }).getByRole("button", { name: /Drone/ }).waitFor()
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

// Type `count` characters at the end of start-here.ttl (the file shown),
// `gap` ms apart, and return each one's time from didChange to the next
// diagnostics publish of the file.
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

test("typing while the view is open stays responsive: keystroke to diagnostics, with and without the view (catches: a graph request per keystroke, or a refresh that keeps the worker for seconds)", async (t) => {
  const { page, errors, close } = await open()
  try {
    await openStart(page)
    const closed = await keystrokes(page, 6, 500)
    await page.locator("#act-graph").click()
    await shown(page)
    // 500 ms apart: each keystroke lands just after the refresh the one before it asked for.
    const opened = await keystrokes(page, 6, 500)
    const graph = await page.evaluate(() => window.__wt.graph)
    t.diagnostic(`keystroke → diagnostics, view closed: ${fmt(closed)}`)
    t.diagnostic(`keystroke → diagnostics, view open:   ${fmt(opened)}`)
    t.diagnostic(`turtle/graph on the starter workspace: ${graph.map((g) => `${g.ms.toFixed(0)} ms (iterations ${g.iterations}, ${g.nodes} of ${g.total})`).join("; ")}`)
    assert.ok(graph.length >= 2, "the edits refreshed the view")
    assert.ok(Math.max(...opened) < 1000, `a keystroke is answered within a second with the view open: ${fmt(opened)}`)
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
  const { page, errors, close } = await open({ files: { "vocabulary.ttl": vocabulary(17000) } })
  try {
    await openStart(page)
    const closed = await keystrokes(page, 4, 300)
    await page.locator("#act-graph").click()
    const [n, total] = await shown(page)
    assert.ok(n > 0 && n <= total, `Showing ${n} of ${total}`)
    assert.match(await page.locator("#view-graph .graph-filters").textContent(), /vocabulary\.ttl17,000/, "the files facet counts the whole vocabulary")
    assert.equal(await page.getByRole("tab", { name: "Focus" }).getAttribute("aria-selected"), "true")
    const first = await page.evaluate(() => window.__wt.graph)
    // A keystroke in start-here.ttl makes a refresh of the whole graph; the next keystroke waits for it.
    const opened = await keystrokes(page, 4, 500)
    const graph = await page.evaluate(() => window.__wt.graph)
    t.diagnostic(`turtle/graph, 17,000 classes, opening: ${first.map((g) => `${g.ms.toFixed(0)} ms (iterations ${g.iterations}${g.focus ? ", focus" : ""}, ${g.nodes} of ${g.total})`).join("; ")}`)
    t.diagnostic(`turtle/graph, 17,000 classes, refreshes: ${graph.slice(first.length).map((g) => `${g.ms.toFixed(0)} ms (${g.nodes} of ${g.total})`).join("; ")}`)
    t.diagnostic(`keystroke → diagnostics, 17,000 classes, view closed: ${fmt(closed)}`)
    t.diagnostic(`keystroke → diagnostics, 17,000 classes, view open:   ${fmt(opened)}`)

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
    await graphMode("graph mode, cap 2,000", () => page.getByRole("tab", { name: "Graph" }).click())
    // In the side bar the filters are folded behind "Filters".
    await page.getByRole("button", { name: "Filters" }).click()
    await graphMode("graph mode, cap 5,000", () => page.getByRole("combobox", { name: "Node cap" }).selectOption("5000"))
    // The playground offers no cap above 5,000 (its layout runs on the editor's worker).
    const caps = await page.getByRole("combobox", { name: "Node cap" }).locator("option").allTextContents()
    assert.deepEqual(caps, ["500", "2,000", "5,000"])
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
