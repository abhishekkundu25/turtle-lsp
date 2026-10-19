// The editor page (editor mode) in the installed Google Chrome, driven by
// Playwright: Monaco with turtle-lsp running in a worker, the explorer, the
// Problems panel, import and export. Run `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium, webkit, firefox } from "playwright-core"

// BROWSER=webkit or BROWSER=firefox runs these in Playwright's builds of
// those browsers; the default is the installed Google Chrome.
const BROWSER = process.env.BROWSER ?? "chrome"
const launch = () => (BROWSER === "chrome" ? chromium.launch({ channel: "chrome", headless: true }) : { webkit, firefox }[BROWSER].launch({ headless: true }))

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
  browser = await launch()
})
after(async () => {
  await browser?.close()
  await new Promise((r) => server.close(r))
})

// A fresh browser profile on the editor, ready: Monaco loaded, the server
// started and the first diagnostics shown.
async function open({ context, intro = false } = {}) {
  const ctx = context ?? (await browser.newContext({ viewport: { width: 1440, height: 900 }, acceptDownloads: true }))
  const page = await ctx.newPage()
  // The first-visit note is tested on its own (`intro: true`).
  if (!intro) await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  const foreign = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  page.on("request", (r) => {
    const u = r.url()
    if (!u.startsWith(origin) && !u.startsWith("data:") && !u.startsWith("blob:")) foreign.push(u)
  })
  await page.goto(origin + "/editor.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, ctx, errors, foreign, close: () => ctx.close() }
}

// Wait until the language server has nothing pending.
const settled = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 10_000 })
const item = (page, p) => page.locator(`[role=treeitem][data-path="${p}"]`)
const openFile = async (page, p) => {
  await item(page, p).click()
  await page.getByRole("tab", { name: p.split("/").pop() }).waitFor()
}
const text = (page, p) => page.evaluate((p) => window.monaco.editor.getModel(window.monaco.Uri.parse(`file:///work/${p}`))?.getValue(), p)
// Put the cursor on `needle` (+ offset) in the active editor and run a command.
async function at(page, needle, offset, command) {
  await page.evaluate(
    ([needle, offset, command]) => {
      const ed = window.monaco.editor.getEditors()[0]
      const m = ed.getModel()
      const pos = m.getPositionAt(m.getValue().indexOf(needle) + offset)
      ed.setPosition(pos)
      ed.focus()
      if (command) ed.trigger("test", command, {})
    },
    [needle, offset, command]
  )
}

test("the editor opens on the sample workspace, with its folders, analysed, loading nothing from elsewhere", async () => {
  const { page, errors, foreign, close } = await open()
  try {
    for (const p of ["ontology", "shapes", "data", "queries", "ontology/core.ttl", "shapes/person.ttl", "data/people.ttl", "queries/people.rq"]) {
      await item(page, p).waitFor()
    }
    assert.equal(await item(page, "data").getAttribute("aria-expanded"), "true")
    await page.getByRole("tab", { name: "start-here.ttl" }).waitFor()
    assert.ok(Number(await page.locator("#status-errors").textContent()) > 0)
    await page.locator("#status-version", { hasText: /turtle-lsp \d+\.\d+\.\d+/ }).waitFor()
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("problems are listed by file and clicking one opens its file at its place", async () => {
  const { page, close } = await open()
  try {
    await page.getByRole("tab", { name: /^Problems/ }).click()
    const row = page.locator(".problem", { hasText: "'labl' is not defined" })
    await row.click()
    await page.getByRole("tab", { name: "core.ttl" }).waitFor()
    const line = await page.evaluate(() => window.monaco.editor.getEditors()[0].getPosition().lineNumber)
    const want = (await text(page, "ontology/core.ttl")).split("\n").findIndex((l) => l.includes("rdfs:labl")) + 1
    assert.equal(line, want)
    assert.ok((await page.locator(".problem", { hasText: "shacl-violation" }).count()) >= 2)
  } finally {
    await close()
  }
})

test("hover and completion come from the language server", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    await at(page, "ex:worksFor", 4, "editor.action.showHover")
    await page.locator(".monaco-hover", { hasText: "works for" }).waitFor()
    await at(page, "ex:name", 3, "editor.action.triggerSuggest")
    await page.locator(".suggest-widget", { hasText: "ex:worksFor" }).waitFor()
  } finally {
    await close()
  }
})

test("the file named in a hover opens at its line when clicked", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    await at(page, "ex:worksFor", 4, "editor.action.showHover")
    await page.locator(".monaco-hover a", { hasText: /^core\.ttl:\d+$/ }).click()
    await page.getByRole("tab", { name: "core.ttl" }).waitFor()
    const line = await page.evaluate(() => window.monaco.editor.getEditors()[0].getPosition().lineNumber)
    const want = (await text(page, "ontology/core.ttl")).split("\n").findIndex((l) => l.startsWith("ex:worksFor")) + 1
    assert.equal(line, want)
  } finally {
    await close()
  }
})

test("a quick fix applies its edit", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "ontology/core.ttl")
    await at(page, "rdfs:labl", 2, "editor.action.quickFix")
    await page.locator(".action-widget", { hasText: "Change to 'rdfs:label'" }).waitFor()
    await page.keyboard.press("Enter")
    await page.waitForFunction(() => !window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/ontology/core.ttl")).getValue().includes("rdfs:labl"))
    await settled(page)
  } finally {
    await close()
  }
})

test("rename changes a term in every file, and format lays a file out", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "ontology/core.ttl")
    await at(page, "ex:worksFor a", 4, "editor.action.rename")
    const input = page.locator(".rename-input")
    await input.waitFor()
    await input.fill("ex:employedBy")
    await input.press("Enter")
    await page.waitForFunction(() => window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/data/people.ttl")).getValue().includes("ex:employedBy"))
    await openFile(page, "data/people.ttl")
    await at(page, "id:", 0, "editor.action.formatDocument")
    // The indent is the file's own, as Monaco (like VS Code) detects it.
    await page.waitForFunction(() => /^id:acme a ex:Organisation ;\n +rdfs:label "ACME" \.$/m.test(window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/data/people.ttl")).getValue()))
  } finally {
    await close()
  }
})

test("the explorer creates, renames and deletes files", async () => {
  const { page, close } = await open()
  try {
    await item(page, "data").click()
    await page.getByRole("button", { name: "New File" }).click()
    const name = page.locator(".tree-input")
    await name.fill("extra.ttl")
    await name.press("Enter")
    await item(page, "data/extra.ttl").waitFor()
    await page.getByRole("tab", { name: "extra.ttl" }).waitFor()
    await page.keyboard.type("@prefix ex: <https://e.org/> .\nex:a ex:b ex:c .\n")
    await item(page, "data/extra.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Rename" }).click()
    await name.fill("renamed.ttl")
    await name.press("Enter")
    await item(page, "data/renamed.ttl").waitFor()
    assert.equal(await item(page, "data/extra.ttl").count(), 0)
    assert.match(await text(page, "data/renamed.ttl"), /ex:a ex:b ex:c/)
    await item(page, "data/renamed.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Delete" }).click()
    await page.waitForFunction(() => !document.querySelector('[data-path="data/renamed.ttl"]'))
    assert.equal(await text(page, "data/renamed.ttl"), undefined)
  } finally {
    await close()
  }
})

test("an imported folder keeps its structure, and its files are analysed", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-web-import-"))
  const { page, errors, close } = await open()
  try {
    fs.mkdirSync(path.join(dir, "model", "shapes"), { recursive: true })
    fs.writeFileSync(path.join(dir, "model", "a.ttl"), "@prefix ex: <https://e.org/> .\nex:a ex:b ex:c .\n")
    fs.writeFileSync(path.join(dir, "model", "shapes", "b.ttl"), "@prefix ex: <https://e.org/> .\nex:s ex:p ex:o ex:q .\n")
    fs.writeFileSync(path.join(dir, "model", "readme.md"), "# not RDF\n")
    await page.locator("#folder-input").setInputFiles(path.join(dir, "model"))
    await item(page, "model/shapes/b.ttl").waitFor()
    await item(page, "model/a.ttl").waitFor()
    assert.equal(await item(page, "model/readme.md").count(), 0)
    await settled(page)
    await page.getByRole("tab", { name: /^Problems/ }).click()
    await page.locator(".problem-file", { hasText: "model/shapes/b.ttl" }).waitFor()
    assert.deepEqual(errors, [])
  } finally {
    await close()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

// The entry names of a zip (its central directory).
function zipNames(buf) {
  const names = []
  let at = buf.lastIndexOf(Buffer.from([0x50, 0x4b, 0x05, 0x06]))
  let p = buf.readUInt32LE(at + 16)
  const n = buf.readUInt16LE(at + 10)
  for (let i = 0; i < n; i++) {
    const len = buf.readUInt16LE(p + 28), extra = buf.readUInt16LE(p + 30), comment = buf.readUInt16LE(p + 32)
    names.push(buf.subarray(p + 46, p + 46 + len).toString("utf8"))
    p += 46 + len + extra + comment
  }
  return names
}

test("a file downloads, and the workspace exports as a zip with its folders", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    const [file] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Download File" }).click()])
    assert.equal(file.suggestedFilename(), "people.ttl")
    assert.equal(fs.readFileSync(await file.path(), "utf8"), await text(page, "data/people.ttl"))
    const [zip] = await Promise.all([page.waitForEvent("download"), page.getByRole("button", { name: "Export Workspace as Zip" }).click()])
    assert.match(zip.suggestedFilename(), /\.zip$/)
    const names = zipNames(fs.readFileSync(await zip.path()))
    for (const p of ["ontology/core.ttl", "shapes/person.ttl", "data/people.ttl", "queries/people.rq"]) assert.ok(names.includes(p), names.join(", "))
  } finally {
    await close()
  }
})

test("the workspace is kept in the browser across a reload", async () => {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  try {
    const first = await open({ context })
    await openFile(first.page, "data/people.ttl")
    await at(first.page, "id:bob", 0)
    await first.page.keyboard.type("# kept\n")
    await settled(first.page)
    await first.page.waitForTimeout(600)
    await first.page.close()
    const second = await open({ context })
    assert.match(await text(second.page, "data/people.ttl"), /# kept\nid:bob/)
  } finally {
    await context.close()
  }
})

test("the labels toggle shows label hints in the editor", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    const hints = () => page.evaluate(() => document.querySelectorAll(".monaco-editor [class*='dyn-rule'], .monaco-editor .inlay-hint, .monaco-editor [class*='inlay']").length)
    const off = await hints()
    await page.getByRole("button", { name: /Labels/ }).click()
    await page.waitForFunction((off) => document.querySelectorAll(".monaco-editor [class*='dyn-rule'], .monaco-editor .inlay-hint, .monaco-editor [class*='inlay']").length > off, off)
  } finally {
    await close()
  }
})

// --- batch 1: parity -----------------------------------------------------------

test("Go to File finds symbols across the workspace with '#' and in the file with '@'", async () => {
  const { page, close } = await open()
  try {
    await page.keyboard.press("ControlOrMeta+p")
    await page.locator("#quick-input").fill("#worksFor")
    await page.locator(".quick-item", { hasText: "ex:worksFor" }).first().waitFor()
    await page.keyboard.press("Enter")
    await page.getByRole("tab", { name: "core.ttl" }).waitFor()
    const line = await page.evaluate(() => window.monaco.editor.getEditors()[0].getPosition().lineNumber)
    assert.equal(line, (await text(page, "ontology/core.ttl")).split("\n").findIndex((l) => l.startsWith("ex:worksFor")) + 1)
    await page.keyboard.press("ControlOrMeta+p")
    await page.locator("#quick-input").fill("@Organisation")
    await page.locator(".quick-item", { hasText: "ex:Organisation" }).first().waitFor()
    await page.keyboard.press("Enter")
    // Picking runs after the symbols arrive: wait for the cursor to land.
    const want = (await text(page, "ontology/core.ttl")).split("\n").findIndex((l) => l.startsWith("ex:Organisation")) + 1
    await page.waitForFunction((want) => window.monaco.editor.getEditors()[0].getPosition().lineNumber === want, want, { timeout: 5000 })
  } finally {
    await close()
  }
})

test("edits travel as ranges and the server keeps the same text", async () => {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  // Record what the page sends the language server worker.
  await context.addInitScript(() => {
    window.__sent = []
    const post = Worker.prototype.postMessage
    Worker.prototype.postMessage = function (m, ...rest) {
      if (m?.method === "textDocument/didChange") window.__sent.push(m.params)
      return post.call(this, m, ...rest)
    }
  })
  const { page, close } = await open({ context })
  try {
    await openFile(page, "data/people.ttl")
    await at(page, "id:bob", 0)
    await page.keyboard.type("id:x ex:name . ")
    await settled(page)
    const sent = await page.evaluate(() => window.__sent)
    assert.ok(sent.length > 0 && sent.every((p) => p.contentChanges.every((c) => c.range)), JSON.stringify(sent[0]))
    // The server's text is the editor's: the new error sits where it was typed.
    const uri = "file:///work/data/people.ttl"
    const line = (await text(page, "data/people.ttl")).split("\n").findIndex((l) => l.startsWith("id:x")) + 1
    const errors = await page.evaluate((uri) => window.monaco.editor.getModelMarkers({ resource: window.monaco.Uri.parse(uri) }).filter((m) => m.severity === 8).map((m) => m.startLineNumber), uri)
    assert.ok(errors.includes(line), `errors on ${errors}, typed on ${line}`)
  } finally {
    await close()
  }
})

test("dragging a file onto a folder moves it, with its tab and text", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/hr.nq")
    const before = await text(page, "data/hr.nq")
    await item(page, "data/hr.nq").dragTo(item(page, "queries"))
    await item(page, "queries/hr.nq").waitFor()
    assert.equal(await item(page, "data/hr.nq").count(), 0)
    assert.equal(await text(page, "queries/hr.nq"), before)
    await page.getByRole("tab", { name: "hr.nq" }).waitFor()
    // A folder moves with everything in it.
    await item(page, "shapes").dragTo(item(page, "ontology"))
    await item(page, "ontology/shapes/person.ttl").waitFor()
    await settled(page)
    assert.ok(Number(await page.locator("#status-shacl").textContent()) >= 2)
  } finally {
    await close()
  }
})

test("Copy Path and Copy Relative Path", async () => {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  // What the page copies, recorded the same way in every browser.
  await context.addInitScript(() => {
    window.__copied = []
    Object.defineProperty(navigator, "clipboard", { value: { writeText: async (t) => void window.__copied.push(t) } })
  })
  try {
    const { page } = await open({ context })
    await item(page, "data/people.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Copy Relative Path" }).click()
    await item(page, "data/people.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Copy Path", exact: true }).click()
    await page.waitForFunction(() => window.__copied.length === 2)
    assert.deepEqual(await page.evaluate(() => window.__copied), ["data/people.ttl", "file:///work/data/people.ttl"])
  } finally {
    await context.close()
  }
})

test("a file's problems fold in the Problems panel", async () => {
  const { page, close } = await open()
  try {
    const header = page.locator(".problem-file", { hasText: "data/people.ttl" })
    const rows = page.locator('.problem[data-file="data/people.ttl"]')
    assert.ok((await rows.count()) > 0)
    await header.click()
    assert.equal(await rows.count(), 0)
    assert.equal(await header.getAttribute("aria-expanded"), "false")
    await header.click()
    assert.ok((await rows.count()) > 0)
  } finally {
    await close()
  }
})

// --- batch 2: saving files opened in place, split editor ---------------------------

// Opening a folder in place needs showDirectoryPicker, which only Chromium
// browsers have; the page hides the button elsewhere.
const inPlace = { skip: BROWSER !== "chrome" && "no showDirectoryPicker in this browser" }

// A context whose folder picker answers with a folder of the browser's
// private file system (OPFS), which behaves like a disk folder, seeded with
// `files` ({ "a.ttl": text, "sub/b.ttl": text }).
async function inPlaceContext(files) {
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  await context.addInitScript(() => {
    window.showDirectoryPicker = async () => (await navigator.storage.getDirectory()).getDirectoryHandle("proj", { create: true })
  })
  const seed = await context.newPage()
  await seed.goto(origin + "/editor.html")
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
// Run an editor command (an action id) in the first editor.
const run = (page, id) =>
  page.evaluate((id) => {
    const ed = window.monaco.editor.getEditors()[0]
    ed.focus()
    ed.trigger("test", id, {})
  }, id)
const A = "@prefix ex: <https://e.org/> .\n\nex:a ex:b ex:c .\n"
const B = "@prefix ex: <https://e.org/> .\n\nex:d ex:e ex:f .\n"

test("a folder opened in place: unsaved marks, Ctrl+S saves to disk, Revert reads it back", inPlace, async () => {
  const context = await inPlaceContext({ "a.ttl": A, "sub/b.ttl": B })
  try {
    const { page } = await open({ context })
    await page.getByRole("button", { name: "Open Folder in Place" }).click()
    await item(page, "proj/sub/b.ttl").waitFor()
    await openFile(page, "proj/a.ttl")
    const tab = page.locator('.tab[data-path="proj/a.ttl"]')
    assert.equal(await tab.getAttribute("class"), "tab")
    await at(page, "ex:a", 0)
    await page.keyboard.type("# edited\n")
    await page.waitForFunction(() => document.querySelector('.tab[data-path="proj/a.ttl"]')?.classList.contains("dirty"))
    await page.keyboard.press("ControlOrMeta+s")
    await page.waitForFunction(() => !document.querySelector('.tab[data-path="proj/a.ttl"]').classList.contains("dirty"))
    assert.equal(await disk(page, "a.ttl"), "@prefix ex: <https://e.org/> .\n\n# edited\nex:a ex:b ex:c .\n")
    await page.keyboard.type("more ")
    await page.waitForFunction(() => document.querySelector('.tab[data-path="proj/a.ttl"]')?.classList.contains("dirty"))
    await run(page, "file.revert")
    await page.waitForFunction(() => !window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/proj/a.ttl")).getValue().includes("more"))
    assert.equal(await text(page, "proj/a.ttl"), await disk(page, "a.ttl"))
    await page.waitForFunction(() => !document.querySelector('.tab[data-path="proj/a.ttl"]').classList.contains("dirty"))
  } finally {
    await context.close()
  }
})

test("Save All writes every unsaved file, and a new file in the folder is created on disk", inPlace, async () => {
  const context = await inPlaceContext({ "a.ttl": A, "sub/b.ttl": B })
  try {
    const { page } = await open({ context })
    await page.getByRole("button", { name: "Open Folder in Place" }).click()
    await item(page, "proj/sub/b.ttl").waitFor()
    for (const [p, anchor] of [["proj/a.ttl", "ex:a"], ["proj/sub/b.ttl", "ex:d"]]) {
      await openFile(page, p)
      await at(page, anchor, 0)
      await page.keyboard.type("# x\n")
    }
    await item(page, "proj/sub").click()
    await page.getByRole("button", { name: "New File" }).click()
    await page.locator(".tree-input").fill("c.ttl")
    await page.locator(".tree-input").press("Enter")
    await page.getByRole("tab", { name: "c.ttl" }).waitFor()
    await page.keyboard.type("@prefix ex: <https://e.org/> .\n")
    await run(page, "file.saveAll")
    await page.waitForFunction(() => !document.querySelector(".tab.dirty"))
    assert.match(await disk(page, "a.ttl"), /^# x\n/m)
    assert.match(await disk(page, "sub/b.ttl"), /^# x\n/m)
    assert.equal(await disk(page, "sub/c.ttl"), "@prefix ex: <https://e.org/> .\n")
  } finally {
    await context.close()
  }
})

test("closing an unsaved tab asks: Cancel keeps it, Don't Save drops the edit, Save writes it", inPlace, async () => {
  const context = await inPlaceContext({ "a.ttl": A })
  try {
    const { page } = await open({ context })
    await page.getByRole("button", { name: "Open Folder in Place" }).click()
    await openFile(page, "proj/a.ttl")
    const edit = async () => {
      await at(page, "ex:a", 0)
      await page.keyboard.type("# e\n")
      await page.waitForFunction(() => document.querySelector('.tab[data-path="proj/a.ttl"]')?.classList.contains("dirty"))
    }
    const close = () => page.locator('.tab[data-path="proj/a.ttl"] [data-close]').click()
    const dialog = page.getByRole("dialog")
    await edit()
    await close()
    await dialog.getByText(/Do you want to save the changes you made to a\.ttl\?/).waitFor()
    // Escape cancels, and the focus goes back to the editor.
    await page.keyboard.press("Escape")
    await dialog.waitFor({ state: "hidden" })
    assert.equal(await page.locator('.tab[data-path="proj/a.ttl"]').count(), 1)
    assert.equal(await focusIs(page), "editor 0")
    await close()
    await dialog.getByRole("button", { name: "Cancel" }).click()
    assert.equal(await page.locator('.tab[data-path="proj/a.ttl"]').count(), 1)
    await close()
    await dialog.getByRole("button", { name: "Don't Save" }).click()
    await page.waitForFunction(() => !document.querySelector('.tab[data-path="proj/a.ttl"]'))
    assert.equal(await text(page, "proj/a.ttl"), A)
    assert.equal(await disk(page, "a.ttl"), A)
    await openFile(page, "proj/a.ttl")
    await edit()
    await close()
    await dialog.getByRole("button", { name: "Save", exact: true }).click()
    await page.waitForFunction(() => !document.querySelector('.tab[data-path="proj/a.ttl"]'))
    assert.equal(await disk(page, "a.ttl"), "@prefix ex: <https://e.org/> .\n\n# e\nex:a ex:b ex:c .\n")
  } finally {
    await context.close()
  }
})

test("a folder opened in place stays linked after a reload", inPlace, async () => {
  const context = await inPlaceContext({ "a.ttl": A })
  try {
    const first = await open({ context })
    await first.page.getByRole("button", { name: "Open Folder in Place" }).click()
    await item(first.page, "proj/a.ttl").waitFor()
    await first.page.waitForTimeout(600)
    await first.page.close()
    const { page } = await open({ context })
    await openFile(page, "proj/a.ttl")
    await at(page, "ex:a", 0)
    await page.keyboard.type("# after reload\n")
    await page.keyboard.press("ControlOrMeta+s")
    await page.waitForFunction(() => !document.querySelector('.tab[data-path="proj/a.ttl"]').classList.contains("dirty"))
    assert.match(await disk(page, "a.ttl"), /# after reload/)
  } finally {
    await context.close()
  }
})

// Where the keyboard focus is: "editor <group>", "tree", or the element.
const focusIs = (page) =>
  page.evaluate(() => {
    const a = document.activeElement
    if (a?.closest(".monaco-editor")) return `editor ${[...document.querySelectorAll(".group")].indexOf(a.closest(".group"))}`
    return a?.id === "tree" ? "tree" : `${a?.tagName}#${a?.id}`
  })

test("Escape gives the focus back: the menu, a name being edited, Settings", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    // The explorer's menu and a name being edited go back to the explorer.
    await item(page, "data/people.ttl").click({ button: "right" })
    await page.locator("#menu:not([hidden])").waitFor()
    await page.keyboard.press("Escape")
    await page.locator("#menu").waitFor({ state: "hidden" })
    assert.equal(await focusIs(page), "tree")
    await page.keyboard.press("F2")
    await page.locator(".tree-input").waitFor()
    await page.keyboard.press("Escape")
    await page.locator(".tree-input").waitFor({ state: "detached" })
    assert.equal(await focusIs(page), "tree")
    // Settings goes back to the editor it was opened from, in a split too.
    await page.evaluate(() => window.monaco.editor.getEditors()[0].focus())
    await page.keyboard.press("ControlOrMeta+Backslash")
    await page.locator(".group").nth(1).waitFor()
    await page.evaluate(() => window.monaco.editor.getEditors()[1].focus())
    await page.keyboard.press("ControlOrMeta+Comma")
    await page.getByRole("searchbox", { name: "Search settings" }).waitFor()
    await page.keyboard.press("Escape")
    assert.equal(await focusIs(page), "editor 1")
  } finally {
    await close()
  }
})

test("a first visit shows the note on privacy once; the status bar shows it again", async () => {
  const { page, errors, foreign, close } = await open({ intro: true })
  try {
    const note = page.getByRole("dialog", { name: "Private by design" })
    await note.waitFor()
    await note.getByText("No server ever sees your data").waitFor()
    // The credit links open GitHub and the extension's page in a new tab.
    const links = await note.locator(".intro-foot a").evaluateAll((as) => as.map((a) => [a.textContent, a.href, a.target, a.rel]))
    const gh = "https://github.com/abhishekkundu25/turtle-lsp"
    assert.deepEqual(links, [
      ["turtle-lsp", gh, "_blank", "noopener"],
      ["GitHub", gh, "_blank", "noopener"],
      ["Get it for VS Code", "https://marketplace.visualstudio.com/items?itemName=abhishekkundu25.turtle-language-server", "_blank", "noopener"],
    ])
    assert.equal(await page.locator("#status-version").getAttribute("href"), `${gh}/releases`)
    assert.equal(await page.evaluate(() => document.activeElement.textContent), "Start editing")
    await note.getByRole("button", { name: "Start editing" }).click()
    await note.waitFor({ state: "hidden" })
    assert.equal(await focusIs(page), "editor 0")
    // Not again after a reload.
    await page.reload()
    await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
    assert.equal(await note.isVisible(), false)
    // "Kept in this browser" shows it; Escape closes it, back to the editor.
    await page.locator("#status-storage").click()
    await note.waitFor()
    await page.keyboard.press("Escape")
    await note.waitFor({ state: "hidden" })
    assert.equal(await focusIs(page), "editor 0")
    // "Open my files…" goes straight to the file picker.
    await page.locator("#status-storage").click()
    const chooser = page.waitForEvent("filechooser")
    await note.getByRole("button", { name: "Open my files…" }).click()
    await chooser
    await note.waitFor({ state: "hidden" })
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("the editor splits into two groups that share each file", async () => {
  const { page, close } = await open()
  try {
    await openFile(page, "data/people.ttl")
    await page.evaluate(() => window.monaco.editor.getEditors()[0].focus())
    await page.keyboard.press("ControlOrMeta+Backslash")
    await page.locator(".group").nth(1).waitFor()
    assert.equal(await page.locator(".group").count(), 2)
    assert.equal(await page.locator('.group .tab[data-path="data/people.ttl"]').count(), 2)
    // Typing in one group shows in the other: one model per file.
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[1]
      ed.focus()
      ed.setPosition({ lineNumber: 1, column: 1 })
    })
    await page.keyboard.type("# both\n")
    const editors = await page.evaluate(() => window.monaco.editor.getEditors().map((e) => e.getModel()?.getValue().startsWith("# both")))
    assert.deepEqual(editors, [true, true])
    // Opening a file goes to the focused group; the other keeps its file.
    await item(page, "ontology/core.ttl").click()
    await page.locator(".group").nth(1).locator('.tab[aria-selected="true"][data-path="ontology/core.ttl"]').waitFor()
    assert.equal(await page.locator('.group').nth(0).locator('.tab[aria-selected="true"]').getAttribute("data-path"), "data/people.ttl")
    // Closing a group's last tabs closes the group.
    for (const p of ["ontology/core.ttl", "data/people.ttl"]) await page.locator(".group").nth(1).locator(`.tab[data-path="${p}"] [data-close]`).click()
    await page.waitForFunction(() => document.querySelectorAll(".group").length === 1)
  } finally {
    await close()
  }
})

// --- batch 3: fetching imports, dropping folders --------------------------------------

const IMPORTING = `@prefix owl: <http://www.w3.org/2002/07/owl#> .

<https://e.org/me> a owl:Ontology ;
    owl:imports <https://onto.example.org/core> .
`
async function withImport(page, respond) {
  await page.route("https://onto.example.org/**", respond)
  await page.evaluate((text) => window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/ontology/labels.ttl")).setValue(text), IMPORTING)
  await openFile(page, "ontology/labels.ttl")
  await page.getByRole("tab", { name: /^Problems/ }).click()
  await page.locator(".problem", { hasText: "is not loaded" }).waitFor()
  await at(page, "<https://onto.example.org/core>", 3, "editor.action.quickFix")
  await page.locator(".action-widget", { hasText: "Fetch https://onto.example.org/core" }).waitFor()
  await page.keyboard.press("Enter")
}

test("the fetch quick fix loads an unresolved import into the workspace", async () => {
  const { page, close } = await open()
  try {
    await withImport(page, (route) =>
      route.fulfill({
        status: 200,
        headers: { "content-type": "text/turtle; charset=utf-8", "access-control-allow-origin": "*" },
        body: "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n@prefix c: <https://onto.example.org/core#> .\n\nc:Thing a owl:Class .\n",
      })
    )
    await item(page, "imports/onto.example.org/core.ttl").waitFor()
    assert.match(await text(page, "imports/onto.example.org/core.ttl"), /c:Thing a owl:Class/)
    await settled(page)
    await page.waitForFunction(() => ![...document.querySelectorAll(".problem")].some((p) => p.textContent.includes("is not loaded")))
  } finally {
    await close()
  }
})

test("a fetch that fails or returns another format says why and adds nothing", async () => {
  for (const [respond, why] of [
    [(route) => route.fulfill({ status: 404, headers: { "access-control-allow-origin": "*" }, body: "" }), /HTTP 404/],
    [(route) => route.fulfill({ status: 200, headers: { "content-type": "application/rdf+xml", "access-control-allow-origin": "*" }, body: "<rdf:RDF/>" }), /application\/rdf\+xml/],
    [(route) => route.abort(), /does not allow|offline/],
  ]) {
    const { page, close } = await open()
    try {
      await withImport(page, respond)
      await page.locator("#toast", { hasText: why }).waitFor()
      assert.equal(await page.locator('[role=treeitem][data-path^="imports"]').count(), 0)
    } finally {
      await close()
    }
  }
})

test("a folder dropped from the desktop lands in the folder under the pointer, structure kept", async () => {
  const { page, errors, close } = await open()
  try {
    // A folder as the browser hands it over (FileSystemEntry API).
    await page.evaluate(() => {
      const file = (name, text, fullPath) => ({ isFile: true, isDirectory: false, name, fullPath, file: (ok) => ok(new File([text], name)) })
      const dir = (name, fullPath, children) => ({
        isFile: false,
        isDirectory: true,
        name,
        fullPath,
        createReader: () => {
          let done = false
          return { readEntries: (ok) => (ok(done ? [] : children), (done = true)) }
        },
      })
      const model = dir("model", "/model", [
        file("a.ttl", "@prefix ex: <https://e.org/> .\nex:a ex:b ex:c .\n", "/model/a.ttl"),
        file("readme.md", "# no\n", "/model/readme.md"),
        dir("shapes", "/model/shapes", [file("b.ttl", "@prefix ex: <https://e.org/> .\nex:d ex:e ex:f .\n", "/model/shapes/b.ttl")]),
      ])
      const drop = new Event("drop", { bubbles: true, cancelable: true })
      Object.defineProperty(drop, "dataTransfer", { value: { getData: () => "", items: [{ webkitGetAsEntry: () => model }], files: [] } })
      document.querySelector('[role=treeitem][data-path="data"]').dispatchEvent(drop)
    })
    await item(page, "data/model/shapes/b.ttl").waitFor()
    await item(page, "data/model/a.ttl").waitFor()
    assert.equal(await item(page, "data/model/readme.md").count(), 0)
    await page.locator("#toast", { hasText: "1 skipped" }).waitFor()
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
