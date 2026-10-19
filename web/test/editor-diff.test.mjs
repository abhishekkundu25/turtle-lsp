// The editor's diff view (web/app/diff.js) in the installed Google Chrome,
// driven by Playwright: Preview Formatting shows the file against its
// formatted text and Apply takes it (one undo step), Compare with Saved
// shows a file of a folder opened in place against its disk text, Escape
// closes and leaves no models behind. Run `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium } from "playwright-core"

const app = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "app")
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".ttf": "font/ttf" }
const SHOT = process.env.DIFF_SHOT

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

// A context whose folder picker gives an OPFS folder "proj" holding `files`.
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
      const w = await (await root.getFileHandle(p, { create: true })).createWritable()
      await w.write(text)
      await w.close()
    }
  }, files)
  await seed.close()
  return context
}

const item = (page, p) => page.locator(`[role=treeitem][data-path="${p}"]`)
const openFile = async (page, p) => {
  await item(page, p).click()
  await page.getByRole("tab", { name: p.split("/").pop() }).waitFor()
}
const text = (page, p) => page.evaluate((p) => window.monaco.editor.getModel(window.monaco.Uri.parse(`file:///work/${p}`))?.getValue(), p)
const modelCount = (page) => page.evaluate(() => window.monaco.editor.getModels().length)
// Run an editor command (an action id) in the first editor.
const run = (page, id) =>
  page.evaluate((id) => {
    const ed = window.monaco.editor.getEditors()[0]
    ed.focus()
    ed.trigger("test", id, {})
  }, id)
// The temporary models go once the diff editor has gone (it may finish computing first).
const noExtraModels = (page, n) => page.waitForFunction((n) => window.monaco.editor.getModels().length === n && !document.querySelector(".dif-editor"), n, { timeout: 5000 })
const view = (page) => page.getByRole("region", { name: "Diff" })
// The text of the diff editor's left and right models.
const sides = (page) =>
  page.evaluate(() => {
    const d = window.monaco.editor.getDiffEditors()[0]
    const m = d?.getModel()
    return m && { left: m.original.getValue(), right: m.modified.getValue() }
  })
const toastSays = (page, re) => page.locator("#toast", { hasText: re }).waitFor()

const MESSY = "@prefix ex: <https://e.org/> .\nex:a    ex:b ex:c ;   ex:d ex:e .\n"
const A = "@prefix ex: <https://e.org/> .\n\nex:a ex:b ex:c .\n"

test("Preview Formatting shows the formatted text; Apply takes it as one undo step; Escape closes", async () => {
  const { page, errors, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    const models = await modelCount(page)
    await page.evaluate((t) => window.monaco.editor.getEditors()[0].getModel().setValue(t), MESSY)
    await page.locator("body[data-busy='false']").waitFor()
    await run(page, "rdf.previewFormat")
    await view(page).waitFor()
    assert.equal(await view(page).locator(".dif-title").textContent(), "people.ttl ↔ people.ttl (formatted)")
    const s = await sides(page)
    assert.equal(s.left, MESSY)
    assert.notEqual(s.right, MESSY)
    assert.match(s.right, /ex:a ex:b ex:c ;\n\s+ex:d ex:e \.\n/)
    assert.equal(await modelCount(page), models + 1)
    if (SHOT) {
      await page.waitForTimeout(500)
      await page.screenshot({ path: SHOT })
    }
    // Escape closes without changing the file, and the temporary model goes.
    await page.keyboard.press("Escape")
    await view(page).waitFor({ state: "hidden" })
    assert.equal(await text(page, "data/people.ttl"), MESSY)
    await noExtraModels(page, models)

    await run(page, "rdf.previewFormat")
    await view(page).waitFor()
    await view(page).getByRole("button", { name: "Apply" }).click()
    await view(page).waitFor({ state: "hidden" })
    assert.equal(await text(page, "data/people.ttl"), s.right)
    await noExtraModels(page, models)
    await run(page, "undo")
    assert.equal(await text(page, "data/people.ttl"), MESSY)

    // Nothing to format: a toast, no view.
    await run(page, "redo")
    await run(page, "rdf.previewFormat")
    await toastSays(page, /Already formatted, or the file has syntax errors/)
    assert.equal(await view(page).isVisible(), false)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Compare with Saved shows the disk text against the edited text; a browser-only file toasts", async () => {
  const context = await inPlaceContext({ "a.ttl": A })
  try {
    const { page, errors } = await open({ context })
    await page.getByRole("button", { name: "Open Folder in Place" }).click()
    await openFile(page, "proj/a.ttl")
    const models = await modelCount(page)
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.setPosition({ lineNumber: 3, column: 1 })
      ed.focus()
      ed.trigger("keyboard", "type", { text: "# edited\n" })
    })
    const edited = "@prefix ex: <https://e.org/> .\n\n# edited\nex:a ex:b ex:c .\n"
    assert.equal(await text(page, "proj/a.ttl"), edited)
    await run(page, "file.compareSaved")
    await view(page).waitFor()
    assert.equal(await view(page).locator(".dif-title").textContent(), "a.ttl (on disk) ↔ a.ttl")
    assert.deepEqual(await sides(page), { left: A, right: edited })
    assert.equal(await view(page).getByRole("button", { name: "Apply" }).isVisible(), false)
    // The right side is the file itself: typing there edits the file.
    await page.evaluate(() => window.monaco.editor.getDiffEditors()[0].getModifiedEditor().trigger("keyboard", "type", { text: "x" }))
    assert.equal(await text(page, "proj/a.ttl"), (await sides(page)).right)
    await page.getByRole("button", { name: "Close Diff" }).click()
    await view(page).waitFor({ state: "hidden" })
    await noExtraModels(page, models)

    await openFile(page, "data/people.ttl")
    await run(page, "file.compareSaved")
    await toastSays(page, /Only files of a folder opened in place have a version on disk/)
    assert.equal(await view(page).isVisible(), false)
    assert.deepEqual(errors, [])
  } finally {
    await context.close()
  }
})
