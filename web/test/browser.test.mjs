// The toolbox page in a real browser: the installed Google Chrome, driven by
// Playwright, with the page served from web/app by a static server on a free
// port. Run `npm run web:build` first.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import os from "node:os"
import http from "node:http"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium, webkit, firefox } from "playwright-core"

// BROWSER=webkit or BROWSER=firefox runs these in Playwright's builds of
// those browsers; the default is the installed Google Chrome.
const BROWSER = process.env.BROWSER ?? "chrome"
const launch = () => (BROWSER === "chrome" ? chromium.launch({ channel: "chrome", headless: true }) : { webkit, firefox }[BROWSER].launch({ headless: true }))

const app = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "app")
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm" }

let server, origin, browser
before(async () => {
  server = http.createServer((req, res) => {
    const rel = decodeURIComponent(new URL(req.url, "http://x").pathname).replace(/^\/$/, "/index.html")
    const file = path.join(app, path.normalize(rel))
    if (!file.startsWith(app) || !fs.existsSync(file)) return res.writeHead(404).end()
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

// A fresh page on the toolbox, ready (the first analysis shown), recording
// console errors and requests to anywhere but the page's own server.
async function open({ width = 1280, height = 900 } = {}) {
  const ctx = await browser.newContext({ viewport: { width, height } })
  const page = await ctx.newPage()
  const errors = []
  const foreign = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  page.on("request", (r) => !r.url().startsWith(origin) && !r.url().startsWith("data:") && foreign.push(r.url()))
  await page.goto(origin + "/")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 15_000 })
  return { page, errors, foreign, close: () => ctx.close() }
}

const tab = (page, name) => page.getByRole("tab", { name: new RegExp(`^${name}`) })
const count = async (page, name) => Number((await tab(page, name).locator(".count").textContent()).replace(/\D/g, "") || 0)
// Wait until the page has applied the analysis of the latest input.
const settled = (page) => page.locator("body[data-busy='false']").waitFor({ timeout: 10_000 })
const editor = (page) => page.getByRole("textbox", { name: "File text" })
const chooseFile = (page, name) => page.getByRole("button", { name, exact: true }).click()

test("the page opens on the sample input, analysed, with nothing loaded from elsewhere", async () => {
  const { page, errors, foreign, close } = await open()
  try {
    for (const name of ["Format", "Validate", "SHACL", "SPARQL"]) await tab(page, name).waitFor()
    assert.ok((await count(page, "Validate")) > 0)
    assert.equal(await count(page, "SHACL"), 2)
    assert.ok((await count(page, "Format")) > 0)
    assert.match(await page.locator("footer").textContent(), /turtle-lsp \d+\.\d+\.\d+/)
    assert.deepEqual(errors, [])
    assert.deepEqual(foreign, [])
  } finally {
    await close()
  }
})

test("validate lists problems with fixes, and one fix removes its problem", async () => {
  const { page, errors, close } = await open()
  try {
    await tab(page, "Validate").click()
    const before = await count(page, "Validate")
    await page.getByRole("button", { name: "Change to 'rdfs:label'" }).click()
    await settled(page)
    assert.equal(await count(page, "Validate"), before - 1)
    await chooseFile(page, "ontology.ttl")
    assert.match(await editor(page).inputValue(), /rdfs:label "Person"/)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("apply all fixes every fixable problem in every file", async () => {
  const { page, close } = await open()
  try {
    await tab(page, "Validate").click()
    await page.getByRole("button", { name: /^Apply all \d+ fixes$/ }).click()
    await settled(page)
    assert.equal(await page.getByRole("button", { name: /^Apply all/ }).count(), 0)
    await chooseFile(page, "data.ttl")
    assert.match(await editor(page).inputValue(), /@prefix rdfs: /)
  } finally {
    await close()
  }
})

test("apply all fixes on the SPARQL side uses the RDF files' terms", async () => {
  const { page, errors, close } = await open()
  try {
    await chooseFile(page, "people.rq")
    await editor(page).fill((await editor(page).inputValue()).replace("a ex:Person", "a ex:Persn"))
    await settled(page)
    await tab(page, "SPARQL").click()
    await page.getByText("Did you mean 'ex:Person'?").waitFor()
    await page.getByRole("button", { name: /^Apply all \d+ fixes$/ }).click()
    await settled(page)
    assert.match(await editor(page).inputValue(), /\?p a ex:Person ;/)
    assert.match(await editor(page).inputValue(), /^PREFIX foaf: /m)
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})

test("clicking a problem shows its file and line in the input", async () => {
  const { page, close } = await open()
  try {
    await tab(page, "Validate").click()
    await page.locator(".problem", { hasText: "rdfs:labl" }).getByRole("button", { name: /ontology\.ttl:\d+/ }).click()
    assert.match(await editor(page).inputValue(), /rdfs:labl/)
    const line = await page.locator(".line.current").textContent()
    assert.match(line, /rdfs:labl/)
  } finally {
    await close()
  }
})

test("format shows the diff with its guarantee, and applying it updates the input", async () => {
  const { page, close } = await open()
  try {
    await tab(page, "Format").click()
    await chooseFile(page, "data.ttl")
    await page.getByText("Same graph").waitFor()
    assert.ok((await page.locator(".diff .add").count()) > 0)
    const before = await count(page, "Format")
    await page.getByRole("button", { name: "Apply formatting" }).click()
    await settled(page)
    assert.equal(await count(page, "Format"), before - 1)
    assert.match(await editor(page).inputValue(), /^id:acme a ex:Organisation ;\n {4}rdfs:label "ACME" \.$/m)
    await page.getByText("Already formatted").waitFor()
  } finally {
    await close()
  }
})

test("typing updates every tool; a syntax error stops formatting and says why", async () => {
  const { page, close } = await open()
  try {
    await chooseFile(page, "data.ttl")
    const before = await count(page, "Validate")
    await editor(page).press("ControlOrMeta+End")
    await editor(page).pressSequentially("\nid:x ex:name .\n")
    await settled(page)
    assert.ok((await count(page, "Validate")) > before)
    await tab(page, "Format").click()
    await page.getByText(/not formatted.*syntax error/i).waitFor()
  } finally {
    await close()
  }
})

test("the SHACL tab follows the shapes in the input", async () => {
  const { page, close } = await open()
  try {
    await tab(page, "SHACL").click()
    await page.getByText("Every person needs a name").first().waitFor()
    await page.getByRole("combobox", { name: "Sample input" }).selectOption("queries")
    await settled(page)
    assert.equal(await tab(page, "SHACL").count(), 0)
  } finally {
    await close()
  }
})

test("SPARQL shows the query's problems and fixes, and its formatted text", async () => {
  const { page, close } = await open()
  try {
    await tab(page, "SPARQL").click()
    await page.getByText("Not formatted: the query has 1 error; fix it first").waitFor()
    await page.getByRole("button", { name: /^Add PREFIX foaf: / }).first().click()
    await settled(page)
    await chooseFile(page, "people.rq")
    assert.match(await editor(page).inputValue(), /^PREFIX foaf: <http:\/\/xmlns\.com\/foaf\/0\.1\/>$/m)
    await page.getByText("Same SPARQL algebra").waitFor()
    assert.ok((await page.locator(".diff .add").count()) > 0)
  } finally {
    await close()
  }
})

test("pasted text and opened files join the input", async () => {
  const { page, close } = await open()
  try {
    await page.getByRole("button", { name: "Paste" }).click()
    await page.getByRole("textbox", { name: "Pasted text" }).fill("@prefix ex: <https://e.org/> .\nex:a ex:b ex:c .\n")
    await page.getByRole("button", { name: "Add to input" }).click()
    await settled(page)
    await page.getByRole("button", { name: "pasted.ttl", exact: true }).waitFor()
    await page.locator("input[type=file]").first().setInputFiles({
      name: "extra.rq",
      mimeType: "application/sparql-query",
      buffer: Buffer.from("SELECT * WHERE { ?s ?p ?o }\n"),
    })
    await settled(page)
    await page.getByRole("button", { name: "extra.rq", exact: true }).waitFor()
  } finally {
    await close()
  }
})

test("an opened folder adds its RDF and SPARQL files, subfolders included, and skips the rest", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-web-folder-"))
  const { page, errors, close } = await open()
  try {
    fs.mkdirSync(path.join(dir, "shapes"))
    fs.writeFileSync(path.join(dir, "people.ttl"), "@prefix ex: <https://e.org/> .\nex:a ex:b ex:c .\n")
    fs.writeFileSync(path.join(dir, "shapes", "people.ttl"), "@prefix ex: <https://e.org/> .\nex:s ex:p ex:o .\n")
    fs.writeFileSync(path.join(dir, "ask.rq"), "ASK { ?s ?p ?o }\n")
    fs.writeFileSync(path.join(dir, "notes.md"), "# not RDF\n")
    await page.getByRole("button", { name: "Open folder" }).click()
    await page.locator("#folder-picker").setInputFiles(dir)
    await settled(page)
    for (const name of ["people.ttl", "shapes/people.ttl", "ask.rq"]) {
      await page.getByRole("button", { name, exact: true }).waitFor()
    }
    assert.equal(await page.getByRole("button", { name: "notes.md", exact: true }).count(), 0)
    await page.getByText("1 file skipped").waitFor()
    assert.deepEqual(errors, [])
  } finally {
    await close()
    fs.rmSync(dir, { recursive: true, force: true })
  }
})

test("on a phone the panes stack and nothing scrolls sideways", async () => {
  const { page, errors, close } = await open({ width: 390, height: 844 })
  try {
    const sideways = await page.evaluate(() => document.scrollingElement.scrollWidth - window.innerWidth)
    assert.ok(sideways <= 0, `${sideways}px of horizontal scroll`)
    const [input, tool] = await Promise.all([page.locator(".pane.input").boundingBox(), page.locator(".pane.tool").boundingBox()])
    assert.ok(tool.y >= input.y + input.height - 1, "the tool pane is below the input")
    for (const name of ["Format", "Validate", "SHACL", "SPARQL"]) assert.ok(await tab(page, name).isVisible())
    assert.deepEqual(errors, [])
  } finally {
    await close()
  }
})
