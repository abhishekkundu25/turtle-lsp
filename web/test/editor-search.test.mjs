// The editor's Search view (web/app/search.js) in the installed Google
// Chrome, driven by Playwright: find across every file, the case, whole-word
// and regex options, opening a result, Replace All with undo, results that
// follow edits, and a bad regex. Run `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import path from "node:path"
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
  // The first-visit note is tested on its own (editor.test.mjs).
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors, close: () => ctx.close() }
}

const text = (page, p) => page.evaluate((p) => window.monaco.editor.getModel(window.monaco.Uri.parse(`file:///work/${p}`))?.getValue(), p)
const summary = (page) => page.locator("#view-search .sx-summary")
const fileRows = (page) => page.locator("#view-search .sx-file")
// The result groups as { path: count }.
const groups = (page) =>
  page.$$eval("#view-search .sx-file", (rows) => Object.fromEntries(rows.map((r) => [r.dataset.file, Number(r.querySelector(".sx-count").textContent)])))

// Open the Search view with Ctrl+Shift+F from inside the editor and search.
async function find(page, query) {
  await page.locator(".group .monaco-editor textarea").first().focus()
  await page.keyboard.press("ControlOrMeta+Shift+F")
  const input = page.getByRole("textbox", { name: "Search", exact: true })
  await assert.doesNotReject(input.waitFor())
  assert.equal(await input.evaluate((el) => el === document.activeElement), true, "Ctrl+Shift+F focuses the query")
  await input.fill(query)
  await input.press("Enter")
}

const sampleCount = (re) =>
  import("../app/samples.js").then(({ WORKSPACE }) => {
    const per = WORKSPACE.map((f) => [f.path, (f.text.match(re) ?? []).length]).filter(([, n]) => n)
    return { total: per.reduce((n, [, c]) => n + c, 0), files: per.length, per: Object.fromEntries(per) }
  })
const plural = (n, w) => `${n} ${w}${n === 1 ? "" : "s"}`
const expectSummary = async (page, { total, files }) => assert.equal(await summary(page).textContent(), `${plural(total, "result")} in ${plural(files, "file")}`)

test("Ctrl+Shift+F finds matches in several files, grouped by file with the match highlighted", async () => {
  const { page, errors, close } = await open()
  try {
    await find(page, "Person")
    const want = await sampleCount(/person/gi)
    await expectSummary(page, want)
    assert.deepEqual(await groups(page), want.per)
    assert.ok(want.files > 2)
    assert.equal(await page.locator("#act-search").getAttribute("class"), "act on")
    const marks = await page.locator("#view-search .sx-match mark").allTextContents()
    assert.equal(marks.length, want.total)
    for (const m of marks) assert.equal(m.toLowerCase(), "person")
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Match Case, Match Whole Word and Use Regular Expression change the results", async () => {
  const { page, errors, close } = await open()
  try {
    await find(page, "Person")
    await page.getByRole("button", { name: "Match Case" }).click()
    await expectSummary(page, await sampleCount(/Person/g))
    await page.getByRole("button", { name: "Match Whole Word" }).click()
    const whole = await sampleCount(/(?<![\w])Person(?![\w])/g)
    await expectSummary(page, whole)
    assert.deepEqual(await groups(page), whole.per)
    await page.getByRole("button", { name: "Match Whole Word" }).click()

    await page.getByRole("button", { name: "Use Regular Expression" }).click()
    assert.equal(await page.getByRole("button", { name: "Use Regular Expression" }).getAttribute("aria-pressed"), "true")
    await page.getByRole("textbox", { name: "Search", exact: true }).fill("ex:[a-z]+For\\b")
    await page.getByRole("textbox", { name: "Search", exact: true }).press("Enter")
    const re = await sampleCount(/ex:[a-z]+For\b/g)
    assert.ok(re.total > 0)
    await expectSummary(page, re)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("clicking a result opens its file with the match selected", async () => {
  const { page, errors, close } = await open()
  try {
    await find(page, "worksFor")
    const row = page.locator('#view-search .sx-file[data-file="shapes/person.ttl"] + .sx-match')
    await row.click()
    await page.getByRole("tab", { name: "person.ttl" }).waitFor()
    const sel = await page.evaluate(() => {
      const ed = window.monaco.editor.getEditors()[0]
      return { uri: ed.getModel().uri.toString(), text: ed.getModel().getValueInRange(ed.getSelection()), line: ed.getSelection().startLineNumber }
    })
    assert.equal(sel.uri, "file:///work/shapes/person.ttl")
    assert.equal(sel.text, "worksFor")
    const t = await text(page, "shapes/person.ttl")
    assert.equal(sel.line, t.slice(0, t.indexOf("worksFor")).split("\n").length)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("Replace All changes every file, says how many, and Ctrl+Z undoes it per file", async () => {
  const { page, errors, close } = await open()
  try {
    const before = await page.evaluate(() => Object.fromEntries(window.monaco.editor.getModels().map((m) => [m.uri.path.slice(6), m.getValue()])))
    await find(page, "Person")
    await page.getByRole("button", { name: "Match Case" }).click()
    const want = await sampleCount(/Person/g)
    await expectSummary(page, want)
    await page.getByRole("button", { name: "Toggle Replace" }).click()
    await page.getByRole("textbox", { name: "Replace", exact: true }).fill("Human")
    assert.equal(await page.locator("#view-search .sx-match ins").first().textContent(), "Human", "the preview shows the replacement")
    await page.getByRole("button", { name: "Replace All", exact: true }).click()
    await page.locator("#toast").getByText(`Replaced ${want.total} occurrences in ${want.files} files`).waitFor()
    assert.equal(await summary(page).textContent(), "No results found.")
    for (const p of Object.keys(want.per)) assert.equal(await text(page, p), before[p].replaceAll("Person", "Human"), p)

    // Undo in one file restores that file only.
    await page.locator("#act-explorer").click()
    await page.locator('#tree [data-path="data/people.ttl"]').click()
    await page.getByRole("tab", { name: "people.ttl" }).waitFor()
    await page.locator(".group .monaco-editor textarea").first().focus()
    await page.keyboard.press("ControlOrMeta+Z")
    assert.equal(await text(page, "data/people.ttl"), before["data/people.ttl"])
    assert.equal(await text(page, "ontology/core.ttl"), before["ontology/core.ttl"].replaceAll("Person", "Human"))
    // The view follows: the restored file's matches are back.
    await page.locator("#act-search").click()
    await page.locator("#view-search .sx-file").first().waitFor()
    assert.deepEqual(await groups(page), { "data/people.ttl": want.per["data/people.ttl"] })
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("the results follow edits and removed files", async () => {
  const { page, errors, close } = await open()
  try {
    await find(page, "zebra")
    await page.getByText("No results found.").waitFor()
    await page.evaluate(() => {
      const m = window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/data/people.ttl"))
      m.pushEditOperations([], [{ range: new window.monaco.Range(1, 1, 1, 1), text: "# zebra zebra\n" }], () => null)
      const n = window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/queries/people.rq"))
      n.pushEditOperations([], [{ range: new window.monaco.Range(1, 1, 1, 1), text: "# zebra\n" }], () => null)
    })
    await page.getByText("3 results in 2 files").waitFor()
    assert.deepEqual(await groups(page), { "data/people.ttl": 2, "queries/people.rq": 1 })

    // Delete a file in the explorer: its group goes.
    await page.locator("#act-explorer").click()
    await page.locator('#tree [data-path="queries/people.rq"]').click({ button: "right" })
    await page.getByRole("menuitem", { name: "Delete" }).click()
    await page.locator("#act-search").click()
    await page.getByText("2 results in 1 file").waitFor()
    assert.equal(await fileRows(page).count(), 1)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("an invalid regular expression shows an inline error, not an exception", async () => {
  const { page, errors, close } = await open()
  try {
    await find(page, "Person")
    await page.getByRole("button", { name: "Use Regular Expression" }).click()
    const input = page.getByRole("textbox", { name: "Search", exact: true })
    await input.fill("(Person")
    await input.press("Enter")
    const error = page.locator("#view-search .sx-error")
    await error.waitFor()
    assert.match(await error.textContent(), /unterminated group/i)
    assert.equal(await fileRows(page).count(), 0)
    await input.fill("(Person)")
    await input.press("Enter")
    await error.waitFor({ state: "hidden" })
    assert.ok((await fileRows(page).count()) > 0)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
