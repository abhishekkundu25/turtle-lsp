// The editor's Type Hierarchy view (web/app/hierarchy.js) in the installed
// Google Chrome, driven by Playwright: subtypes and supertypes of a class,
// children loaded on expand, opening a row, and the message off a class.
// Run `npm run web:build` first.
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

async function open() {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/editor.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors, close: () => ctx.close() }
}

const CORE = "file:///work/ontology/core.ttl"
const ADDED = "\nex:Employee rdfs:subClassOf ex:Person .\nex:Manager rdfs:subClassOf ex:Employee .\n"

// Open ontology/core.ttl and add Employee ⊑ Person and Manager ⊑ Employee
// through its Monaco model, as typing would.
async function addHierarchy(page) {
  await page.locator("#tree").getByText("core.ttl", { exact: true }).click()
  await page.getByRole("tab", { name: "core.ttl" }).waitFor()
  await page.evaluate(
    ([uri, text]) => {
      const m = window.monaco.editor.getModel(window.monaco.Uri.parse(uri))
      const end = m.getFullModelRange().getEndPosition()
      m.pushEditOperations([], [{ range: new window.monaco.Range(end.lineNumber, end.column, end.lineNumber, end.column), text }], () => null)
    },
    [CORE, ADDED]
  )
}

// Put the cursor in the first `word` of core.ttl (after the first `from`)
// and run Show Type Hierarchy with Shift+Alt+H.
async function showAt(page, word, from = "") {
  await page.evaluate(
    ([uri, word, from]) => {
      const ed = window.monaco.editor.getEditors().find((e) => e.getModel()?.uri.toString() === uri)
      const v = ed.getModel().getValue()
      const at = ed.getModel().getPositionAt(v.indexOf(word, from ? v.indexOf(from) : 0) + 1)
      ed.setPosition(at)
      ed.focus()
    },
    [CORE, word, from]
  )
  await page.keyboard.press("Shift+Alt+H")
}

const rows = (page) => page.locator("#view-hierarchy .th-row")
const names = (page) => page.$$eval("#view-hierarchy .th-row", (r) => r.map((x) => [Number(x.getAttribute("aria-level")), x.querySelector(".th-name").textContent]))
async function waitNames(page, want) {
  await page.waitForFunction((n) => document.querySelectorAll("#view-hierarchy .th-row").length === n && !document.querySelector("#view-hierarchy .codicon-loading"), want.length)
  assert.deepEqual(await names(page), want)
}

test("Subtypes of ex:Person list ex:Employee, and expanding it lists ex:Manager", async () => {
  const { page, errors, close } = await open()
  try {
    await addHierarchy(page)
    await showAt(page, "ex:Person a owl:Class")
    await page.locator("#view-hierarchy").waitFor()
    assert.equal(await page.locator("#act-hierarchy").getAttribute("class"), "act on")
    assert.equal(await page.getByRole("button", { name: "Show Subtypes" }).getAttribute("aria-pressed"), "true")
    await waitNames(page, [[1, "ex:Person"], [2, "ex:Employee"]])
    const employee = rows(page).nth(1)
    assert.match(await employee.locator(".th-detail").textContent(), /core\.ttl/)
    assert.equal(await employee.getAttribute("aria-expanded"), "false")
    await employee.locator(".th-twist").click()
    await waitNames(page, [[1, "ex:Person"], [2, "ex:Employee"], [3, "ex:Manager"]])
    assert.equal(await rows(page).nth(1).getAttribute("aria-expanded"), "true")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Supertypes of ex:Manager list ex:Employee, then ex:Person", async () => {
  const { page, errors, close } = await open()
  try {
    await addHierarchy(page)
    await showAt(page, "ex:Manager")
    await waitNames(page, [[1, "ex:Manager"]])
    assert.match(await page.locator("#view-hierarchy .th-none").textContent(), /No subtypes/)
    await page.getByRole("button", { name: "Show Supertypes" }).click()
    assert.equal(await page.getByRole("button", { name: "Show Supertypes" }).getAttribute("aria-pressed"), "true")
    await waitNames(page, [[1, "ex:Manager"], [2, "ex:Employee"]])
    await rows(page).nth(1).locator(".th-twist").click()
    await waitNames(page, [[1, "ex:Manager"], [2, "ex:Employee"], [3, "ex:Person"]])
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("reopening the view asks again; clicking a row opens its file with the definition selected", async () => {
  const { page, errors, close } = await open()
  try {
    await addHierarchy(page)
    await showAt(page, "ex:Person a owl:Class")
    await waitNames(page, [[1, "ex:Person"], [2, "ex:Employee"]])
    // Leave core.ttl for another file, then open Employee from the view.
    await page.locator("#act-explorer").click()
    await page.locator("#tree").getByText("people.ttl", { exact: true }).click()
    await page.getByRole("tab", { name: "people.ttl" }).waitFor()
    // Reopening the view keeps the item and asks again for its subtypes.
    await page.evaluate((u) => {
      const m = window.monaco.editor.getModel(window.monaco.Uri.parse(u))
      const end = m.getFullModelRange().getEndPosition()
      m.pushEditOperations([], [{ range: new window.monaco.Range(end.lineNumber, end.column, end.lineNumber, end.column), text: "ex:Intern rdfs:subClassOf ex:Person .\n" }], () => null)
    }, CORE)
    await page.locator("#act-hierarchy").click()
    await waitNames(page, [[1, "ex:Person"], [2, "ex:Employee"], [2, "ex:Intern"]])
    await rows(page).nth(1).locator(".th-name").click()
    const sel = await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      return { uri: ed.getModel().uri.toString(), text: ed.getModel().getValueInRange(ed.getSelection()), line: ed.getSelection().startLineNumber }
    })
    assert.equal(sel.uri, CORE)
    assert.match(sel.text, /Employee/)
    const t = await page.evaluate((u) => window.monaco.editor.getModel(window.monaco.Uri.parse(u)).getValue(), CORE)
    assert.equal(sel.line, t.slice(0, t.indexOf("ex:Employee rdfs:subClassOf")).split("\n").length)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("off a class or property the view says so", async () => {
  const { page, errors, close } = await open()
  try {
    await addHierarchy(page)
    await showAt(page, "@prefix")
    const msg = page.locator("#view-hierarchy .th-msg")
    await msg.waitFor()
    assert.match(await msg.textContent(), /No class or property at the cursor/)
    assert.equal(await rows(page).count(), 0)
    // Show Type Hierarchy on a class replaces the message.
    await showAt(page, "ex:Person a owl:Class")
    await waitNames(page, [[1, "ex:Person"], [2, "ex:Employee"]])
    assert.equal(await msg.isHidden(), true)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
