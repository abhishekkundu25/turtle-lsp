// The editor's Vim mode (web/app/vim.js) in the installed Google Chrome,
// driven by Playwright: turned on in Settings, Vim keys edit the text, the
// mode shows in the status bar, :w and :e save and open files, both editors
// of a split use Vim, the page's shortcuts and completion keep working, it
// turns off cleanly, survives a reload, and is not fetched while off.
// Run `npm run web:build` first.
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

// Wait until the workspace is written to the browser's storage (the page says so in body[data-saved]).
const saved = (page) => page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })

// A fresh profile (or `context`) on the editor, ready. `vimLoads` counts
// requests for monaco-vim; `foreign` lists requests to anywhere else.
async function open({ context } = {}) {
  const ctx = context ?? (await browser.newContext({ viewport: { width: 1440, height: 900 } }))
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  const foreign = []
  const seen = { vimLoads: 0 }
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  page.on("request", (r) => {
    const u = r.url()
    if (u.includes("monaco-vim")) seen.vimLoads++
    if (!u.startsWith(origin) && !u.startsWith("data:") && !u.startsWith("blob:")) foreign.push(u)
  })
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors, foreign, seen, close: () => ctx.close() }
}

const settingsPage = (page) => page.getByRole("region", { name: "Settings" })
const vimControl = (page) => settingsPage(page).getByLabel("Editor: Vim Mode", { exact: true })
// The status-bar line of the focused editor.
const mode = (page) => page.locator("#status-vim .vim-status:visible")
const modeIs = (page, m) => mode(page).locator("span").first().filter({ hasText: new RegExp(`^--${m}--$`) }).waitFor()

async function openSettings(page) {
  await page.locator(".group .monaco-editor textarea").first().focus()
  await page.keyboard.press("ControlOrMeta+Comma")
  await settingsPage(page).waitFor()
}
async function setVim(page, on) {
  await openSettings(page)
  await vimControl(page).setChecked(on)
  await page.getByRole("button", { name: "Close Settings" }).click()
  await settingsPage(page).waitFor({ state: "hidden" })
}
// Put `text` in editor `i`, the cursor at line:column, and focus it.
const load = (page, text, line = 1, column = 1, i = 0) =>
  page.evaluate(
    ([text, line, column, i]) => {
      const ed = window.monaco.editor.getEditors()[i]
      ed.getModel().setValue(text)
      ed.setPosition({ lineNumber: line, column })
      ed.focus()
    },
    [text, line, column, i]
  )
const value = (page, i = 0) => page.evaluate((i) => window.monaco.editor.getEditors()[i].getModel().getValue(), i)
const valueIs = (page, want, i = 0) => page.waitForFunction(([want, i]) => window.monaco.editor.getEditors()[i].getModel().getValue() === want, [want, i], { timeout: 5000 })
const item = (page, p) => page.locator(`[role=treeitem][data-path="${p}"]`)
const openFile = async (page, p) => {
  await item(page, p).click()
  await page.getByRole("tab", { name: p.split("/").pop() }).waitFor()
}

test("Vim Mode, turned on in Settings: ciw, dd, u, a count and . edit as Vim; the mode shows", async () => {
  const { page, errors, foreign, seen, close } = await open()
  try {
    await openSettings(page)
    assert.equal(await vimControl(page).isChecked(), false, "off by default")
    assert.equal(await page.locator("#status-vim").isVisible(), false)
    assert.equal(seen.vimLoads, 0, "monaco-vim is not fetched while Vim Mode is off")
    await page.getByRole("button", { name: "Close Settings" }).click()
    await setVim(page, true)
    await modeIs(page, "NORMAL")
    assert.ok(seen.vimLoads > 0)

    await load(page, "alpha beta gamma\nl2\nl3\nl4\nl5\nl6\n", 1, 8)
    await page.keyboard.type("ciw")
    await modeIs(page, "INSERT")
    await page.keyboard.type("BETA")
    await page.keyboard.press("Escape")
    await modeIs(page, "NORMAL")
    await valueIs(page, "alpha BETA gamma\nl2\nl3\nl4\nl5\nl6\n")
    await page.keyboard.type("dd")
    await valueIs(page, "l2\nl3\nl4\nl5\nl6\n")
    await page.keyboard.type("u")
    await valueIs(page, "alpha BETA gamma\nl2\nl3\nl4\nl5\nl6\n")
    await page.keyboard.type("j2dd")
    await valueIs(page, "alpha BETA gamma\nl4\nl5\nl6\n")
    await page.keyboard.type(".")
    await valueIs(page, "alpha BETA gamma\nl6\n")
    // The : command line shows what is typed; Escape leaves it.
    await page.keyboard.type(":nohl")
    assert.equal(await mode(page).locator("input").inputValue(), "nohl")
    await page.keyboard.press("Escape")
    await mode(page).locator("input").waitFor({ state: "detached" })
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("both editors of a split use Vim; Ctrl+\\, Ctrl+P, F1, Ctrl+Shift+F and completion keep working", async () => {
  const { page, errors, foreign, close } = await open()
  try {
    await setVim(page, true)
    await modeIs(page, "NORMAL")
    await openFile(page, "data/people.ttl")
    await page.evaluate(() => window.monaco.editor.getEditors()[0].focus())
    await page.keyboard.press("Control+Backslash")
    await page.locator(".group").nth(1).waitFor()
    await page.waitForFunction(() => document.querySelectorAll("#status-vim .vim-status").length === 2)
    assert.equal(await mode(page).count(), 1, "only the focused editor's line shows")
    // Vim keys in the second editor, then in the first: one model, so both change.
    await load(page, "one\ntwo\nthree\n", 1, 1, 1)
    await page.keyboard.type("dd")
    await valueIs(page, "two\nthree\n", 1)
    await page.keyboard.type("i")
    await modeIs(page, "INSERT")
    await page.evaluate(() => window.monaco.editor.getEditors()[0].focus())
    await modeIs(page, "NORMAL")
    await page.keyboard.type("dd")
    await valueIs(page, "three\n", 0)
    // Vim binds Ctrl+P (up a line); Go to File still gets it. F1 and
    // Ctrl+Shift+F too.
    const focus0 = () => page.evaluate(() => window.monaco.editor.getEditors()[0].focus())
    await focus0()
    await page.keyboard.press("ControlOrMeta+p")
    await page.locator("#quick-input").waitFor()
    await page.keyboard.press("Escape")
    await page.locator("#quick-input").waitFor({ state: "hidden" })
    await focus0()
    await page.keyboard.press("F1")
    await page.locator(".quick-input-widget").waitFor()
    await page.keyboard.press("Escape")
    await page.locator(".quick-input-widget").waitFor({ state: "hidden" })
    await focus0()
    await page.keyboard.press("ControlOrMeta+Shift+f")
    await page.locator("#view-search").waitFor()
    // Completion opens in insert mode.
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.getModel().setValue("@prefix ex: <https://toolbox.example.org/def/> .\n\nex:a ex:wo")
      ed.setPosition({ lineNumber: 3, column: 11 })
      ed.focus()
    })
    await page.keyboard.type("a")
    await modeIs(page, "INSERT")
    await page.keyboard.press("Control+Space")
    await page.locator(".suggest-widget.visible").waitFor()
    await page.keyboard.press("Escape")
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("Vim Mode survives a reload; turning it off restores plain typing and hides the status", async () => {
  const { page, errors, foreign, close } = await open()
  try {
    await setVim(page, true)
    await modeIs(page, "NORMAL")
    await saved(page)
    await page.reload()
    await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
    await modeIs(page, "NORMAL")
    await load(page, "one\ntwo\n")
    await page.keyboard.type("dd")
    await valueIs(page, "two\n")
    await setVim(page, false)
    await page.locator("#status-vim").waitFor({ state: "hidden" })
    assert.equal(await page.locator("#status-vim .vim-status").count(), 0)
    await load(page, "one\n")
    await page.keyboard.type("dd")
    await valueIs(page, "ddone\n")
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

// A context whose folder picker answers with a folder of the browser's
// private file system (OPFS), seeded with `files` (as in editor.test.mjs).
async function inPlaceContext(files) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  await context.addInitScript(() => {
    window.showDirectoryPicker = async () => (await navigator.storage.getDirectory()).getDirectoryHandle("proj", { create: true })
  })
  const seed = await context.newPage()
  await seed.goto(origin + "/index.html")
  await seed.evaluate(async (files) => {
    const root = await (await navigator.storage.getDirectory()).getDirectoryHandle("proj", { create: true })
    for (const [p, text] of Object.entries(files)) {
      const parts = p.split("/")
      let dir = root
      for (const d of parts.slice(0, -1)) dir = await dir.getDirectoryHandle(d, { create: true })
      const w = await (await dir.getFileHandle(parts.at(-1), { create: true })).createWritable()
      await w.write(text)
      await w.close()
    }
  }, files)
  await seed.close()
  return context
}
const disk = (page, p) =>
  page.evaluate(async (p) => {
    let dir = await (await navigator.storage.getDirectory()).getDirectoryHandle("proj")
    const parts = p.split("/")
    for (const d of parts.slice(0, -1)) dir = await dir.getDirectoryHandle(d)
    return (await (await dir.getFileHandle(parts.at(-1))).getFile()).text()
  }, p)
const A = "@prefix ex: <https://e.org/> .\n\nex:a ex:b ex:c .\n"
const B = "@prefix ex: <https://e.org/> .\n\nex:d ex:e ex:f .\n"

test(":w saves to disk, :e opens a workspace file, :q closes the tab", async () => {
  const context = await inPlaceContext({ "a.ttl": A, "sub/b.ttl": B })
  try {
    const { page, errors, foreign } = await open({ context })
    await page.getByRole("button", { name: "Open Folder in Place" }).click()
    await item(page, "proj/sub/b.ttl").waitFor()
    await openFile(page, "proj/a.ttl")
    await setVim(page, true)
    await modeIs(page, "NORMAL")
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.setPosition({ lineNumber: 3, column: 1 })
      ed.focus()
    })
    await page.keyboard.type("O# edited")
    await page.keyboard.press("Escape")
    await page.waitForFunction(() => document.querySelector('.tab[data-path="proj/a.ttl"]')?.classList.contains("dirty"))
    await page.keyboard.type(":w")
    await page.keyboard.press("Enter")
    await page.waitForFunction(() => !document.querySelector('.tab[data-path="proj/a.ttl"]').classList.contains("dirty"))
    assert.equal(await disk(page, "a.ttl"), "@prefix ex: <https://e.org/> .\n\n# edited\nex:a ex:b ex:c .\n")
    await modeIs(page, "NORMAL")

    await page.keyboard.type(":e proj/sub/b.ttl")
    await page.keyboard.press("Enter")
    await page.locator('.tab[aria-selected="true"][data-path="proj/sub/b.ttl"]').waitFor()
    assert.equal(await value(page), B)
    // Vim keys go on in the opened file, and an unknown path says so.
    await page.keyboard.type(":e nowhere.ttl")
    await page.keyboard.press("Enter")
    await mode(page).locator(".vim-notification", { hasText: "nowhere.ttl" }).waitFor()
    await page.keyboard.type(":q")
    await page.keyboard.press("Enter")
    await page.locator('.tab[data-path="proj/sub/b.ttl"]').waitFor({ state: "detached" })
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await context.close()
  }
})
