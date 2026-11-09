// SPARQL queries in the editor (web/app/query-pane.js) in the installed
// Google Chrome, driven by Playwright: the Run button on a query file, the
// results pane (a table, true or false, Turtle, an error at its line:column),
// Ctrl/Cmd+Enter, and closing the pane. Run `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium } from "playwright-core"

const app = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "app")
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".ttf": "font/ttf" }

let server, origin, browser
// Serve a query worker that fails to load, for the next request of it.
let brokenWorker = false
// While set, serve the real worker behind one that acts on a marker in the
// query's text: "#hang" says it started and then keeps the thread busy (a
// query that runs for minutes), "#crash" answers as an engine panic does.
let hookedWorker = false
const HOOKED_WORKER = `import "./worker.js?real"
const real = self.onmessage
self.onmessage = (ev) => {
  const { id, announce, files, query } = ev.data
  const text = files?.find((f) => f.name === query)?.text ?? ""
  if (text.includes("#hang")) {
    if (announce) self.postMessage({ id, started: true })
    const end = Date.now() + 120_000
    while (Date.now() < end);
  } else if (text.includes("#crash")) self.postMessage({ id, error: "RuntimeError: unreachable", crashed: true })
  else real(ev)
}`
before(async () => {
  server = http.createServer((req, res) => {
    const rel = decodeURIComponent(new URL(req.url, "http://x").pathname)
    if (rel === "/worker.js" && brokenWorker) {
      brokenWorker = false
      return res.writeHead(200, { "content-type": "text/javascript", "cache-control": "no-store" }).end('throw new Error("boom")')
    }
    if (rel === "/worker.js" && hookedWorker && !req.url.includes("?real")) return res.writeHead(200, { "content-type": "text/javascript", "cache-control": "no-store" }).end(HOOKED_WORKER)
    const file = path.join(app, path.normalize(rel))
    if (!file.startsWith(app) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return res.writeHead(404).end()
    res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream" })
    fs.createReadStream(file).pipe(res)
  })
  await new Promise((r) => server.listen(0, "127.0.0.1", r))
  origin = `http://127.0.0.1:${server.address().port}`
  browser = await chromium.launch({ channel: "chrome", headless: true })
})
after(async () => {
  await browser?.close()
  await new Promise((r) => server.close(r))
})

async function open({ context } = {}) {
  const ctx = context ?? (await browser.newContext({ viewport: { width: 1440, height: 900 } }))
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors, close: () => ctx.close() }
}

const item = (page, p) => page.locator(`[role=treeitem][data-path="${p}"]`)
const openFile = async (page, p) => {
  await item(page, p).click()
  await page.getByRole("tab", { name: p.split("/").pop() }).waitFor()
}
// Replace the active editor's text and put the focus in it.
const setText = (page, text) =>
  page.evaluate((text) => {
    const ed = window.monaco.editor.getEditors()[0]
    ed.getModel().setValue(text)
    ed.focus()
  }, text)
const runButton = (page) => page.getByRole("button", { name: "Run Query" })
const pane = (page) => page.getByRole("region", { name: "Query results" })

test("Run shows a query's rows in a table under the editor; the pane closes and follows its file", async () => {
  const { page, errors, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    await runButton(page).waitFor({ state: "hidden" })
    await openFile(page, "queries/people.rq")
    await runButton(page).click()
    const table = pane(page).locator("table")
    await table.waitFor()
    assert.deepEqual(await table.locator("th").allTextContents(), ["?name"])
    const cells = await table.locator("td").allTextContents()
    assert.ok(cells.includes('"Alice"'), cells.join(", "))
    assert.match(await pane(page).getByRole("status").textContent(), new RegExp(`^${cells.length} rows · \\d+ ms$`))
    // Another file hides the pane; the query's file shows it again.
    await openFile(page, "data/people.ttl")
    await pane(page).waitFor({ state: "hidden" })
    await openFile(page, "queries/people.rq")
    await pane(page).waitFor()
    await pane(page).getByRole("button", { name: "Close Results" }).click()
    await pane(page).waitFor({ state: "hidden" })
    assert.ok(await page.evaluate(() => window.monaco.editor.getEditors()[0].hasTextFocus()))
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Ctrl+Enter runs the query: true or false, Turtle, and an error at its line:column", async () => {
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    const body = pane(page).locator(".qpane-body")
    await setText(page, "ASK { id:alice a ex:Person }")
    await page.keyboard.press("ControlOrMeta+Enter")
    await body.getByText("true", { exact: true }).waitFor()
    await setText(page, "CONSTRUCT { ?p ex:employer ?o } WHERE { ?p ex:worksFor ?o }")
    await page.keyboard.press("ControlOrMeta+Enter")
    await body.locator("pre", { hasText: "ex:employer" }).waitFor()
    assert.match(await pane(page).getByRole("status").textContent(), /^\d+ triples · \d+ ms$/)
    await setText(page, "SELECT ?s\nWHERE { ?s a }")
    await page.keyboard.press("ControlOrMeta+Enter")
    const at = body.getByRole("button", { name: "2:14" })
    await at.waitFor()
    assert.match(await body.textContent(), /syntax error/)
    await at.click()
    assert.deepEqual(await page.evaluate(() => window.monaco.editor.getEditors()[0].getPosition()), { lineNumber: 2, column: 14 })
    // In a Turtle file Ctrl+Enter is left to the editor.
    await openFile(page, "data/people.ttl")
    await runButton(page).waitFor({ state: "hidden" })
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

const inferred = (page) => page.getByRole("combobox", { name: "Inferred", exact: true })
const note = (page) => pane(page).locator(".qpane-note")

test("on a fresh visit a query includes TurbOWL's inferred triples and says so; Inferred off leaves both out", async () => {
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    assert.equal(await inferred(page).inputValue(), "default")
    await runButton(page).click()
    await pane(page).locator("table").waitFor()
    assert.match(await note(page).textContent(), /^Includes [\d,]+ inferred triples \(TurbOWL, default rules\)(; TurbOWL reported \d+ findings?)?$/)
    // Bob works for Alice, and ex:worksFor's range is ex:Organisation: only
    // inference makes id:alice one.
    const body = pane(page).locator(".qpane-body")
    await setText(page, "SELECT ?o WHERE { ?o a ex:Organisation } ORDER BY ?o")
    await page.keyboard.press("ControlOrMeta+Enter")
    await body.locator("td", { hasText: "id:alice" }).waitFor()
    assert.deepEqual(await body.locator("td").allTextContents(), ["id:acme", "id:alice"])
    await inferred(page).selectOption("off")
    await runButton(page).click()
    await page.waitForFunction(() => document.querySelectorAll(".qpane-body td").length === 1)
    assert.deepEqual(await body.locator("td").allTextContents(), ["id:acme"])
    assert.equal(await note(page).isVisible(), false)
    assert.equal(await note(page).textContent(), "")
    // The choice is the setting `query.reasoning`, both ways.
    await page.keyboard.press("ControlOrMeta+Comma")
    const settings = page.getByRole("region", { name: "Settings" })
    const setting = settings.getByRole("combobox", { name: "Query: Inferred Triples" })
    assert.equal(await setting.inputValue(), "off")
    await setting.selectOption("complete")
    await page.keyboard.press("Escape")
    await settings.waitFor({ state: "hidden" })
    assert.equal(await inferred(page).inputValue(), "complete")
    await runButton(page).click()
    await body.locator("td", { hasText: "id:alice" }).waitFor()
    assert.match(await note(page).textContent(), /^Includes [\d,]+ inferred triples \(TurbOWL, complete rules\)/)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("a prefix only Settings declares is known to the query, as it is to the editor", async () => {
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    const body = pane(page).locator(".qpane-body")
    const text = "SELECT ?p WHERE { ?p a zz:Person } ORDER BY ?p"
    await setText(page, text)
    await page.keyboard.press("ControlOrMeta+Enter")
    await body.getByText("Undeclared prefix 'zz:'").waitFor()
    // Settings → Prefixes: zz:, in no file.
    await page.keyboard.press("ControlOrMeta+Comma")
    const settings = page.getByRole("region", { name: "Settings" })
    await settings.waitFor()
    await settings.getByRole("button", { name: "Add Prefix" }).click()
    await settings.getByLabel("Prefix name").fill("zz")
    await settings.getByLabel("Namespace IRI").fill("https://toolbox.example.org/def/")
    await page.locator("body[data-busy='false']").waitFor()
    await page.keyboard.press("Escape")
    await settings.waitFor({ state: "hidden" })
    await setText(page, text)
    await page.keyboard.press("ControlOrMeta+Enter")
    await body.locator("table").waitFor()
    assert.ok((await body.locator("td").allTextContents()).some((c) => c.startsWith("id:")))
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("a query worker that fails to load gives an error, and the next run starts a new one", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    brokenWorker = true
    await runButton(page).click()
    await pane(page).locator(".qerror", { hasText: "the query engine stopped" }).waitFor({ timeout: 10_000 })
    assert.equal(await runButton(page).isEnabled(), true)
    await runButton(page).click()
    await pane(page).locator("table").waitFor()
  } finally {
    await close()
  }
})

const cancelButton = (page) => page.getByRole("button", { name: "Cancel Query" })

test("while a query runs, Run turns into Cancel: it ends the query, and the next run answers in a fresh worker", async () => {
  hookedWorker = true
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    const body = pane(page).locator(".qpane-body")
    const status = pane(page).getByRole("status")
    // A run with Inferred on leaves a note; the next run clears it while it runs.
    await runButton(page).click()
    await body.locator("table").waitFor()
    assert.equal(await note(page).isVisible(), true)
    await setText(page, "#hang\nASK { ?s ?p ?o }")
    await page.keyboard.press("ControlOrMeta+Enter")
    await cancelButton(page).waitFor()
    assert.equal(await status.textContent(), "Running…")
    assert.equal(await note(page).isVisible(), false)
    assert.equal(await runButton(page).count(), 0)
    // By keyboard: the focused Cancel, then Enter.
    await cancelButton(page).focus()
    await page.keyboard.press("Enter")
    await body.getByText("Cancelled.", { exact: true }).waitFor({ timeout: 5_000 })
    assert.equal(await status.textContent(), "cancelled")
    assert.equal(await body.getAttribute("aria-busy"), null)
    // The button is Run again and keeps the focus. The old worker is still
    // busy for two minutes, so an answer comes from a new one.
    assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("aria-label")), "Run Query")
    await setText(page, "ASK { id:alice a ex:Person }")
    await runButton(page).click()
    await body.getByText("true", { exact: true }).waitFor({ timeout: 10_000 })
    assert.deepEqual(errors, [])
  } finally {
    hookedWorker = false
    await close()
  }
})

test("a query past its time limit and the grace after it is stopped, and says so", async () => {
  hookedWorker = true
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    // The default limit is 30 seconds: a grace of -29.5 s ends it at 0.5 s.
    await page.evaluate(async () => ((await import("/query-pane.js")).watchdog.graceMs = -29_500))
    const body = pane(page).locator(".qpane-body")
    await setText(page, "#hang\nASK { ?s ?p ?o }")
    await page.keyboard.press("ControlOrMeta+Enter")
    await cancelButton(page).waitFor()
    await body.getByText(/^Stopped: the query ran past its time limit of 30 seconds\./).waitFor({ timeout: 10_000 })
    assert.match(await body.textContent(), /try Inferred off/)
    assert.equal(await pane(page).getByRole("status").textContent(), "stopped")
    await runButton(page).waitFor()
    await setText(page, "ASK { id:alice a ex:Person }")
    await runButton(page).click()
    await body.getByText("true", { exact: true }).waitFor({ timeout: 10_000 })
    assert.deepEqual(errors, [])
  } finally {
    hookedWorker = false
    await close()
  }
})

test("an engine crash with Inferred on says to try Inferred off; with it off, it does not", async () => {
  hookedWorker = true
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    const body = pane(page).locator(".qpane-body")
    await setText(page, "#crash\nASK { ?s ?p ?o }")
    await runButton(page).click()
    const error = body.locator(".qerror")
    await error.waitFor()
    assert.match(await error.textContent(), /RuntimeError: unreachable/)
    assert.match(await error.textContent(), /try Inferred off/)
    await inferred(page).selectOption("off")
    await runButton(page).click()
    await page.waitForFunction(() => !document.querySelector(".qpane-body[aria-busy]") && document.querySelector(".qerror"))
    assert.match(await error.textContent(), /RuntimeError: unreachable/)
    assert.doesNotMatch(await error.textContent(), /Inferred off/)
    assert.deepEqual(errors, [])
  } finally {
    hookedWorker = false
    await close()
  }
})

test("the suggestion list shows its rows in full (page styles stay out of Monaco's widgets)", async () => {
  const { page, errors, close } = await open()
  try {
    await openFile(page, "queries/people.rq")
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.getModel().setValue("PREFIX ex: <http://example.org/>\nselect ?name where { ?p a ex:Person ; ex:name ?na }\n")
      ed.setPosition({ lineNumber: 2, column: 50 })
      ed.focus()
    })
    await page.keyboard.press("Control+Space")
    const row = page.locator(".suggest-widget .monaco-list-row").first()
    await row.waitFor()
    const [list, item] = await Promise.all([
      page.locator(".suggest-widget .monaco-list").boundingBox(),
      row.boundingBox(),
    ])
    assert.ok(list.height >= item.height, `list ${list.height}px, row ${item.height}px`)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
