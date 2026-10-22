// The Content-Security-Policy of both pages, enforced by the browser: the
// pages and Monaco run under it without a violation, and what it exists to
// stop is stopped. Run `npm run web:build` first.
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

// A page that records every policy violation the browser reports.
async function open(url, ready) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } })
  const page = await ctx.newPage()
  page.setDefaultTimeout(15_000)
  await page.addInitScript(() => {
    window.__violations = []
    document.addEventListener("securitypolicyviolation", (e) => window.__violations.push(`${e.violatedDirective} ${e.blockedURI}`))
    localStorage.setItem("rdf-editor-intro-seen", "1")
  })
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + url)
  await page.locator(ready).waitFor({ timeout: 20_000 })
  return { page, errors, close: () => ctx.close() }
}
const violations = (page) => page.evaluate(() => window.__violations)

// What an attacker's markup would try: an inline script, a script from elsewhere,
// an inline event handler, a request to another site, a framed page. (Not eval:
// the browser lets code the test driver runs through; app-modules.test.mjs
// checks the policies do not allow it.)
async function attack(page) {
  return page.evaluate(async () => {
    window.__ran = []
    const inline = document.createElement("script")
    inline.textContent = "window.__ran.push('inline script')"
    document.head.append(inline)
    const outside = document.createElement("script")
    outside.src = "https://cdn.example.org/x.js"
    document.head.append(outside)
    const handler = document.createElement("img")
    handler.setAttribute("onerror", "window.__ran.push('handler')")
    handler.src = "data:image/gif;base64,x"
    document.body.append(handler)
    let fetched = true
    try {
      await fetch("https://example.org/", { mode: "no-cors" })
    } catch {
      fetched = false
    }
    const frame = document.createElement("iframe")
    frame.src = "https://example.org/"
    document.body.append(frame)
    await new Promise((r) => setTimeout(r, 300))
    return { ran: window.__ran, fetched }
  })
}

test("the toolbox runs under its policy with no violation, and the policy stops injected script, other sites and frames", async () => {
  const { page, errors, close } = await open("/toolbox.html", "body[data-ready='true']")
  try {
    // Use it: type, format, a fix; the engine runs in its worker.
    await page.getByRole("tab", { name: /^Validate/ }).click()
    await page.getByRole("button", { name: /^Apply all \d+ fixes$/ }).click()
    await page.locator("body[data-busy='false']").waitFor()
    assert.deepEqual(await violations(page), [])
    assert.deepEqual(errors, [])
    const result = await attack(page)
    assert.deepEqual(result, { ran: [], fetched: false })
    const blocked = await violations(page)
    for (const directive of ["script-src-elem", "script-src", "connect-src", "frame-src"]) assert.ok(blocked.some((v) => v.startsWith(directive)), `${directive} blocked something: ${blocked}`)
    // A <base> tag cannot redirect relative URLs.
    assert.equal(await page.evaluate(() => { const b = document.createElement("base"); b.href = "https://example.org/"; document.head.append(b); return new URL("a.js", document.baseURI).href.startsWith(location.origin) }), true)
  } finally {
    await close()
  }
})

test("the editor runs under its policy with no violation (Monaco, its workers, its fonts), and the policy stops injected script, other sites' scripts and frames", async () => {
  const { page, errors, close } = await open("/index.html", "body[data-ready='true']")
  try {
    // Monaco's own work: a diff editor (its worker), hover, the command palette.
    await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      ed.focus()
      ed.trigger("test", "rdf.previewFormat", {})
    })
    await page.locator(".dif").waitFor({ state: "attached" })
    await page.evaluate(() => window.monaco.editor.getEditors()[0].trigger("test", "editor.action.quickCommand", {}))
    await page.keyboard.press("Escape")
    assert.deepEqual(await violations(page), [])
    assert.deepEqual(errors, [])
    const result = await attack(page)
    // The editor page may read other sites (the fetch quick fix); the rest is stopped.
    assert.deepEqual(result.ran, [])
    const blocked = await violations(page)
    for (const directive of ["script-src-elem", "script-src", "frame-src"]) assert.ok(blocked.some((v) => v.startsWith(directive)), `${directive} blocked something: ${blocked}`)
  } finally {
    await close()
  }
})
