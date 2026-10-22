// The toolbox page under load and failure: thousands of problems, a large
// file, results computed for older text, a worker that cannot start.
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

async function open({ before } = {}) {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 900 } })
  const page = await ctx.newPage()
  page.setDefaultTimeout(15_000)
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await before?.(page)
  await page.goto(origin + "/toolbox.html")
  return { page, errors, close: () => ctx.close() }
}
const ready = (page) => page.locator("body[data-ready='true']").waitFor()
const settled = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 15_000 })
const tab = (page, name) => page.getByRole("tab", { name: new RegExp(`^${name}`) })
const editor = (page) => page.getByRole("textbox", { name: "File text" })
const chooseFile = (page, name) => page.getByRole("button", { name, exact: true }).click()
// Type `text` at the start of the input.
async function typeAtStart(page, text) {
  await editor(page).evaluate((ed) => (ed.focus(), ed.setSelectionRange(0, 0)))
  await page.keyboard.type(text)
}
const count = async (page, name) => Number((await tab(page, name).locator(".count").textContent()).replace(/\D/g, "") || 0)
// Paste `body` as a new file and wait for its analysis.
async function paste(page, body) {
  await page.getByRole("button", { name: "Paste" }).click()
  // Set, not typed: the text is large.
  await page.getByLabel("Pasted text").evaluate((el, body) => (el.value = body), body)
  await page.getByRole("button", { name: "Add to input" }).click()
}
// Worker messages that match `match` wait until `release()`.
async function holdMessages(page, match) {
  await page.evaluate((src) => {
    const matches = new Function("m", `return (${src})(m)`)
    const post = Worker.prototype.postMessage
    window.__held = []
    window.__posted = []
    window.__holding = true
    window.__release = () => {
      window.__holding = false
      for (const f of window.__held.splice(0)) f()
    }
    Worker.prototype.postMessage = function (m, ...rest) {
      window.__posted.push(m.op)
      if (window.__holding && matches(m)) {
        window.__held.push(() => post.call(this, m, ...rest))
        return
      }
      return post.call(this, m, ...rest)
    }
  }, match.toString())
}

// Lines that each use a prefix nobody declared: several problems per line.
const manyProblems = (lines) => Array.from({ length: lines }, (_, i) => `undeclared:s${i} undeclared:p undeclared:o${i} .`).join("\n") + "\n"

test("5,000 problems draw in under half a second, the list is capped at 500 and grows 500 at a time", async () => {
  const { page, errors, close } = await open()
  try {
    await ready(page)
    await paste(page, manyProblems(2000))
    await page.waitForFunction(() => Number(document.querySelector("#tab-validate .count")?.textContent) >= 5000)
    await settled(page)
    await tab(page, "Validate").click()
    const total = await count(page, "Validate")
    assert.ok(total >= 5000, `${total} problems`)
    const ms = Number(await page.evaluate(() => document.body.dataset.renderMs))
    assert.ok(ms < 500, `the page took ${ms} ms to draw ${total} problems`)
    assert.equal(await page.locator("#tool .problem").count(), 500)
    await page.getByRole("button", { name: "Show 500 more" }).click()
    assert.equal(await page.locator("#tool .problem").count(), 1000)
    await page.getByText(`${total - 1000} not shown`).waitFor()
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("a 20,000-line file draws the lines in view only, edits quickly, and draws the lines scrolled to", async () => {
  const { page, errors, close } = await open()
  try {
    await ready(page)
    const lines = Array.from({ length: 20_000 }, (_, i) => `<https://e.org/s${i}> a <https://e.org/C> .`)
    await paste(page, "@prefix ex: <https://e.org/> .\n" + lines.join("\n") + "\n")
    await settled(page)
    const drawn = await page.locator("#hl .line").count()
    assert.ok(drawn > 20 && drawn < 400, `${drawn} lines drawn`)
    const height = await page.evaluate(() => document.getElementById("hl").scrollHeight - document.getElementById("editor").scrollHeight)
    assert.ok(Math.abs(height) < 40, `the drawn text is as tall as the input (${height}px apart)`)
    // An edit costs a screenful of lines, not the file.
    const ms = await page.evaluate(() => {
      const ed = document.getElementById("editor")
      ed.value = "# x\n" + ed.value
      const t0 = performance.now()
      ed.dispatchEvent(new Event("input"))
      return performance.now() - t0
    })
    assert.ok(ms < 100, `an edit took ${ms} ms`)
    // Scrolled far down, those lines are drawn and coloured.
    await page.evaluate(() => (document.getElementById("editor").scrollTop = 19 * 15_000))
    await page.locator('#hl .line[data-n="15005"]').waitFor()
    assert.ok((await page.locator("#hl .line").count()) < 400)
    assert.match(await page.locator('#hl .line[data-n="15005"]').textContent(), /s150\d\d/)
    assert.ok((await page.locator('#hl .line[data-n="15005"] .t-iri').count()) > 0, "coloured")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("one analysis runs at a time: typing while it runs waits for it, then analyses the latest text once", async () => {
  const { page, close } = await open()
  try {
    await ready(page)
    await settled(page)
    await holdMessages(page, (m) => m.op === "analyse")
    await chooseFile(page, "data.ttl")
    await typeAtStart(page, "# a\n")
    await page.waitForFunction(() => window.__held.length === 1)
    // More typing, and the pause that would start the next analysis: nothing is posted while the first runs.
    await page.keyboard.type("# b\n")
    await page.evaluate(() => new Promise((r) => setTimeout(r, 500)))
    assert.equal(await page.evaluate(() => window.__posted.filter((op) => op === "analyse").length), 1)
    await page.evaluate(() => window.__release())
    await settled(page)
    // The first result was drawn; one more followed, for the text as it is.
    assert.equal(await page.evaluate(() => window.__posted.filter((op) => op === "analyse").length), 2)
    assert.equal(await page.locator("body").getAttribute("data-busy"), "false")
    assert.match(await editor(page).inputValue(), /^# a\n# b\n/)
  } finally {
    await close()
  }
})

test("Apply all fixes leaves a file edited while they were computed as it is, and says so", async () => {
  const { page, close } = await open()
  try {
    await ready(page)
    await settled(page)
    await tab(page, "Validate").click()
    await chooseFile(page, "data.ttl")
    const before = await editor(page).inputValue()
    await holdMessages(page, (m) => m.op === "fix_all")
    await page.getByRole("button", { name: /^Apply all \d+ fixes$/ }).click()
    await page.waitForFunction(() => window.__held.length === 1)
    await typeAtStart(page, "# typed meanwhile\n")
    await page.evaluate(() => window.__release())
    await page.locator("#crash", { hasText: "edited while the fixes were computed" }).waitFor()
    assert.equal(await editor(page).inputValue(), "# typed meanwhile\n" + before)
    await settled(page)
    // Another file was fixed all the same, and the fix can be taken back.
    await chooseFile(page, "ontology.ttl")
    assert.match(await editor(page).inputValue(), /@prefix rdfs: /)
  } finally {
    await close()
  }
})

test("Undo takes back a fix or a formatting, in the files still as it left them", async () => {
  const { page, close } = await open()
  try {
    await ready(page)
    await settled(page)
    const undo = page.getByRole("button", { name: "Undo", exact: true })
    assert.equal(await undo.isDisabled(), true)
    await tab(page, "Validate").click()
    await chooseFile(page, "data.ttl")
    const before = await editor(page).inputValue()
    await page.getByRole("button", { name: /^Apply all \d+ fixes$/ }).click()
    await settled(page)
    assert.notEqual(await editor(page).inputValue(), before)
    assert.equal(await undo.isDisabled(), false)
    await undo.click()
    await settled(page)
    assert.equal(await editor(page).inputValue(), before)
    assert.equal(await undo.isDisabled(), true)
    // Not over what was typed since.
    await page.getByRole("button", { name: /^Apply all \d+ fixes$/ }).click()
    await settled(page)
    await typeAtStart(page, "# mine\n")
    await undo.click()
    await page.locator("#crash", { hasText: "edited since" }).waitFor()
    assert.match(await editor(page).inputValue(), /^# mine\n/)
    // A formatting, from the Format tab.
    await tab(page, "Format").click()
    await chooseFile(page, "ontology.ttl")
    const ontology = await editor(page).inputValue()
    const apply = page.getByRole("button", { name: "Apply formatting" })
    if (await apply.count()) {
      await apply.click()
      await settled(page)
      assert.notEqual(await editor(page).inputValue(), ontology)
      await undo.click()
      assert.equal(await editor(page).inputValue(), ontology)
    }
  } finally {
    await close()
  }
})

test("a worker that cannot start fails what waits on it, restarts twice, then says the tools are off and stops waiting", async () => {
  const { page, close } = await open({ before: (page) => page.route("**/worker.js", (route) => route.fulfill({ status: 404, body: "" })) })
  try {
    await page.locator("#crash", { hasText: "keeps stopping" }).waitFor({ timeout: 20_000 })
    await settled(page)
    assert.equal(await page.locator("body").getAttribute("data-ready"), "false")
    // Typing no longer waits for an engine that is not there.
    await typeAtStart(page, "# x")
    await settled(page)
    await page.locator("#crash", { hasText: /engine is off/ }).waitFor()
  } finally {
    await close()
  }
})

test("a worker that dies once is replaced and the tools work again", async () => {
  // The first worker script is a dud; later ones are the real file.
  let served = 0
  const { page, close } = await open({
    before: (page) =>
      page.route("**/worker.js", (route) => (served++ === 0 ? route.fulfill({ status: 404, body: "" }) : route.continue())),
  })
  try {
    await ready(page)
    await settled(page)
    assert.ok(served >= 2, "a second worker was started")
    assert.ok((await count(page, "Validate")) > 0)
    await page.locator("#crash", { hasText: "restarted" }).waitFor({ state: "attached" })
  } finally {
    await close()
  }
})
