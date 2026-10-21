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
before(async () => {
  server = http.createServer((req, res) => {
    const rel = decodeURIComponent(new URL(req.url, "http://x").pathname)
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
