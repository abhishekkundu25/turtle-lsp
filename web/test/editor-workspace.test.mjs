// How the editor keeps its workspace in the browser: one record per file
// with a revision, a connection that lets go when another tab upgrades the
// database, writes only for changes to files and layout, the `data-saved`
// signal, the move from the first storage layout, and two tabs on one
// workspace (the second reads, the first writes). Run `npm run web:build` first.
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

async function open(context) {
  const page = await context.newPage()
  page.setDefaultTimeout(8000)
  await page.addInitScript(() => localStorage.setItem("rdf-editor-intro-seen", "1"))
  const errors = []
  page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
  page.on("pageerror", (e) => errors.push(String(e)))
  await page.goto(origin + "/index.html")
  await page.locator("body[data-ready='true']").waitFor({ timeout: 20_000 })
  return { page, errors }
}
const newContext = () => browser.newContext({ viewport: { width: 1440, height: 900 } })
const saved = (page) => page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })
const text = (page, p) => page.evaluate((p) => window.monaco.editor.getModel(window.monaco.Uri.parse(`file:///work/${p}`))?.getValue(), p)
const item = (page, p) => page.locator(`[role=treeitem][data-path="${p}"]`)
const openFile = async (page, p) => {
  await item(page, p).click()
  await page.getByRole("tab", { name: p.split("/").pop() }).waitFor()
}
// Put the cursor on `needle` in the active editor.
const at = (page, needle) =>
  page.evaluate((needle) => {
    const ed = window.monaco.editor.getEditors()[0]
    const m = ed.getModel()
    ed.setPosition(m.getPositionAt(m.getValue().indexOf(needle)))
    ed.focus()
  }, needle)
// The kept records, read straight from IndexedDB.
const records = (page) =>
  page.evaluate(
    () =>
      new Promise((resolve, reject) => {
        const req = indexedDB.open("rdf-toolbox-editor")
        req.onerror = () => reject(req.error)
        req.onsuccess = () => {
          const db = req.result
          const tx = db.transaction(["files", "meta"])
          const out = {}
          tx.objectStore("files").getAll().onsuccess = (e) => (out.files = e.target.result)
          tx.objectStore("meta").get("layout").onsuccess = (e) => (out.layout = e.target.result)
          tx.oncomplete = () => (db.close(), resolve(out))
        }
      })
  )

test("the workspace is kept as one record per file, each with a revision, and the layout apart", async () => {
  const ctx = await newContext()
  try {
    const { page, errors } = await open(ctx)
    await saved(page)
    const kept = await records(page)
    assert.equal(kept.files.length, 11)
    assert.ok(kept.files.every((f) => f.rev === 1 && typeof f.text === "string"))
    assert.deepEqual(Object.keys(kept.layout).sort(), ["expanded", "focused", "folders", "groups", "roots", "settings"])
    assert.equal(kept.layout.groups[0].active, "start-here.ttl")
    assert.deepEqual(errors, [])
  } finally {
    await ctx.close()
  }
})

test("only what changed is written: an edit writes its file with the next revision; a cursor move and a diagnostics publish write nothing", async () => {
  const ctx = await newContext()
  try {
    const { page } = await open(ctx)
    await saved(page)
    // Count every record put into IndexedDB from here on.
    await page.evaluate(() => {
      window.__puts = []
      const put = IDBObjectStore.prototype.put
      IDBObjectStore.prototype.put = function (value, key) {
        window.__puts.push(this.name + ":" + (key ?? value?.path ?? "?"))
        return put.call(this, value, key)
      }
    })
    // Cursor moves, and the diagnostics the server publishes meanwhile: no write is even asked for.
    await page.evaluate(() => window.monaco.editor.getEditors()[0].focus())
    for (let i = 0; i < 5; i++) await page.keyboard.press("ArrowDown")
    await page.waitForFunction(() => document.body.dataset.busy === "false")
    assert.equal(await page.evaluate(() => document.body.dataset.saved), "true", "nothing is waiting to be written")
    // An edit: its file only, one revision on.
    await openFile(page, "data/people.ttl")
    await saved(page)
    await page.evaluate(() => (window.__puts.length = 0))
    await at(page, "id:bob")
    await page.keyboard.type("# kept\n")
    assert.equal(await page.evaluate(() => document.body.dataset.saved), "false", "a change is waiting to be written")
    await saved(page)
    assert.deepEqual(await page.evaluate(() => window.__puts), ["files:data/people.ttl"])
    const kept = await records(page)
    assert.equal(kept.files.find((f) => f.path === "data/people.ttl").rev, 2)
    assert.match(kept.files.find((f) => f.path === "data/people.ttl").text, /# kept\nid:bob/)
    assert.ok(kept.files.filter((f) => f.path !== "data/people.ttl").every((f) => f.rev === 1), "the other files were not rewritten")
    // A structure change: the layout (and the new file).
    await page.evaluate(() => (window.__puts.length = 0))
    await page.getByRole("button", { name: "New File" }).click()
    await page.locator(".tree-input").fill("new.ttl")
    await page.locator(".tree-input").press("Enter")
    await item(page, "data/new.ttl").waitFor()
    await saved(page)
    assert.deepEqual((await page.evaluate(() => window.__puts)).sort(), ["files:data/new.ttl", "meta:layout"])
    // Deleting a file removes its record.
    await page.evaluate(() => (window.__puts.length = 0))
    await item(page, "data/new.ttl").click({ button: "right" })
    await page.getByRole("menuitem", { name: "Delete" }).click()
    await saved(page)
    assert.equal((await records(page)).files.some((f) => f.path === "data/new.ttl"), false)
  } finally {
    await ctx.close()
  }
})

test("the database connection is kept, and lets go when another tab upgrades the database", async () => {
  const ctx = await newContext()
  try {
    const { page } = await open(ctx)
    const result = await page.evaluate(async () => {
      const store = await import(location.origin + "/store.js")
      const name = "versionchange-test"
      const first = await store.open(name, 2)
      const again = await store.open(name, 2)
      const kept = first === again
      // Another tab asks for a newer version: a connection that stays open would block it.
      const upgraded = await new Promise((resolve) => {
        const req = indexedDB.open(name, 3)
        let blocked = false
        req.onblocked = () => (blocked = true)
        req.onsuccess = () => (req.result.close(), resolve({ blocked }))
        req.onerror = () => resolve({ error: String(req.error) })
        setTimeout(() => resolve({ timeout: true, blocked }), 4000)
      })
      indexedDB.deleteDatabase(name)
      return { kept, upgraded }
    })
    assert.equal(result.kept, true, "one connection")
    assert.deepEqual(result.upgraded, { blocked: false }, "the upgrade was not held up by this tab")
  } finally {
    await ctx.close()
  }
})

test("a file whose stored revision is not the one a tab saw is not overwritten", async () => {
  const ctx = await newContext()
  try {
    const { page } = await open(ctx)
    const out = await page.evaluate(async () => {
      const store = await import(location.origin + "/store.js")
      const name = "revision-test"
      const first = await store.write({ put: [{ path: "a.ttl", text: "one", rev: 0 }] }, name, 2)
      const stale = await store.write({ put: [{ path: "a.ttl", text: "two", rev: 0 }] }, name, 2)
      const second = await store.write({ put: [{ path: "a.ttl", text: "three", rev: 1 }], del: [] }, name, 2)
      const kept = await store.readAll(name, 2)
      const removed = await store.write({ del: ["a.ttl"], layout: { folders: [] } }, name, 2)
      const after = await store.readAll(name, 2)
      indexedDB.deleteDatabase(name)
      return { first: [first.ok, [...first.revs], first.conflicts], stale: [stale.ok, stale.conflicts], second: [second.ok, [...second.revs]], kept, removed: removed.ok, after }
    })
    assert.deepEqual(out.first, [true, [["a.ttl", 1]], []])
    assert.deepEqual(out.stale, [true, ["a.ttl"]])
    assert.deepEqual(out.second, [true, [["a.ttl", 2]]])
    assert.equal(out.kept, null, "no layout yet: nothing kept as a workspace")
    assert.equal(out.removed, true)
    assert.deepEqual(out.after, { files: [], layout: { folders: [] } })
  } finally {
    await ctx.close()
  }
})

test("a workspace kept by the first storage layout (one record) is carried over", async () => {
  const ctx = await newContext()
  try {
    // Make the old database where the page will look, before the page runs.
    const seed = await ctx.newPage()
    await seed.goto(origin + "/dom.js")
    await seed.evaluate(
      () =>
        new Promise((resolve, reject) => {
          const req = indexedDB.open("rdf-toolbox-editor", 1)
          req.onupgradeneeded = () => req.result.createObjectStore("workspace")
          req.onsuccess = () => {
            const tx = req.result.transaction("workspace", "readwrite")
            tx.objectStore("workspace").put(
              {
                files: [
                  { path: "old/a.ttl", text: "# from before\n", disk: undefined },
                  { path: "b.ttl", text: "# also\n" },
                ],
                folders: ["empty"],
                expanded: ["old"],
                roots: [],
                groups: [{ tabs: ["old/a.ttl"], active: "old/a.ttl" }],
                focused: 0,
                settings: { "editor.minimap.enabled": false },
              },
              "current"
            )
            tx.oncomplete = () => (req.result.close(), resolve())
            tx.onerror = () => reject(tx.error)
          }
        })
    )
    await seed.close()
    const { page, errors } = await open(ctx)
    await item(page, "old/a.ttl").waitFor()
    await item(page, "empty").waitFor()
    assert.equal(await text(page, "old/a.ttl"), "# from before\n")
    assert.equal(await text(page, "b.ttl"), "# also\n")
    await page.getByRole("tab", { name: "a.ttl" }).waitFor()
    const kept = await records(page)
    assert.deepEqual(kept.files.map((f) => [f.path, f.rev]).sort(), [["b.ttl", 1], ["old/a.ttl", 1]])
    assert.deepEqual(kept.layout.settings, { "editor.minimap.enabled": false })
    assert.equal(await page.evaluate(() => indexedDB.databases().then((l) => l.find((d) => d.name === "rdf-toolbox-editor").version)), 2)
    assert.deepEqual(errors, [])
  } finally {
    await ctx.close()
  }
})

test("two tabs: the second shows the workspace read-only with a notice, writes nothing, and takes over when the first closes", async () => {
  const ctx = await newContext()
  try {
    const first = await open(ctx)
    await saved(first.page)
    await openFile(first.page, "data/people.ttl")
    await at(first.page, "id:bob")
    await first.page.keyboard.type("# from the first tab\n")
    await saved(first.page)

    const second = await open(ctx)
    assert.equal(await second.page.evaluate(() => document.body.dataset.saved), "readonly")
    const notice = second.page.getByRole("dialog", { name: "Open in another tab" })
    await notice.waitFor()
    await notice.getByRole("button", { name: "Read only" }).click()
    await notice.waitFor({ state: "hidden" })
    assert.match(await second.page.locator("#status-storage").textContent(), /Read-only/)
    // It shows what the first tab kept.
    assert.match(await text(second.page, "data/people.ttl"), /# from the first tab\nid:bob/)
    // Typing is refused: the editors are read-only.
    assert.equal(await second.page.evaluate(() => window.monaco.editor.getEditors().every((e) => e.getOption(window.monaco.editor.EditorOption.readOnly))), true)
    // Its errors are still drawn in the text, not only listed in Problems.
    await second.page.waitForFunction(() => document.querySelectorAll(".view-overlays .squiggly-error").length > 0, null, { timeout: 10_000 })
    // Typing there says why it is refused, in the notice, not Monaco's bubble.
    await second.page.locator(".view-lines").first().click()
    await second.page.keyboard.type("x")
    await notice.waitFor()
    assert.equal(await second.page.locator(".monaco-editor-overlaymessage:visible").count(), 0)
    await second.page.keyboard.press("Escape")
    await notice.waitFor({ state: "hidden" })
    // So are changes to the files and folders, with the reason.
    await second.page.getByRole("button", { name: "New File" }).click()
    await notice.waitFor()
    await notice.getByRole("button", { name: "Read only" }).click()
    assert.equal(await second.page.locator(".tree-input").count(), 0)
    await second.page.evaluate(() => window.monaco.editor.getEditors()[0].trigger("test", "rdf.fixAll", {}))
    // Even a change made in code is not written.
    await second.page.evaluate(() => window.monaco.editor.getModel(window.monaco.Uri.parse("file:///work/data/people.ttl")).setValue("# overwritten by the second tab\n"))
    await second.page.evaluate(() => new Promise((r) => setTimeout(r, 700)))
    assert.equal(await second.page.evaluate(() => document.body.dataset.saved), "readonly")

    // The first tab goes on and still owns the workspace.
    await first.page.keyboard.type("# more\n")
    await saved(first.page)
    const kept = await records(first.page)
    assert.match(kept.files.find((f) => f.path === "data/people.ttl").text, /# from the first tab\n/)
    assert.doesNotMatch(kept.files.find((f) => f.path === "data/people.ttl").text, /overwritten/)

    // The first tab closes: the second owns the workspace, with what the first kept.
    await first.page.close()
    await second.page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })
    await second.page.locator("#toast", { hasText: "You can edit here now" }).waitFor()
    assert.match(await text(second.page, "data/people.ttl"), /# from the first tab\n# more\nid:bob/)
    assert.equal(await second.page.evaluate(() => window.monaco.editor.getEditors().some((e) => e.getOption(window.monaco.editor.EditorOption.readOnly))), false)
    await openFile(second.page, "data/people.ttl")
    await at(second.page, "id:bob")
    await second.page.keyboard.type("# from the second tab\n")
    await saved(second.page)
    assert.match((await records(second.page)).files.find((f) => f.path === "data/people.ttl").text, /# from the second tab\n/)
    assert.deepEqual(second.errors, [])
  } finally {
    await ctx.close()
  }
})

test("two tabs: Edit here moves the workspace to the second tab, with what the first had not written yet, and the first becomes read-only", async () => {
  const ctx = await newContext()
  try {
    const first = await open(ctx)
    await saved(first.page)
    const second = await open(ctx)
    const notice = second.page.getByRole("dialog", { name: "Open in another tab" })
    await notice.waitFor()

    // The first tab types; the hand-over writes it before the second tab reads.
    await openFile(first.page, "data/people.ttl")
    await at(first.page, "id:bob")
    await first.page.keyboard.type("# typed just before\n")
    await notice.getByRole("button", { name: "Edit here" }).click()
    await second.page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })
    await second.page.locator("#toast", { hasText: "You can edit here now" }).waitFor()
    assert.match(await text(second.page, "data/people.ttl"), /# typed just before\nid:bob/)

    // The first tab is read-only now, and says why.
    await first.page.getByRole("dialog", { name: "Editing moved to another tab" }).waitFor()
    assert.equal(await first.page.evaluate(() => document.body.dataset.saved), "readonly")
    assert.equal(await first.page.evaluate(() => window.monaco.editor.getEditors().every((e) => e.getOption(window.monaco.editor.EditorOption.readOnly))), true)

    // The second tab writes; the first tab can take it back the same way.
    await openFile(second.page, "data/people.ttl")
    await at(second.page, "id:bob")
    await second.page.keyboard.type("# from the second tab\n")
    await saved(second.page)
    await first.page.getByRole("dialog", { name: "Editing moved to another tab" }).getByRole("button", { name: "Edit here" }).click()
    await first.page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })
    assert.match(await text(first.page, "data/people.ttl"), /# from the second tab\n/)
    await second.page.getByRole("dialog", { name: "Editing moved to another tab" }).waitFor()
    assert.deepEqual(first.errors, [])
    assert.deepEqual(second.errors, [])
  } finally {
    await ctx.close()
  }
})

test("a tab that takes over the workspace takes the settings the other tab kept, and writes them back as they are", async () => {
  const ctx = await newContext()
  try {
    const first = await open(ctx)
    await saved(first.page)
    const second = await open(ctx)
    assert.equal(await second.page.evaluate(() => document.body.dataset.saved), "readonly")

    // The first tab changes a setting after the second one started.
    await first.page.locator(".group .monaco-editor textarea").first().focus()
    await first.page.keyboard.press("ControlOrMeta+Comma")
    const settings = first.page.getByRole("region", { name: "Settings" })
    await settings.getByLabel("Minimap: Enabled", { exact: true }).uncheck()
    await first.page.keyboard.press("Escape")
    await saved(first.page)
    assert.deepEqual((await records(first.page)).layout.settings, { "editor.minimap.enabled": false })

    await first.page.close()
    await second.page.locator("body[data-saved='true']").waitFor({ timeout: 10_000 })
    await second.page.locator("#toast", { hasText: "You can edit here now" }).waitFor()
    const minimap = () => second.page.evaluate(() => window.monaco.editor.getEditors().map((e) => e.getOption(window.monaco.editor.EditorOption.minimap).enabled))
    assert.ok((await minimap()).every((on) => on === false), String(await minimap()))
    // A change to the layout writes it with the settings the first tab kept.
    await openFile(second.page, "data/graphs.trig")
    await saved(second.page)
    const kept = await records(second.page)
    assert.ok(kept.layout.groups.some((g) => g.tabs.includes("data/graphs.trig")))
    assert.deepEqual(kept.layout.settings, { "editor.minimap.enabled": false })
    assert.deepEqual(second.errors, [])
  } finally {
    await ctx.close()
  }
})
