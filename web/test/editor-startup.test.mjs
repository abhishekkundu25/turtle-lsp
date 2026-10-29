// How soon the editor page shows a file, and colours it: the text comes with
// Monaco, before the language server has started, and a file shown for the
// first time gets the server's colours without Monaco's 300 ms wait. Run
// `npm run web:build` first.
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

async function context() {
  const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 } })
  await ctx.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  return ctx
}

// Whether the server's semantic tokens are on the model of the first editor.
const coloured = (page) => page.evaluate(() => Boolean(window.monaco?.editor.getEditors()[0]?.getModel()?.tokenization.hasCompleteSemanticTokens()))

test("the editor shows the file's text and Monaco's colours before the language server answers, and the server's features come when it does", async () => {
  const ctx = await context()
  // Hold the engine back: the worker cannot answer until it is let through.
  let release
  const held = new Promise((r) => (release = r))
  await ctx.route("**/engine.wasm", async (route) => (await held, route.continue()))
  const page = await ctx.newPage()
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  try {
    await page.goto(origin + "/index.html")
    await page.locator(".view-lines", { hasText: "@prefix" }).first().waitFor({ timeout: 15_000 })
    assert.equal(await page.locator("body").getAttribute("data-ready"), "false", "the server has not answered yet")
    // Monarch's colours: more than one token class on the lines shown.
    const classes = await page.evaluate(() => new Set([...document.querySelectorAll(".view-lines span span[class^='mtk']")].map((s) => s.className)).size)
    assert.ok(classes > 1, `Monaco colours the text (${classes} token classes)`)
    assert.equal(await coloured(page), false)

    release()
    await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
    // Nothing sent before the server started was lost: its colours, the
    // diagnostics of the files opened then, and its version in the status bar.
    await page.waitForFunction(() => window.monaco.editor.getEditors()[0].getModel().tokenization.hasCompleteSemanticTokens(), null, { timeout: 5_000 })
    assert.notEqual(await page.locator("#problems-count").textContent(), "0")
    assert.match(await page.locator("#status-version").textContent(), /^turtle-lsp \d/)
    assert.deepEqual(errors, [])
  } finally {
    release()
    await ctx.close()
  }
})

test("a file shown for the first time gets the server's colours within 150 ms, not after Monaco's 300 ms wait", async () => {
  const ctx = await context()
  const page = await ctx.newPage()
  try {
    await page.goto(origin + "/index.html")
    await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
    await page.locator("body[data-busy='false']").waitFor({ timeout: 10_000 })
    for (const path of ["ontology/core.ttl", "data/people.ttl"]) {
      // From the moment the editor takes the file to the moment its semantic tokens are on it.
      await page.evaluate(() => {
        const ed = window.monaco.editor.getEditors()[0]
        window.__colours = new Promise((resolve) => {
          const sub = ed.onDidChangeModel(() => {
            sub.dispose()
            const shown = performance.now()
            const model = ed.getModel()
            const poll = () => (model.tokenization.hasCompleteSemanticTokens() ? resolve(performance.now() - shown) : setTimeout(poll, 5))
            poll()
          })
        })
      })
      await page.locator(`[role=treeitem][data-path="${path}"]`).click()
      const ms = await page.evaluate(() => window.__colours)
      assert.ok(ms < 150, `${path}: the server's colours came ${Math.round(ms)} ms after the file was shown`)
    }
  } finally {
    await ctx.close()
  }
})
