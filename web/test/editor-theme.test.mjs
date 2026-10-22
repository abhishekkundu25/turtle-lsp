// The editor's colour theme (web/app/theme.js) in the installed Google
// Chrome, driven by Playwright: Dark (the default) and Light in Settings
// force the page's colours and the editors' theme whatever the system says,
// Auto follows the system and its changes, the choice is there from the first
// paint after a reload, and Reset All returns to Dark. Run `npm run web:build`
// first.
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

const ready = (page) => page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })

// Open the editor with the system set to `system`. Every page load records,
// at DOMContentLoaded (before the editor is ready), the theme attribute and
// the body background in window.__firstPaint.
// Wait until the workspace is written to the browser's storage (the page says so in body[data-saved]).
const saved = (page) => page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })

async function open(system) {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, colorScheme: system })
  await ctx.addInitScript(() => {
    document.addEventListener("DOMContentLoaded", () => {
      window.__firstPaint = { theme: document.documentElement.dataset.theme, bg: getComputedStyle(document.body).backgroundColor, ready: document.body.dataset.ready }
    })
  })
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  const foreign = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  page.on("request", (r) => {
    const u = r.url()
    if (!u.startsWith(origin) && !u.startsWith("data:") && !u.startsWith("blob:")) foreign.push(u)
  })
  await page.goto(origin + "/index.html")
  await ready(page)
  return { page, errors, foreign, close: () => ctx.close() }
}

const LIGHT_BG = "rgb(255, 255, 255)"
const DARK_BG = "rgb(30, 30, 30)"
const bodyBg = (page) => page.evaluate(() => getComputedStyle(document.body).backgroundColor)
// The theme class ("vs" or "vs-dark") of every Monaco editor under `scope`
// (the diff editor's gutter is a .monaco-editor without one).
const editorThemes = (page, scope = ".group") =>
  page.evaluate((scope) => [...document.querySelectorAll(`${scope} .monaco-editor:not(.gutter)`)].map((el) => (el.classList.contains("vs-dark") ? "vs-dark" : el.classList.contains("vs") ? "vs" : "?")), scope)

// The page and the editor are `mode` ("light" or "dark").
async function looks(page, mode) {
  const cls = mode === "dark" ? "vs-dark" : "vs"
  await page.waitForFunction(
    ([mode, cls]) => document.documentElement.dataset.theme === mode && [...document.querySelectorAll(".group .monaco-editor")].every((el) => el.classList.contains(cls)),
    [mode, cls]
  )
  assert.equal(await bodyBg(page), mode === "dark" ? DARK_BG : LIGHT_BG)
  const themes = await editorThemes(page)
  assert.ok(themes.length > 0 && themes.every((t) => t === cls), JSON.stringify(themes))
}

const settingsPage = (page) => page.getByRole("region", { name: "Settings" })
const themeControl = (page) => settingsPage(page).getByLabel("Appearance: Color Theme", { exact: true })
async function openSettings(page) {
  await page.locator(".group .monaco-editor textarea").first().focus()
  await page.keyboard.press("ControlOrMeta+Comma")
  await settingsPage(page).waitFor()
}
const stored = (page) => page.evaluate(() => localStorage.getItem("rdf-editor-theme"))

test("Light under a dark system makes the page and the editor light; Dark under a light system makes them dark", async () => {
  for (const [system, forced] of [["dark", "light"], ["light", "dark"]]) {
    const { page, errors, foreign, close } = await open(system)
    try {
      await looks(page, "dark")
      await openSettings(page)
      await themeControl(page).selectOption("auto")
      await looks(page, system)
      await themeControl(page).selectOption(forced)
      await looks(page, forced)
      assert.equal(await stored(page), forced)
      // The system changing does not move a forced theme.
      await page.emulateMedia({ colorScheme: forced })
      await page.emulateMedia({ colorScheme: system })
      await page.waitForTimeout(100)
      await looks(page, forced)
      assert.deepEqual(errors, [])
      assert.deepEqual(foreign, [])
    } finally {
      await close()
    }
  }
})

test("Auto follows the system and its changes while open", async () => {
  const { page, errors, foreign, close } = await open("light")
  try {
    // A first visit is dark from the first paint, whatever the system says.
    assert.equal((await page.evaluate(() => window.__firstPaint)).bg, DARK_BG)
    await looks(page, "dark")
    assert.equal(await stored(page), "dark")
    await openSettings(page)
    await themeControl(page).selectOption("auto")
    await looks(page, "light")
    assert.equal(await stored(page), "auto")
    await page.emulateMedia({ colorScheme: "dark" })
    await looks(page, "dark")
    await page.emulateMedia({ colorScheme: "light" })
    await looks(page, "light")
    // Back to Auto from a forced theme picks the system up again.
    await themeControl(page).selectOption("dark")
    await looks(page, "dark")
    await themeControl(page).selectOption("auto")
    await looks(page, "light")
    assert.equal(await stored(page), "auto")
    // Auto is there from the first paint after a reload.
    await saved(page)
    await page.reload()
    assert.equal((await page.evaluate(() => window.__firstPaint)).bg, LIGHT_BG)
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("the choice survives a reload and is there at the first paint; Reset All returns to Dark", async () => {
  const { page, errors, foreign, close } = await open("dark")
  try {
    const first = await page.evaluate(() => window.__firstPaint)
    assert.equal(first.ready, "false")
    assert.equal(first.bg, DARK_BG, "the default paints dark")
    await openSettings(page)
    await themeControl(page).selectOption("light")
    await looks(page, "light")
    await saved(page)
    await page.reload()
    const probe = await page.evaluate(() => window.__firstPaint)
    assert.deepEqual(probe, { theme: "light", bg: LIGHT_BG, ready: "false" })
    await ready(page)
    await looks(page, "light")
    await openSettings(page)
    assert.equal(await themeControl(page).inputValue(), "light")

    await settingsPage(page).getByRole("button", { name: "Reset All" }).click()
    assert.equal(await themeControl(page).inputValue(), "dark")
    await looks(page, "dark")
    assert.equal(await stored(page), "dark")
    await saved(page)
    await page.reload()
    assert.equal((await page.evaluate(() => window.__firstPaint)).bg, DARK_BG)
    await ready(page)
    await looks(page, "dark")
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("the diff view follows a forced theme", async () => {
  const { page, errors, foreign, close } = await open("light")
  try {
    await openSettings(page)
    await themeControl(page).selectOption("dark")
    await looks(page, "dark")
    await page.locator('[role=treeitem][data-path="data/people.ttl"]').click()
    await page.getByRole("tab", { name: "people.ttl" }).waitFor()
    await page.evaluate(() => window.monaco.editor.getEditors()[0].getModel().setValue("@prefix ex: <https://e.org/> .\nex:a    ex:b ex:c ;   ex:d ex:e .\n"))
    await page.locator("body[data-busy='false']").waitFor()
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.focus()
      ed.trigger("test", "rdf.previewFormat", {})
    })
    const view = page.getByRole("region", { name: "Diff" })
    await view.waitFor()
    const inDiff = () => editorThemes(page, ".dif-editor")
    assert.ok((await inDiff()).length >= 2)
    assert.ok((await inDiff()).every((t) => t === "vs-dark"), JSON.stringify(await inDiff()))
    await page.keyboard.press("Escape")
    await view.waitFor({ state: "hidden" })
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})
