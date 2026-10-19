// The editor's Settings page (web/app/settings.js) in the installed Google
// Chrome, driven by Playwright: Ctrl+, opens it, search filters, turtle-lsp
// settings change what the server answers, editor settings change the
// editor, settings survive a reload and Reset restores the default.
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
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors, close: () => ctx.close() }
}

const idle = (page) => page.locator("body[data-busy='false']").waitFor()
const settingsPage = (page) => page.getByRole("region", { name: "Settings" })
const control = (page, name) => settingsPage(page).getByLabel(name, { exact: true })

// Open Settings with Ctrl+, from inside the editor.
async function openSettings(page) {
  await page.locator(".group .monaco-editor textarea").first().focus()
  await page.keyboard.press("ControlOrMeta+Comma")
  await settingsPage(page).waitFor()
}

// Markers of `p` whose message matches `re`.
const markerCount = (page, p, re) =>
  page.evaluate(
    ([p, src]) => window.monaco.editor.getModelMarkers({ resource: window.monaco.Uri.parse(`file:///work/${p}`) }).filter((m) => new RegExp(src, "i").test(m.message)).length,
    [p, re.source]
  )
const labelsIs = (page, text) => page.locator("#status-labels", { hasText: new RegExp(`^${text}$`) }).waitFor()
const shaclCount = (page) => page.locator("#status-shacl").textContent().then(Number)

test("Ctrl+, opens Settings, the search box filters, Escape closes", async () => {
  const { page, errors, close } = await open()
  try {
    await openSettings(page)
    const search = page.getByRole("searchbox", { name: "Search settings" })
    assert.equal(await search.evaluate((el) => el === document.activeElement), true, "Ctrl+, focuses the search box")
    const all = await settingsPage(page).locator(".stg-item:visible").count()
    assert.equal(all, 15)
    await search.fill("shacl")
    const titles = await settingsPage(page).locator(".stg-item:visible .stg-title b").allTextContents()
    assert.deepEqual(titles, ["Enabled", "Validate While Typing", "Max Triples"])
    assert.equal(await settingsPage(page).locator(".stg-count").textContent(), "3 Settings Found")
    assert.equal(await settingsPage(page).getByRole("heading", { name: "Editor" }).isVisible(), false)
    await search.fill("zzzz")
    await settingsPage(page).getByText("No settings found.").waitFor()
    await search.press("Escape")
    assert.equal(await search.inputValue(), "")
    assert.equal(await settingsPage(page).locator(".stg-item:visible").count(), all)
    await search.press("Escape")
    await settingsPage(page).waitFor({ state: "hidden" })
    // The gear button and the palette open it too.
    await page.getByRole("button", { name: "Settings", exact: true }).click()
    await settingsPage(page).waitFor()
    await page.getByRole("button", { name: "Close Settings" }).click()
    await settingsPage(page).waitFor({ state: "hidden" })
    await page.locator(".group .monaco-editor textarea").first().focus()
    await page.keyboard.press("F1")
    await page.keyboard.type("Preferences: Open Settings")
    await page.keyboard.press("Enter")
    await settingsPage(page).waitFor()
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

// A collection stays on one line only when it fits the line width.
test("Format: Line Width changes what Format Document writes", async () => {
  const { page, errors, close } = await open()
  try {
    const long = `@prefix ex: <https://toolbox.example.org/def/> .\n@prefix id: <https://toolbox.example.org/id/> .\n\nid:alice ex:knows ( id:bob id:carol id:dave id:erin id:frank id:grace id:heidi id:ivan ) .\n`
    const format = () =>
      page.evaluate(async (text) => {
        const ed = window.monaco.editor.getEditors()[0]
        ed.getModel().setValue(text)
        await ed.getAction("editor.action.formatDocument").run()
        return ed.getModel().getValue()
      }, long)
    await openSettings(page)
    await control(page, "Format: Line Width").fill("40")
    const narrow = await format()
    await control(page, "Format: Line Width").fill("120")
    const wide = await format()
    assert.notEqual(narrow, wide)
    assert.ok(narrow.split("\n").filter((l) => !l.startsWith("@prefix")).every((l) => l.length <= 40), narrow)
    assert.ok(wide.split("\n").some((l) => l.includes("id:bob") && l.includes("id:ivan")), wide)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("SHACL: Enabled off removes the violations, on brings them back", async () => {
  const { page, errors, close } = await open()
  try {
    const on = await shaclCount(page)
    assert.ok(on > 0)
    await openSettings(page)
    await control(page, "SHACL: Enabled").uncheck()
    await page.waitForFunction(() => document.getElementById("status-shacl").textContent === "0")
    await control(page, "SHACL: Enabled").check()
    await page.waitForFunction((n) => document.getElementById("status-shacl").textContent === String(n), on)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Inlay Hints: Labels all shows label hints and the status-bar button follows", async () => {
  const { page, errors, close } = await open()
  try {
    await page.locator('#tree [data-path="data/people.ttl"]').click()
    await idle(page)
    const button = page.locator("#status-labels")
    await labelsIs(page, "Labels: Opaque")
    const HINTS = ".monaco-editor [class*='dyn-rule'], .monaco-editor .inlay-hint, .monaco-editor [class*='inlay']"
    const hints = () => page.locator(HINTS).count()
    const before = await hints()
    await openSettings(page)
    await control(page, "Inlay Hints: Labels").selectOption("all")
    await labelsIs(page, "Labels: All")
    await page.waitForFunction(([sel, n]) => document.querySelectorAll(sel).length > n, [HINTS, before])
    // The button goes back to the other mode, and the select follows.
    await button.click()
    await labelsIs(page, "Labels: Opaque")
    assert.equal(await control(page, "Inlay Hints: Labels").inputValue(), "opaque")
    await control(page, "Inlay Hints: Labels").selectOption("off")
    await button.click()
    await labelsIs(page, "Labels: All")
    await button.click()
    await labelsIs(page, "Labels: Off")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("an added prefix declares rdfs: everywhere; removing it brings the problem back", async () => {
  const { page, errors, close } = await open()
  try {
    const undeclared = () => markerCount(page, "data/people.ttl", /rdfs/)
    assert.ok((await undeclared()) > 0, "data/people.ttl uses rdfs: without declaring it")
    await openSettings(page)
    await settingsPage(page).getByRole("button", { name: "Add Prefix" }).click()
    await settingsPage(page).getByLabel("Prefix name").fill("rdfs")
    await settingsPage(page).getByLabel("Namespace IRI").fill("http://www.w3.org/2000/01/rdf-schema#")
    await page.waitForFunction(() => {
      const uri = window.monaco.Uri.parse("file:///work/data/people.ttl")
      return !window.monaco.editor.getModelMarkers({ resource: uri }).some((m) => /rdfs/i.test(m.message))
    })
    await settingsPage(page).getByRole("button", { name: "Remove Prefix rdfs" }).click()
    await page.waitForFunction(() => {
      const uri = window.monaco.Uri.parse("file:///work/data/people.ttl")
      return window.monaco.editor.getModelMarkers({ resource: uri }).some((m) => /rdfs/i.test(m.message))
    })
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Tab Size, Word Wrap, Minimap and Font Size apply to the editor and its models", async () => {
  const { page, errors, close } = await open()
  try {
    const opts = () =>
      page.evaluate(() => {
        const ed = window.monaco.editor.getEditors()[0]
        const O = window.monaco.editor.EditorOption
        return { wrap: ed.getOption(O.wordWrap), minimap: ed.getOption(O.minimap).enabled, font: ed.getOption(O.fontSize), tabs: window.monaco.editor.getModels().map((m) => m.getOptions().tabSize) }
      })
    const start = await opts()
    assert.deepEqual([start.wrap, start.minimap, start.font], ["off", true, 13])
    await openSettings(page)
    await control(page, "Editor: Detect Indentation").uncheck()
    await control(page, "Editor: Tab Size").fill("2")
    await control(page, "Editor: Word Wrap").selectOption("on")
    await control(page, "Minimap: Enabled").uncheck()
    await control(page, "Editor: Font Size").fill("16")
    const now = await opts()
    assert.deepEqual([now.wrap, now.minimap, now.font], ["on", false, 16])
    assert.ok(now.tabs.length > 3 && now.tabs.every((t) => t === 2), JSON.stringify(now.tabs))
    // A file created afterwards gets them too.
    const fresh = await page.evaluate(() => {
      const m = window.monaco.editor.createModel("a\n", "turtle", window.monaco.Uri.parse("inmemory://x.ttl"))
      const o = m.getOptions()
      m.dispose()
      return [o.tabSize, o.insertSpaces]
    })
    assert.deepEqual(fresh, [2, true])
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("settings survive a reload; Reset and Reset All restore the defaults", async () => {
  const { page, errors, close } = await open()
  try {
    await openSettings(page)
    await control(page, "Format: Line Width").fill("60")
    await control(page, "Minimap: Enabled").uncheck()
    await control(page, "Inlay Hints: Labels").selectOption("all")
    assert.equal(await settingsPage(page).locator(".stg-item.modified").count(), 3)
    await page.waitForTimeout(500) // the workspace is kept 300 ms after a change
    await page.reload()
    await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
    await labelsIs(page, "Labels: All")
    assert.equal(await page.evaluate(() => window.monaco.editor.getEditors()[0].getOption(window.monaco.editor.EditorOption.minimap).enabled), false)
    await openSettings(page)
    assert.equal(await control(page, "Format: Line Width").inputValue(), "60")
    assert.equal(await control(page, "Minimap: Enabled").isChecked(), false)

    await settingsPage(page).getByRole("button", { name: "Reset Format: Line Width" }).click()
    assert.equal(await control(page, "Format: Line Width").inputValue(), "100")
    assert.equal(await settingsPage(page).locator(".stg-item.modified").count(), 2)
    await settingsPage(page).getByRole("button", { name: "Reset All" }).click()
    assert.equal(await settingsPage(page).locator(".stg-item.modified").count(), 0)
    assert.equal(await control(page, "Minimap: Enabled").isChecked(), true)
    assert.equal(await page.evaluate(() => window.monaco.editor.getEditors()[0].getOption(window.monaco.editor.EditorOption.minimap).enabled), true)
    await labelsIs(page, "Labels: Opaque")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
