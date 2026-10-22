// The editor's whole-workspace commands and its one network call keep their
// guard: Fix All and Format All never write a result computed for an older
// text over newer edits, the server is opened with the model's own text, and
// the fetch quick fix refuses what it should. Run `npm run web:build` first.
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

async function open() {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  const page = await ctx.newPage()
  page.setDefaultTimeout(10_000)
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors, close: () => ctx.close() }
}
const settled = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 10_000 })
const item = (page, p) => page.locator(`[role=treeitem][data-path="${p}"]`)
const text = (page, p) => page.evaluate((p) => window.monaco.editor.getModel(window.monaco.Uri.parse(`file:///work/${p}`))?.getValue(), p)
const run = (page, id) =>
  page.evaluate((id) => {
    const ed = window.monaco.editor.getEditors()[0]
    ed.focus()
    ed.trigger("test", id, {})
  }, id)
// An empty workspace holding `files` ({ "a.ttl": text }).
async function workspaceOf(page, files) {
  await run(page, "workspace.clear")
  await page.waitForFunction(() => !document.querySelector("[role=treeitem]"))
  await page.locator("#files-input").setInputFiles(Object.entries(files).map(([name, body]) => ({ name, mimeType: "text/plain", buffer: Buffer.from(body) })))
  for (const name of Object.keys(files)) await item(page, name).waitFor()
  await settled(page)
}
// Messages to the language server worker that match `match` are held back
// until `release()`; the others go on.
async function holdMessages(page, match) {
  await page.evaluate((src) => {
    const matches = new Function("m", `return (${src})(m)`)
    const post = Worker.prototype.postMessage
    window.__held = []
    window.__holding = true
    window.__release = () => {
      window.__holding = false
      for (const f of window.__held.splice(0)) f()
    }
    Worker.prototype.postMessage = function (m, ...rest) {
      if (window.__holding && matches(m)) {
        window.__held.push(() => post.call(this, m, ...rest))
        return
      }
      return post.call(this, m, ...rest)
    }
  }, match.toString())
}
const edit = (page, path, line) =>
  page.evaluate(([path, line]) => {
    const m = window.monaco.editor.getModel(window.monaco.Uri.parse(`file:///work/${path}`))
    m.applyEdits([{ range: new window.monaco.Range(1, 1, 1, 1), text: line }])
  }, [path, line])

const UNDECLARED = "foaf:a foaf:b foaf:c .\n"

test("Fix All leaves a file edited while the fixes were computed as it is, fixes the others, and says so", async () => {
  const { page, errors, close } = await open()
  try {
    await workspaceOf(page, { "a.ttl": UNDECLARED, "b.ttl": UNDECLARED })
    await holdMessages(page, (m) => m.kind === "fixAll")
    await run(page, "rdf.fixAll")
    await page.waitForFunction(() => window.__held.length === 1)
    await edit(page, "a.ttl", "# typed meanwhile\n")
    await page.evaluate(() => window.__release())
    await page.locator("#toast", { hasText: "changed meanwhile" }).waitFor()
    assert.equal(await text(page, "a.ttl"), "# typed meanwhile\n" + UNDECLARED, "the edit stays, the old fix is not written over it")
    assert.match(await text(page, "b.ttl"), /^@prefix foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/> \./)
    // Run again on the current text: now it is fixed.
    await page.evaluate(() => (window.__holding = false))
    await run(page, "rdf.fixAll")
    await page.waitForFunction(() => /@prefix foaf:/.test(window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/a.ttl")).getValue()))
    assert.match(await text(page, "a.ttl"), /# typed meanwhile/)
    // One undo takes a whole file's fix back.
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.setModel(window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/b.ttl")))
      ed.focus()
      ed.trigger("test", "undo", {})
    })
    assert.equal(await text(page, "b.ttl"), UNDECLARED)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Format All skips a file edited while it was formatted, formats the others, and says so", async () => {
  const { page, errors, close } = await open()
  try {
    const MESSY = "@prefix ex: <https://e.org/> .\nex:a    ex:b   ex:c ;ex:d ex:e .\n"
    await workspaceOf(page, { "a.ttl": MESSY, "b.ttl": MESSY })
    await holdMessages(page, (m) => m.kind === "request" && m.method === "textDocument/formatting")
    await run(page, "rdf.formatAll")
    // Formatting asks file by file: the first request is waiting.
    await page.waitForFunction(() => window.__held.length === 1)
    await edit(page, "a.ttl", "# typed meanwhile\n")
    await page.evaluate(() => window.__release())
    await page.locator("#toast", { hasText: "changed meanwhile" }).waitFor()
    assert.equal(await text(page, "a.ttl"), "# typed meanwhile\n" + MESSY)
    assert.match(await text(page, "b.ttl"), /ex:a ex:b ex:c ;\n\s+ex:d ex:e \.\n/)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("the server is opened with the model's text, which Monaco may have given other line endings", async () => {
  const { page, close } = await open()
  try {
    await page.evaluate(() => {
      window.__opened = []
      const post = Worker.prototype.postMessage
      Worker.prototype.postMessage = function (m, ...rest) {
        if (m?.method === "textDocument/didOpen") window.__opened.push(m.params.textDocument)
        return post.call(this, m, ...rest)
      }
    })
    const mixed = "# one\r\n# two\n# three\n"
    await workspaceOf(page, { "mixed.ttl": mixed })
    const model = await text(page, "mixed.ttl")
    assert.notEqual(model, mixed, "Monaco normalised the line endings")
    const opened = await page.evaluate(() => window.__opened.find((d) => d.uri.endsWith("/mixed.ttl")))
    assert.equal(opened.text, model)
  } finally {
    await close()
  }
})

async function fetchImport(page, iri) {
  await page.evaluate((iri) => window.monaco.editor.getEditors()[0].trigger("test", "turtle.fetchImport", iri), iri)
}

test("fetchImport refuses an address that is not http(s), says so, and does not throw", async () => {
  const { page, errors, close } = await open()
  try {
    for (const bad of ["not a url", "javascript:alert(1)", "file:///etc/passwd", "ftp://example.org/x.ttl", "https://example.org/x>\n<evil> a <y>", undefined, 42]) {
      await fetchImport(page, bad)
      await page.locator("#toast", { hasText: "not an http or https address" }).waitFor()
      await page.evaluate(() => (document.getElementById("toast").hidden = true))
    }
    assert.equal(await page.locator('[role=treeitem][data-path^="imports"]').count(), 0)
    assert.deepEqual(errors, [], "nothing was thrown out of the command")
  } finally {
    await close()
  }
})

test("fetchImport reads at most 10 MB: a longer answer is refused, whether or not it says how long it is", async () => {
  const { page, errors, close } = await open()
  try {
    const big = Buffer.alloc(11 * 1024 * 1024, "# x\n")
    await page.route("https://big.example.org/**", (route) => route.fulfill({ status: 200, headers: { "content-type": "text/turtle", "access-control-allow-origin": "*" }, body: big }))
    await fetchImport(page, "https://big.example.org/onto.ttl")
    await page.locator("#toast", { hasText: "larger than 10 MB" }).waitFor()
    assert.equal(await page.locator('[role=treeitem][data-path^="imports"]').count(), 0)
    // A small one is still taken.
    await page.route("https://small.example.org/**", (route) => route.fulfill({ status: 200, headers: { "content-type": "text/turtle", "access-control-allow-origin": "*" }, body: "<https://small.example.org/o> a <http://www.w3.org/2002/07/owl#Ontology> .\n" }))
    await fetchImport(page, "https://small.example.org/o.ttl")
    await item(page, "imports/small.example.org/o.ttl").waitFor()
    assert.deepEqual(errors.filter((e) => !/Failed to load resource/.test(e)), [])
  } finally {
    await close()
  }
})
