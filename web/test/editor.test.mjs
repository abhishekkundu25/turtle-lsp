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

// A fresh browser profile on the editor, ready: Monaco loaded, the server
// started and the first diagnostics shown.
async function open({ context } = {}) {
  const ctx = context ?? (await browser.newContext({ viewport: { width: 1440, height: 900 }, acceptDownloads: true }))
  const page = await ctx.newPage()
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
    await page.getByRole("tab", { name: "people.ttl" }).waitFor()
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
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 }, permissions: ["clipboard-read", "clipboard-write"] })
  try {
    const { page } = await open({ context })
    await item(page, "data/people.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Copy Relative Path" }).click()
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "data/people.ttl")
    await item(page, "data/people.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Copy Path", exact: true }).click()
    assert.equal(await page.evaluate(() => navigator.clipboard.readText()), "file:///work/data/people.ttl")
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
