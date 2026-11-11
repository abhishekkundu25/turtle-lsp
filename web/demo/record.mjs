// Records the README's demos of the browser editor (web/app) in the installed
// Google Chrome, driven by Playwright, and turns each recording into an MP4
// and a GIF in assets/:
//
//   sparql-reasoning   a SPARQL query over a tiny class hierarchy, run with
//                      Inferred off (no rows) and then with TurbOWL's default
//                      rules (the inferred rows)
//   graph-view         the workspace graph: rdf:type edges on, zoom, pan,
//                      search and select a node, its focus view, the class
//                      hierarchy
//   hover-proof        hover on an instance: its inferred types, each with
//                      its proof, and a proof's link to its line
//
// Run from the repository root:
//
//   npm ci && npm run web:build
//   node web/demo/record.mjs                    every demo
//   node web/demo/record.mjs sparql-reasoning   one or more by name
//
// Needs Google Chrome (as the web tests do) and ffmpeg on PATH, or FFMPEG set
// to its path. KEEP=1 keeps the recorded frames in the temporary folder and
// prints where. This file is not a test: the web tests run
// web/test/*.test.mjs only.
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium } from "playwright-core"
import { listen } from "../test/static-server.mjs"

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "..")
const ASSETS = path.join(ROOT, "assets")
const FFMPEG = process.env.FFMPEG ?? "ffmpeg"

// The page's size in CSS pixels, unless a demo sets its own; it is recorded
// at twice that, so text stays sharp in the crop.
const VIEWPORT = { width: 1120, height: 700 }
const SCALE = 2
// The page size of the demo being recorded, and the box it records (CSS pixels).
let viewport = VIEWPORT
let shown = { x: 0, y: 0, ...VIEWPORT }
// The GIF's width in pixels and frames per second; the MP4 keeps the
// recorded size (at most 1600 wide) at 30 fps.
const GIF_WIDTH = 960
const GIF_FPS = 15

const ANIMALS = `@prefix ex: <http://example.org/> .
@prefix owl: <http://www.w3.org/2002/07/owl#> .
@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .

# A small class hierarchy.
ex:Animal a owl:Class .
ex:Mammal a owl:Class ; rdfs:subClassOf ex:Animal .
ex:Bird   a owl:Class ; rdfs:subClassOf ex:Animal .
ex:Dog    a owl:Class ; rdfs:subClassOf ex:Mammal .
ex:Cat    a owl:Class ; rdfs:subClassOf ex:Mammal .
ex:Parrot a owl:Class ; rdfs:subClassOf ex:Bird .
ex:Person a owl:Class .

ex:owns a owl:ObjectProperty ;
  rdfs:domain ex:Person .

# No triple here says that any of them is an ex:Animal.
ex:rex   a ex:Dog ;    rdfs:label "Rex" .
ex:tom   a ex:Cat ;    rdfs:label "Tom" .
ex:polly a ex:Parrot ; rdfs:label "Polly" .
ex:alice rdfs:label "Alice" ;
  ex:owns ex:rex, ex:polly .
`

// The query's head; the demo types the rest.
const QUERY_HEAD = `PREFIX ex: <http://example.org/>
PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>

# Which resources are animals?
`
const QUERY_BODY = `SELECT ?animal ?name WHERE {
  ?animal a ex:Animal ;
          rdfs:label ?name .
}
ORDER BY ?name
`

const FILES = { "animals.ttl": ANIMALS, "animals.rq": QUERY_HEAD }

const pause = (page, ms) => page.waitForTimeout(ms)
const ready = (page) => page.locator("body[data-ready='true']").waitFor({ timeout: 30_000 })
const idle = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 60_000 })
const editorOf = (page) => page.locator("#groups > .group").first()

// A caption card, as in assets/demo.gif; a null title hides it. It sits in
// the recorded box: at its bottom left, or `top` pixels below the box's top,
// at its right.
async function caption(page, title, sub = "", top = null) {
  const right = `${viewport.width - shown.x - shown.width + 16}px`
  const place = top == null ? { left: `${shown.x + 16}px`, right: "auto", top: "auto", bottom: `${viewport.height - shown.y - shown.height + 16}px` } : { left: "auto", right, top: `${shown.y + top}px`, bottom: "auto" }
  await page.evaluate(
    ([title, sub, place]) => {
      let el = document.getElementById("demo-caption")
      if (!el) {
        el = document.createElement("div")
        el.id = "demo-caption"
        el.style.cssText =
          "position:fixed;z-index:100000;pointer-events:none;display:flex;gap:12px;align-items:center;padding:12px 18px 12px 14px;border-radius:10px;background:rgba(18,24,38,.94);border:1px solid rgba(255,255,255,.12);box-shadow:0 8px 24px rgba(0,0,0,.45);color:#e8edf5;font:600 16px/1.3 system-ui,-apple-system,'Segoe UI',sans-serif;transition:opacity .25s"
        document.body.append(el)
      }
      if (title == null) return void (el.style.opacity = "0")
      el.innerHTML = `<div><div>${title}</div>${sub ? `<div style="font-weight:400;font-size:13px;opacity:.75;margin-top:2px">${sub}</div>` : ""}</div>`
      Object.assign(el.style, place)
      el.style.opacity = "1"
    },
    [title, sub, place]
  )
}

// A visible pointer: headless Chrome draws none, so the recording shows this
// dot where the mouse is.
function pointer() {
  addEventListener("DOMContentLoaded", () => {
    const dot = document.createElement("div")
    dot.id = "demo-pointer"
    dot.style.cssText =
      "position:fixed;left:-40px;top:-40px;width:18px;height:18px;margin:-3px 0 0 -3px;z-index:100001;pointer-events:none;background:url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='18' height='18' viewBox='0 0 18 18'%3E%3Cpath d='M2 1l13 8-6 1.5L6 16z' fill='white' stroke='black' stroke-width='1.2' stroke-linejoin='round'/%3E%3C/svg%3E\") no-repeat"
    document.documentElement.append(dot)
    const move = (e) => ((dot.style.left = e.clientX + "px"), (dot.style.top = e.clientY + "px"))
    addEventListener("pointermove", move, true)
    addEventListener("pointerdown", move, true)
  })
}

// Move the mouse to (x, y) in steps, as a hand would.
async function glide(page, x, y, steps = 18) {
  await page.mouse.move(x, y, { steps })
}

async function clickOn(page, locator, wait = 250) {
  const b = await locator.boundingBox()
  await glide(page, b.x + b.width / 2, b.y + b.height / 2)
  await pause(page, wait)
  await page.mouse.click(b.x + b.width / 2, b.y + b.height / 2)
}

// The middle of the first `text` in the editor (its line revealed), in page pixels.
async function textAt(page, text) {
  return page.evaluate((text) => {
    const ed = window.monaco.editor.getEditors()[0]
    const m = ed.getModel()
    const [match] = m.findMatches(text, false, false, true, null, false)
    const { startLineNumber: lineNumber, startColumn, endColumn } = match.range
    ed.revealLineInCenterIfOutsideViewport(lineNumber)
    const a = ed.getScrolledVisiblePosition({ lineNumber, column: startColumn })
    const b = ed.getScrolledVisiblePosition({ lineNumber, column: endColumn })
    const r = ed.getDomNode().getBoundingClientRect()
    return { x: r.left + (a.left + b.left) / 2, y: r.top + a.top + a.height / 2 }
  }, text)
}

// Opens the editor with only FILES in its workspace and `file` shown.
async function setUp(page, file) {
  await page.goto(origin + "/index.html")
  await ready(page)
  await idle(page)
  // An empty workspace, then the demo's files.
  await page.locator(".monaco-editor").first().click()
  await page.keyboard.press("F1")
  await page.keyboard.type("Remove All Files")
  await pause(page, 300)
  await page.keyboard.press("Enter")
  await page.locator("#tree [role=treeitem]").first().waitFor({ state: "detached" })
  await page.locator("#files-input").setInputFiles(Object.entries(FILES).map(([name, body]) => ({ name, mimeType: "text/plain", buffer: Buffer.from(body) })))
  await page.locator("#tree").getByText("animals.ttl", { exact: true }).waitFor()
  await openFile(page, file)
  await idle(page)
  // No Problems panel: the editor group gets the page's height.
  await page.keyboard.press("F1")
  await page.keyboard.type("View: Toggle Panel")
  await pause(page, 300)
  await page.keyboard.press("Enter")
  await page.locator(".wb.panel-closed").waitFor()
  await page.locator(".toast").waitFor({ state: "detached", timeout: 15_000 }).catch(() => page.evaluate(() => document.querySelectorAll(".toast").forEach((t) => t.remove())))
  // Monaco a little larger, without the minimap.
  await page.evaluate(() => window.monaco.editor.getEditors().forEach((e) => e.updateOptions({ fontSize: 14, minimap: { enabled: false } })))
  await page.mouse.move(-10, -10)
  await pause(page, 1200)
}

async function openFile(page, name) {
  await page.locator(`[role=treeitem][data-path="${name}"]`).click()
  await page.getByRole("tab", { name }).waitFor()
}

let origin, browser

// Records `page` by taking screenshots one after another (Chrome's
// screencast gives CSS pixels, a screenshot gives device pixels): each a JPEG
// of `clip` (CSS pixels) in `dir`, with the second it was taken. About 15 to
// 20 a second. `stop()` returns them.
function screenshots(page, dir, clip) {
  const frames = []
  let stopped = false
  const loop = (async () => {
    while (!stopped) {
      const t = Date.now() / 1000
      const file = path.join(dir, `f${String(frames.length).padStart(5, "0")}.jpg`)
      try {
        await page.screenshot({ path: file, type: "jpeg", quality: 92, clip, caret: "initial", animations: "allow" })
      } catch (e) {
        if (stopped) break
        throw e
      }
      frames.push({ file, t })
    }
  })()
  return {
    async stop() {
      const end = Date.now() / 1000
      stopped = true
      await loop
      return { frames, end }
    },
  }
}

// One demo: a page of `size` set up with `file` shown, then `act(page)`
// recorded in the box `crop(page)` gives (the whole page when it gives null).
async function record(name, { file, act, crop = async () => null, size = VIEWPORT }) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), `demo-${name}-`))
  viewport = size
  const context = await browser.newContext({ viewport, deviceScaleFactor: SCALE, colorScheme: "dark" })
  const page = await context.newPage()
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  await page.addInitScript(pointer)
  const errors = []
  page.on("pageerror", (e) => errors.push(String(e)))
  await setUp(page, file)
  const box = (await crop(page)) ?? { x: 0, y: 0, ...viewport }
  shown = { x: Math.round(box.x), y: Math.round(box.y), width: Math.floor(box.width / 2) * 2, height: Math.floor(box.height / 2) * 2 }
  const rec = screenshots(page, dir, shown)
  let recorded
  try {
    await act(page)
  } finally {
    recorded = await rec.stop()
    await context.close()
  }
  const { frames, end } = recorded
  if (errors.length) console.warn(`${name}: page errors:\n  ${errors.join("\n  ")}`)
  encode(name, dir, frames, end)
  if (process.env.KEEP) console.log(`${name}: frames in ${dir}`)
  else fs.rmSync(dir, { recursive: true, force: true })
}

function encode(name, dir, frames, end) {
  if (frames.length < 2) throw new Error(`${name}: ${frames.length} frames recorded`)
  // The concat demuxer's list: each frame for as long as it was on screen.
  const lines = []
  frames.forEach((f, i) => lines.push(`file '${f.file}'`, `duration ${Math.max(0.001, (frames[i + 1]?.t ?? end) - f.t).toFixed(4)}`))
  lines.push(`file '${frames.at(-1).file}'`)
  const list = path.join(dir, "frames.txt")
  fs.writeFileSync(list, lines.join("\n") + "\n")
  const length = end - frames[0].t
  const ff = (...args) => execFileSync(FFMPEG, ["-v", "error", "-y", ...args], { stdio: "inherit" })
  const input = ["-f", "concat", "-safe", "0", "-i", list]
  const mp4 = path.join(ASSETS, `${name}.mp4`)
  const gif = path.join(ASSETS, `${name}.gif`)
  // MP4: H.264, yuv420p in TV range (the JPEGs are full range), at most 1600
  // wide, moov atom first for streaming.
  const mp4Filters = ["scale='min(1600,iw)':-2:flags=lanczos:in_range=full:out_range=tv:out_color_matrix=bt709", "fps=30", "format=yuv420p"].join(",")
  const bt709 = ["-colorspace", "bt709", "-color_primaries", "bt709", "-color_trc", "bt709", "-color_range", "tv"]
  ff(...input, "-vf", mp4Filters, "-c:v", "libx264", "-preset", "slow", "-crf", "23", "-tune", "stillimage", "-pix_fmt", "yuv420p", ...bt709, "-movflags", "+faststart", "-an", mp4)
  // GIF: one palette for the whole clip; only the changed parts of a frame are stored.
  const base = [`fps=${GIF_FPS}`, `scale=${GIF_WIDTH}:-2:flags=lanczos`].join(",")
  ff(...input, "-filter_complex", `${base},split[a][b];[a]palettegen=max_colors=128:stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=4:diff_mode=rectangle`, gif)
  const size = (f) => `${(fs.statSync(f).size / 1e6).toFixed(2)} MB`
  console.log(`${name}: ${path.relative(ROOT, mp4)} ${size(mp4)}, ${path.relative(ROOT, gif)} ${size(gif)}, ${length.toFixed(1)} s, ${frames.length} frames`)
}

// --- the demos ----------------------------------------------------------------

const graphIdle = (page) => page.locator(".graph-host .graph[data-busy='false'] .graph-status", { hasText: /Showing/ }).waitFor({ timeout: 60_000 })

// A wheel turned in `steps` notches at (x, y): negative `dy` zooms the graph in.
async function wheel(page, x, y, dy, steps = 8) {
  await glide(page, x, y)
  for (let i = 0; i < steps; i++) {
    await page.mouse.wheel(0, dy / steps)
    await pause(page, 60)
  }
}

// A drag from (x, y) by (dx, dy), slowly.
async function drag(page, x, y, dx, dy) {
  await glide(page, x, y)
  await page.mouse.down()
  await page.mouse.move(x + dx, y + dy, { steps: 30 })
  await page.mouse.up()
}

const DEMOS = {
  "sparql-reasoning": {
    file: "animals.rq",
    size: { width: 1100, height: 600 },
    crop: (page) => editorOf(page).boundingBox(),
    async act(page) {
      await caption(page, "SPARQL over inferred triples", "A query over a small class hierarchy", 150)
      await page.evaluate(() => {
        const ed = window.monaco.editor.getEditors()[0]
        // Typed by hand: no completion popups, brackets or indents of the editor's own.
        ed.updateOptions({ quickSuggestions: false, suggestOnTriggerCharacters: false, autoClosingBrackets: "never", autoIndent: "none", acceptSuggestionOnEnter: "off", wordBasedSuggestions: "off", parameterHints: { enabled: false } })
        ed.setPosition({ lineNumber: ed.getModel().getLineCount(), column: 1 })
        ed.focus()
      })
      await pause(page, 1500)
      await page.keyboard.type(QUERY_BODY, { delay: 60 })
      await pause(page, 1000)
      const select = page.getByRole("combobox", { name: "Inferred", exact: true })
      const run = page.getByRole("button", { name: "Run Query" })
      const pane = page.getByRole("region", { name: "Query results" })
      // First without inference: no file says that anything is an animal.
      await caption(page, "Inferred: Off", "Only the triples the files hold", 150)
      await clickOn(page, select)
      await select.selectOption("off")
      await pause(page, 1000)
      await clickOn(page, run)
      await pane.getByText("No results.").waitFor()
      await pause(page, 3000)
      // Then with TurbOWL's default OWL 2 RL rules.
      await caption(page, "Inferred: Default rules", "Also what TurbOWL infers (OWL 2 RL)", 150)
      await clickOn(page, select)
      await select.selectOption("default")
      await pause(page, 1000)
      await clickOn(page, run)
      await pane.locator("table").waitFor()
      const b = await pane.boundingBox()
      await glide(page, b.x + 260, b.y + 110)
      await pause(page, 5000)
    },
  },

  "graph-view": {
    file: "animals.ttl",
    crop: (page) => editorOf(page).boundingBox(),
    async act(page) {
      await caption(page, "Workspace graph", "The whole workspace, drawn")
      await pause(page, 600)
      await clickOn(page, page.locator("#act-graph"))
      await graphIdle(page)
      await pause(page, 2200)
      // Instances joined to their classes.
      await caption(page, "rdf:type edges", "Each instance joined to its class")
      await clickOn(page, page.getByRole("checkbox", { name: "rdf:type edges" }))
      await graphIdle(page)
      await pause(page, 2500)
      const c = await page.locator(".graph-host canvas").boundingBox()
      // Zoom in where the classes are, pan, and fit again.
      await caption(page, "Zoom and pan", "Wheel to zoom, drag to pan")
      await wheel(page, c.x + c.width * 0.5, c.y + c.height * 0.45, -260)
      await pause(page, 1200)
      await drag(page, c.x + c.width * 0.6, c.y + c.height * 0.6, 70, 40)
      await pause(page, 1200)
      await clickOn(page, page.getByRole("button", { name: "Fit", exact: true }))
      await pause(page, 1500)
      // Search for a node and select it.
      await caption(page, "Search and select", "The node, its types and its links")
      const search = page.getByRole("searchbox", { name: "Search nodes" })
      await clickOn(page, search)
      await page.keyboard.type("rex", { delay: 140 })
      await page.getByRole("group", { name: "Search results" }).getByRole("button").first().waitFor()
      await pause(page, 900)
      await page.keyboard.press("Enter")
      await page.getByRole("region", { name: "Selected node" }).waitFor()
      await glide(page, c.x + c.width / 2 + 40, c.y + c.height / 2 + 40)
      await pause(page, 3200)
      // Its neighbours, then the class tree.
      await caption(page, "Focus", "The selected node and its neighbours")
      await clickOn(page, page.getByRole("tab", { name: "Focus" }))
      await graphIdle(page)
      await pause(page, 3000)
      await caption(page, "Hierarchy", "The class tree")
      await clickOn(page, page.getByRole("tab", { name: "Hierarchy" }))
      await pause(page, 1500)
      for (const name of ["ex:Mammal", "ex:Bird"]) {
        const row = page.locator(".graph-tree .graph-row", { has: page.locator(".graph-tree-name", { hasText: new RegExp(`^${name}$`) }) })
        await clickOn(page, row.locator(".graph-twist"))
        await pause(page, 900)
      }
      await pause(page, 2500)
    },
  },

  "hover-proof": {
    file: "animals.ttl",
    size: { width: 1100, height: 640 },
    // The editor's left part: the code and the hover.
    crop: async (page) => {
      const b = await editorOf(page).boundingBox()
      return { x: b.x, y: b.y, width: 620, height: b.height }
    },
    async act(page) {
      await caption(page, "Inferred types on hover", "Each with its proof, one W3C rule per step")
      await pause(page, 1500)
      // ex:rex: a Dog, so a Mammal, so an Animal.
      const rex = await textAt(page, "rex   a")
      await glide(page, rex.x - 18, rex.y, 30)
      const hover = page.locator(".monaco-hover", { hasText: "Inferred" })
      await hover.waitFor({ timeout: 20_000 })
      await pause(page, 5000)
      // ex:alice owns a pet, so she is an ex:Person (the domain of ex:owns).
      await page.mouse.move(rex.x + 400, rex.y + 200, { steps: 12 })
      await hover.waitFor({ state: "hidden" })
      await pause(page, 600)
      const alice = await textAt(page, "alice rdfs")
      await glide(page, alice.x - 20, alice.y, 30)
      await hover.waitFor({ timeout: 20_000 })
      await pause(page, 3500)
      // Each triple of a proof links to its line: here ex:owns's domain.
      await caption(page, "Proofs link to their lines", "Click one to go to the triple")
      await clickOn(page, hover.locator("li a", { hasText: /^animals\.ttl:\d+$/ }).first(), 600)
      await pause(page, 3500)
    },
  },
}

const names = process.argv.slice(2)
for (const n of names) if (!DEMOS[n]) throw new Error(`no demo "${n}"; the demos: ${Object.keys(DEMOS).join(", ")}`)
const server = await listen()
origin = server.origin
browser = await chromium.launch({ channel: "chrome", headless: true })
try {
  for (const n of names.length ? names : Object.keys(DEMOS)) await record(n, DEMOS[n])
} finally {
  await browser.close()
  await server.close()
}
