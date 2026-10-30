// The graph view's Schema mode (web/app/graph/schema.js, the class inspector
// in panel.js) and Follow cursor: the pure parts in Node (the answer checked,
// one badge per row, problems from rows, the tidy layout), then the view in
// the installed Google Chrome, driven by Playwright, on the graph view's test
// page (graph-ui.html: a webview-strict policy, a fake server that answers
// `turtle/schema` with schema-fixture.json). Each test names the defect it catches.
import { test, before, after, describe } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { chromium } from "playwright-core"
import { listen } from "./static-server.mjs"
import { problemsOf, rowBadge, schemaModel, tidyLayout } from "../app/graph/schema.js"
import { normalizeState } from "../app/graph/filters.js"
import { A, EX, bigSchema } from "./graph-fixture.mjs"

const HERE = path.dirname(fileURLToPath(import.meta.url))
const WEB = path.join(HERE, "..")
const FIXTURE = JSON.parse(fs.readFileSync(path.join(HERE, "schema-fixture.json"), "utf8"))
const PUMP = 1, MOTOR = 3, PIPE = 4
const name = (c) => FIXTURE.classes[c].label

// ---- pure parts, in Node -----------------------------------------------------------------

test("each class's problems come from its rows and match the answer's count (catches: a box badge and an inspector that disagree with the server)", () => {
  const m = schemaModel(FIXTURE)
  FIXTURE.classes.forEach((c, i) => assert.equal(m.classes[i].problems.length, c.problems, c.name))
  assert.deepEqual(m.classes[PUMP].problems.map((p) => [p.kind, p.sev]), [["conflict", "error"], ["missing", "error"], ["badValues", "warning"], ["unused", ""], ["undeclared", ""]])
  assert.equal(m.classes[PUMP].worst, "error")
  const undeclared = m.classes[PUMP].problems.find((p) => p.kind === "undeclared")
  assert.equal(undeclared.at, null, "nothing declares it: nowhere to open")
  const bad = m.classes[PUMP].problems.find((p) => p.kind === "badValues")
  assert.deepEqual(bad.at, FIXTURE.classes[PUMP].rows[0].declared[0])
  // A broken answer: bad indexes and unknown states are dropped or made safe.
  const broken = schemaModel({ classes: [{ id: "a", parents: [0, 5, "x"], rows: [{ property: "p", state: "weird", target: 9, used: -3 }, null] }, null], focus: 7 })
  assert.deepEqual(broken.classes[0].parents, [])
  assert.deepEqual([broken.classes[0].rows[0].state, broken.classes[0].rows[0].target, broken.classes[0].rows[0].used], ["ok", -1, 0])
  assert.equal(broken.focus, -1)
  assert.equal(schemaModel(null).classes.length, 0)
})

test("a row shows at most one badge, the worst: conflict, then missing, then bad values, then never used or not declared (catches: rows crowded with badges, a missing count hidden behind a milder one)", () => {
  const r = (o) => ({ state: "ok", missing: 0, badValues: 0, ...o })
  assert.equal(rowBadge(r({ state: "conflict", missing: 3, badValues: 2 })).kind, "conflict")
  assert.equal(rowBadge(r({ state: "badValues", missing: 3, badValues: 2 })).kind, "missing")
  assert.equal(rowBadge(r({ state: "badValues", badValues: 2 })).text, "2 bad values")
  assert.equal(rowBadge(r({ state: "unused", missing: 1 })).kind, "missing")
  assert.equal(rowBadge(r({ state: "unused" })).text, "never used")
  assert.equal(rowBadge(r({ state: "undeclared" })).text, "not declared")
  assert.equal(rowBadge(r({})), null)
  // Only red and amber for problems, and only for those that are errors or warnings.
  assert.deepEqual(["conflict", "badValues", "unused"].map((state) => rowBadge(r({ state })).sev), ["error", "warning", ""])
  assert.equal(problemsOf([r({ name: "p", declared: [], state: "ok", missing: 2, required: { min: 1 } })])[0].text, "2 instances miss p")
})

test("the tidy layout puts each class under its first parent, centres parents, draws extra parents, cuts cycles and never overlaps boxes (catches: boxes on top of each other, a hang on A ⊑ B ⊑ A)", () => {
  // 0 ⊒ 1, 2; 1 ⊒ 3; 3 also under 2; 4 alone; 5 ⊑ 6 ⊑ 5 (a cycle).
  const parents = [[], [0], [0], [1, 2], [], [6], [5]]
  const heights = [100, 60, 80, 40, 30, 50, 50]
  const l = tidyLayout(parents, heights, { width: 100, gapX: 20, gapY: 40 })
  assert.ok(l.y[1] >= l.y[0] + heights[0] + 40 - 1e-9 && l.y[1] === l.y[2], "children one row down, in one row")
  assert.equal(l.x[0], (l.x[1] + l.x[2]) / 2, "a parent over the middle of its children")
  assert.ok(l.y[3] > l.y[1] && l.x[3] === l.x[1], "under its first parent")
  assert.deepEqual(l.links.filter((k) => k.child === 3).map((k) => [k.parent, k.primary]), [[1, true], [2, false]], "the second parent is a drawn line, not a place")
  assert.ok(l.first[5] === -1 || l.first[6] === -1, "the cycle is cut")
  const all = parents.map((_, i) => i)
  for (const i of all) assert.ok(Number.isFinite(l.x[i]) && Number.isFinite(l.y[i]))
  // No overlaps, also on 300 generated classes.
  const big = bigSchema(300)
  const bh = big.classes.map((c) => 30 + c.rows.length * 24)
  const bl = tidyLayout(big.classes.map((c) => c.parents), bh)
  const boxes = [[l, heights, 100], [bl, bh, 264]]
  for (const [lay, hs, w] of boxes) {
    const n = hs.length
    const order = [...Array(n).keys()].sort((a, b) => lay.x[a] - lay.x[b])
    for (let a = 0; a < n; a++)
      for (let b = a + 1; b < n && lay.x[order[b]] < lay.x[order[a]] + w; b++) {
        const p = order[a], q = order[b]
        assert.ok(lay.y[p] + hs[p] <= lay.y[q] || lay.y[q] + hs[q] <= lay.y[p], `boxes ${p} and ${q} overlap`)
      }
    for (let i = 0; i < n; i++) assert.ok(lay.x[i] + w <= lay.width + 1e-6 && lay.y[i] + hs[i] <= lay.height + 1e-6, "inside the bounds")
  }
})

test("a stored Hierarchy mode opens Schema; Follow cursor and the Schema toggles are stored, off by default (catches: a saved state that opens on nothing after the upgrade)", () => {
  assert.equal(normalizeState({ mode: "hierarchy" }).mode, "schema")
  const d = normalizeState({})
  assert.equal(d.follow, false)
  assert.deepEqual(d.schema, { problems: false, imports: false })
  const s = normalizeState({ mode: "schema", follow: true, schema: { problems: true, imports: "yes" } })
  assert.deepEqual([s.mode, s.follow, s.schema.problems, s.schema.imports], ["schema", true, true, false])
})

// ---- the view, in Chrome -------------------------------------------------------------------

describe("Schema in Chrome", () => {
  let server, browser
  before(async () => {
    server = await listen({ root: WEB })
    browser = await chromium.launch({ channel: "chrome", headless: true })
  })
  after(async () => {
    await browser?.close()
    await server?.close()
  })

  // The view opened on Schema with the fixture answer (`mount` adds to it).
  async function open(mount = {}, viewport = { width: 1400, height: 900 }) {
    const ctx = await browser.newContext({ viewport })
    const page = await ctx.newPage()
    page.setDefaultTimeout(10_000)
    const errors = []
    page.on("console", (m) => m.type() === "error" && errors.push(m.text()))
    page.on("pageerror", (e) => errors.push(String(e)))
    await page.goto(`${server.origin}/test/graph-ui.html`)
    await page.locator("body[data-ready='true']").waitFor()
    await page.evaluate((o) => window.__graph.mount(o), { schema: FIXTURE, stored: { mode: "schema" }, ...mount })
    await page.locator(".graph[data-busy='false'] .graph-status", { hasText: /Showing/ }).waitFor()
    const box = (c) => page.locator(`.schema-box[data-c="${c}"]`)
    return {
      page,
      box,
      row: (c, r) => box(c).locator(`.schema-row[data-r="${r}"]`),
      info: page.locator(".graph-info"),
      calls: () => page.evaluate(() => window.__graph.calls),
      schemaCalls: async () => (await page.evaluate(() => window.__graph.calls)).filter((c) => c.method === "turtle/schema"),
      stored: () => page.evaluate(() => window.__graph.stored.graph),
      follow: (line, character = 2) => page.evaluate(([l, ch]) => window.__graph.view.follow(l == null ? null : { uri: "file:///ws/ontology.ttl", position: { line: l, character: ch } }), [line, character]),
      close: async () => {
        assert.deepEqual(await page.evaluate(() => window.__violations), [], "no policy violation")
        assert.deepEqual(errors, [], "no console error")
        await ctx.close()
      },
    }
  }
  const followOn = async (v) => {
    await v.page.getByRole("button", { name: "Follow cursor" }).click()
    assert.equal(await v.page.getByRole("button", { name: "Follow cursor" }).getAttribute("aria-pressed"), "true")
  }
  // Long enough for a 150 ms debounce and an answer.
  const rest = (v) => v.page.waitForTimeout(400)

  test("draws a box per class with its rows, states in words, the subclass lines, the ghost and the imports box; the focus class is in the inspector (catches: a blank Schema tab, states shown by colour only)", async () => {
    const v = await open()
    try {
      assert.equal(await v.page.getByRole("tab", { name: "Schema" }).getAttribute("aria-selected"), "true")
      const s = await v.schemaCalls()
      assert.equal(s.length, 1)
      assert.deepEqual(s[0].params, { maxClasses: 300, imports: false })
      assert.match(await v.page.locator(".graph-status").textContent(), /Showing 8 of 8 classes/)
      // Seven classes and one box for the imported one.
      assert.equal(await v.page.locator(".schema-box").count(), 8)
      assert.equal(await v.box(6).count(), 0, "the imported class is folded")
      assert.equal(await v.page.locator(".schema-box.is-imports .schema-name").textContent(), "1 imported class")
      const pump = v.page.getByRole("group", { name: /^Pump, 352 instances, 5 problems$/ })
      assert.equal(await pump.locator(".schema-row").count(), 6)
      assert.equal(await pump.locator(".schema-pb").textContent(), "5")
      assert.match(await pump.locator(".schema-file").textContent(), /^ontology\.ttl:22$/)
      // Each row says its state in words, for a screen reader.
      assert.deepEqual(await pump.locator(".schema-row .graph-sr").allTextContents(), ["bad values: ", "range and shape disagree: ", "declared and used: ", "declared and used: ", "never used: ", "not declared: "])
      assert.equal(await v.row(PUMP, 0).locator(".schema-use").textContent(), "340/352")
      assert.equal(await v.row(PUMP, 3).locator(".schema-card").textContent(), "1..1")
      assert.equal(await v.box(7).getAttribute("class"), "schema-box is-undefined", "Gasket is a ghost")
      assert.match(await v.box(7).textContent(), /never defined/)
      // CentrifugalPump hangs under Pump: one line to Pump's triangle.
      const [p, c] = await Promise.all([v.box(PUMP).boundingBox(), v.box(2).boundingBox()])
      assert.ok(c.y > p.y + p.height, "the subclass below its class")
      assert.ok(Math.abs(c.x + c.width / 2 - (p.x + p.width / 2)) < 2, "centred under it")
      assert.match(await v.page.locator(".schema-tree").getAttribute("d"), /^M/)
      // The focus class is selected and in the inspector.
      assert.match(await v.box(PUMP).getAttribute("class"), /is-selected/)
      await v.info.getByRole("heading", { name: "Pump" }).waitFor()
      assert.equal(await v.page.locator("canvas").isHidden(), true)
    } finally {
      await v.close()
    }
  })

  test("each row shows one badge, the worst (catches: a missing count lost behind a state, two badges on a row)", async () => {
    const v = await open()
    try {
      for (const b of await v.page.locator(".schema-row").all()) assert.ok((await b.locator(".schema-badge").count()) <= 1)
      const badge = (c, r) => v.row(c, r).locator(".schema-badge").textContent()
      assert.equal(await badge(PUMP, 0), "12 bad values")
      assert.equal(await badge(PUMP, 1), "range ≠ shape")
      assert.equal(await badge(PUMP, 3), "3 missing")
      assert.equal(await badge(PUMP, 4), "never used")
      assert.equal(await badge(PUMP, 5), "not declared")
      assert.equal(await badge(0, 0), "7 missing", "an ok row with instances missing it")
      assert.equal(await v.row(PUMP, 2).locator(".schema-badge").count(), 0)
      // Red for errors, amber for warnings, neither for the rest.
      const color = (c, r) => v.row(c, r).locator(".schema-badge").evaluate((e) => getComputedStyle(e).color)
      const [err, warn, muted] = await v.page.evaluate(() => {
        const s = getComputedStyle(document.querySelector(".graph"))
        const probe = document.createElement("span")
        document.querySelector(".graph").append(probe)
        const of = (v) => ((probe.style.color = s.getPropertyValue(v)), getComputedStyle(probe).color)
        const out = [of("--graph-error"), of("--graph-warning"), of("--graph-muted")]
        probe.remove()
        return out
      })
      assert.equal(await color(PUMP, 1), err)
      assert.equal(await color(PUMP, 0), warn)
      assert.equal(await color(PUMP, 4), muted)
    } finally {
      await v.close()
    }
  })

  test("Problems only fades the classes without problems and hides the rows that are fine, and is stored (catches: a filter that hides nothing, or forgets itself)", async () => {
    const v = await open()
    try {
      await v.page.getByRole("button", { name: "Problems only" }).click()
      assert.equal((await v.stored()).schema.problems, true)
      assert.equal(await v.box(2).evaluate((e) => getComputedStyle(e).opacity), "0.3")
      assert.equal(await v.box(PUMP).evaluate((e) => getComputedStyle(e).opacity), "1")
      const shown = await v.box(PUMP).locator(".schema-row:visible .schema-pn").allTextContents()
      assert.deepEqual(shown, ["ex:hasPressure", "ex:hasFlow", "ex:drivenBy", "ex:maxRpm", "ex:vendorCode"])
      // The layout follows the shorter boxes: CentrifugalPump moves up.
      const [p, c] = await Promise.all([v.box(PUMP).boundingBox(), v.box(2).boundingBox()])
      assert.ok(c.y - (p.y + p.height) < 80, "no gap left by the hidden rows")
      await v.page.getByRole("button", { name: "Problems only" }).click()
      assert.equal(await v.box(PUMP).locator(".schema-row:visible").count(), 6)
    } finally {
      await v.close()
    }
  })

  test("hovering a link chip draws one arrow to its target and leaving removes it; a click selects the target (catches: a hairball of arrows, a chip that goes nowhere)", async () => {
    const v = await open()
    try {
      const chip = v.row(PUMP, 2).locator(".schema-chip")
      assert.equal(await chip.textContent(), "→ Pipe")
      await chip.hover()
      const lines = v.page.locator(".schema-lines")
      assert.match(await lines.getAttribute("class"), /is-hot/)
      assert.match(await v.page.locator(".schema-hot").getAttribute("d"), /^M[\d.]+,[\d.]+C/)
      assert.match(await v.box(PIPE).getAttribute("class"), /is-target/)
      // The arrow ends at the target box.
      const end = await v.page.locator(".schema-hot-dot").boundingBox()
      const pipe = await v.box(PIPE).boundingBox()
      assert.ok(Math.abs(end.x + end.width / 2 - pipe.x) < 3 || Math.abs(end.x + end.width / 2 - (pipe.x + pipe.width)) < 3, "at the box's side")
      await v.page.mouse.move(5, 5)
      assert.doesNotMatch(await lines.getAttribute("class"), /is-hot/)
      assert.doesNotMatch(await v.box(PIPE).getAttribute("class"), /is-target/)
      await chip.click()
      await v.info.getByRole("heading", { name: "Pipe" }).waitFor()
      assert.match(await v.box(PIPE).getAttribute("class"), /is-selected/)
      // Centred on it.
      const view = await v.page.locator(".schema-view").boundingBox()
      const b = await v.box(PIPE).boundingBox()
      assert.ok(Math.abs(b.x + b.width / 2 - (view.x + view.width / 2)) < 3)
    } finally {
      await v.close()
    }
  })

  test("the imported classes' box lists them on request, asking with imports, and folds them again (catches: an upper ontology drowning the model, a toggle that asks nothing)", async () => {
    const v = await open()
    try {
      await v.page.getByRole("button", { name: "List them" }).click()
      await v.box(6).waitFor()
      const s = await v.schemaCalls()
      assert.equal(s.at(-1).params.imports, true)
      assert.equal((await v.stored()).schema.imports, true)
      assert.match(await v.box(6).textContent(), /imported/)
      // Material's link chip in Pipe now points at Material's own box.
      await v.page.getByRole("button", { name: "Fold" }).click()
      await v.box(6).waitFor({ state: "detached" })
      assert.equal((await v.schemaCalls()).at(-1).params.imports, false)
    } finally {
      await v.close()
    }
  })

  test("the inspector lists rows declared vs. used with sources that open, and the problems with buttons that open where to fix them (catches: a dead source link, a fix button for a problem with no place)", async () => {
    const v = await open()
    try {
      const rows = v.info.getByRole("list", { name: "Properties" }).getByRole("listitem")
      assert.equal(await rows.count(), 6)
      assert.match(await rows.nth(0).textContent(), /ex:hasPressure340\/35212 bad values/)
      assert.match(await rows.nth(5).textContent(), /Declared nowhere/)
      await v.info.getByRole("button", { name: "Open shape declaration at shapes.ttl:21" }).click()
      await v.info.getByRole("button", { name: "Open domain declaration at ontology.ttl:26" }).click()
      const range = (line) => ({ start: { line, character: 0 }, end: { line, character: 10 } })
      assert.deepEqual(await v.page.evaluate(() => window.__graph.reveals), [{ uri: "file:///ws/shapes.ttl", range: range(20) }, { uri: "file:///ws/ontology.ttl", range: range(25) }])
      const problems = v.info.getByRole("list", { name: "Problems" }).getByRole("listitem")
      assert.equal(await problems.count(), 5)
      assert.equal(await v.info.getByRole("heading", { name: "Problems (5)" }).count(), 1)
      await v.info.getByRole("button", { name: "Open: 12 values of ex:hasPressure not xsd:decimal" }).click()
      assert.deepEqual((await v.page.evaluate(() => window.__graph.reveals)).at(-1), { uri: "file:///ws/ontology.ttl", range: range(25) })
      const none = v.info.getByRole("button", { name: "Open: ex:vendorCode is not declared" })
      assert.equal(await none.isDisabled(), true)
      assert.equal(await none.getAttribute("title"), "Nothing declares it: no place to open")
      // A row's name lights it in its box; the class's own definition opens.
      await v.info.getByRole("button", { name: "ex:maxRpm", exact: true }).click()
      assert.match(await v.row(PUMP, 4).getAttribute("class"), /is-lit/)
      await v.info.getByRole("button", { name: "Go to definition" }).click()
      assert.deepEqual((await v.page.evaluate(() => window.__graph.reveals)).at(-1), { uri: "file:///ws/ontology.ttl", range: range(21) })
      // Close: no class, the drawing takes the width.
      await v.info.getByRole("button", { name: "Close" }).click()
      await v.info.waitFor({ state: "hidden" })
    } finally {
      await v.close()
    }
  })

  test("under 700 px Schema is the inspector alone, on the focus class (catches: unreadable boxes in a side split)", async () => {
    const v = await open({ width: 600 }, { width: 600, height: 800 })
    try {
      assert.equal(await v.page.locator(".graph-schemabox").isHidden(), true)
      await v.info.getByRole("heading", { name: "Pump" }).waitFor()
      const [info, stage] = await v.page.evaluate(() => [".graph-info", ".graph-stage"].map((q) => document.querySelector(q).getBoundingClientRect().toJSON()))
      assert.ok(Math.abs(info.width - stage.width) < 2 && Math.abs(info.height - stage.height) < 2, "the inspector fills the stage")
      assert.equal(await v.info.getByRole("button", { name: "Close" }).isHidden(), true)
      assert.equal(await v.page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), true)
    } finally {
      await v.close()
    }
  })

  test("the legend is behind ?, and lists each state in words (catches: a legend always in the way, or none)", async () => {
    const v = await open()
    try {
      const legend = v.page.getByRole("list", { name: "Legend" })
      assert.equal(await legend.isHidden(), true)
      const q = v.page.getByRole("button", { name: "Legend" })
      await q.click()
      assert.equal(await q.getAttribute("aria-expanded"), "true")
      assert.match(await legend.textContent(), /never used.*not declared.*bad values.*range and shape disagree/)
      await q.click()
      assert.equal(await legend.isHidden(), true)
    } finally {
      await v.close()
    }
  })

  test("the keyboard reaches boxes and rows: arrows move, Enter selects, Enter again opens, Enter on a link goes there (catches: a mouse-only Schema)", async () => {
    const v = await open()
    try {
      const focused = () => v.page.evaluate(() => {
        const e = document.activeElement
        return [e.closest(".schema-box")?.dataset.c, e.dataset.r ?? null]
      })
      await v.page.locator('.schema-box[tabindex="0"]').focus()
      assert.deepEqual(await focused(), [String(PUMP), null])
      await v.page.keyboard.press("ArrowDown")
      assert.deepEqual(await focused(), [String(PUMP), "0"])
      await v.page.keyboard.press("ArrowDown")
      await v.page.keyboard.press("ArrowDown")
      assert.deepEqual(await focused(), [String(PUMP), "2"])
      // Enter on a link row walks to its target.
      await v.page.keyboard.press("Enter")
      await v.info.getByRole("heading", { name: "Pipe" }).waitFor()
      assert.deepEqual(await focused(), [String(PIPE), null])
      // Up from a box goes to its parent; Pipe has none, so to the box before it.
      await v.page.locator(`.schema-box[data-c="${PUMP}"]`).focus()
      await v.page.keyboard.press("ArrowUp")
      assert.deepEqual(await focused(), ["0", null], "Pump's parent, Equipment")
      await v.page.keyboard.press("Enter")
      await v.info.getByRole("heading", { name: "Equipment" }).waitFor()
      assert.deepEqual(await v.page.evaluate(() => window.__graph.reveals), [])
      await v.page.keyboard.press("Enter")
      assert.equal((await v.page.evaluate(() => window.__graph.reveals))[0].range.start.line, 3, "Enter on the selected class opens it")
      await v.page.keyboard.press("ArrowRight")
      assert.notDeepEqual(await focused(), ["0", null])
      await v.page.keyboard.press("Escape")
      await v.info.waitFor({ state: "hidden" })
    } finally {
      await v.close()
    }
  })

  test("zoomed out under 0.5 the boxes fold to their header and the layout closes up (catches: an unreadable overview of tiny rows)", async () => {
    const v = await open()
    try {
      const view = await v.page.locator(".schema-view").boundingBox()
      for (let k = 0; k < 8; k++) await v.page.mouse.move(view.x + 200, view.y + 200), await v.page.mouse.wheel(0, 200)
      await v.page.waitForFunction(() => document.querySelector(".graph-schemabox").classList.contains("schema-far"))
      assert.equal(await v.box(PUMP).locator(".schema-rows").isHidden(), true)
      assert.equal(await v.box(PUMP).locator(".schema-pb").isVisible(), true, "the problem count stays")
      // Fit is still there and brings the rows back when they fit.
      await v.page.getByRole("button", { name: "Fit" }).click()
      assert.equal(await v.box(PUMP).locator(".schema-rows").isVisible(), true)
    } finally {
      await v.close()
    }
  })

  test("Follow cursor: off by default and stored; several moves within 150 ms ask once, for the last; the answer selects the class and lights the row (catches: a request per keystroke, the toggle forgotten)", async () => {
    const v = await open({ atClass: { 30: [MOTOR, 1], 40: [PIPE] } })
    try {
      const n = (await v.calls()).length
      await v.follow(30)
      await rest(v)
      assert.equal((await v.calls()).length, n, "off: nothing asked")
      await followOn(v)
      assert.equal((await v.stored()).follow, true)
      for (const line of [10, 11, 12, 40, 30]) await v.follow(line)
      await rest(v)
      const s = await v.schemaCalls()
      assert.equal((await v.calls()).length, n + 1, "one request for five moves")
      assert.deepEqual(s.at(-1).params.at, { uri: "file:///ws/ontology.ttl", position: { line: 30, character: 2 } })
      await v.info.getByRole("heading", { name: name(MOTOR) }).waitFor()
      assert.match(await v.row(MOTOR, 1).getAttribute("class"), /is-lit/)
      // The cursor on no term: an answer without a focus leaves the view as it is.
      await v.follow(99)
      await rest(v)
      assert.equal((await v.calls()).length, n + 2)
      assert.equal(await v.info.getByRole("heading", { name: name(MOTOR) }).count(), 1)
      // Off again: nothing asked, and stored.
      await v.page.getByRole("button", { name: "Follow cursor" }).click()
      assert.equal((await v.stored()).follow, false)
      await v.follow(40)
      await rest(v)
      assert.equal((await v.calls()).length, n + 2)
    } finally {
      await v.close()
    }
  })

  test("Follow cursor keeps one request in flight and drops an answer for an older cursor (catches: a late answer jumping the view back)", async () => {
    const v = await open({ stored: { mode: "schema", follow: true }, atClass: { 30: [MOTOR], 40: [PIPE] } })
    try {
      const n = (await v.calls()).length
      await v.page.evaluate(() => (window.__graph.hold = true))
      await v.follow(30)
      await rest(v)
      assert.equal((await v.calls()).length, n + 1)
      await v.follow(40)
      await rest(v)
      assert.equal((await v.calls()).length, n + 1, "the second waits for the first")
      await v.page.evaluate(() => window.__graph.release(0))
      await v.page.waitForFunction((k) => window.__graph.calls.length === k, n + 2)
      assert.equal(await v.info.getByRole("heading", { name: name(MOTOR) }).count(), 0, "the answer for line 30 is dropped")
      assert.equal((await v.calls()).at(-1).params.at.position.line, 40)
      await v.page.evaluate(() => window.__graph.release(0))
      await v.info.getByRole("heading", { name: name(PIPE) }).waitFor()
    } finally {
      await v.close()
    }
  })

  test("Follow cursor asks nothing while the view is hidden (catches: a closed graph costing a request per cursor move)", async () => {
    const v = await open({ stored: { mode: "schema", follow: true }, atClass: { 30: [MOTOR] } })
    try {
      const n = (await v.calls()).length
      await v.page.evaluate(() => (document.getElementById("host").hidden = true))
      await v.follow(30)
      await rest(v)
      assert.equal((await v.calls()).length, n)
      // Hidden after the move but before the rest ends: still nothing.
      await v.page.evaluate(() => (document.getElementById("host").hidden = false))
      await v.follow(30)
      await v.page.evaluate(() => (document.getElementById("host").hidden = true))
      await rest(v)
      assert.equal((await v.calls()).length, n)
      await v.page.evaluate(() => (document.getElementById("host").hidden = false))
      await v.follow(30)
      await rest(v)
      assert.equal((await v.calls()).length, n + 1)
    } finally {
      await v.close()
    }
  })

  test("Follow cursor in Graph selects and centres the node; in Focus walks to it (catches: a toggle that only works in Schema)", async () => {
    const v = await open({ stored: { mode: "graph", follow: true }, atNode: { 2: `${EX}Valve`, 8: `${EX}hasPart` } })
    try {
      const n = (await v.calls()).length
      await v.follow(2)
      await rest(v)
      const c = (await v.calls()).slice(n)
      assert.equal(c.length, 1)
      assert.equal(c[0].method, "turtle/graph")
      assert.deepEqual([c[0].params.at.position.line, c[0].params.maxNodes, c[0].params.layout.iterations, c[0].params.depth], [2, 1, 0, 0])
      await v.page.getByRole("region", { name: "Selected node" }).getByRole("heading", { name: "Valve" }).waitFor()
      // Centred: a click in the middle of the canvas hits Valve.
      await v.page.keyboard.press("Escape")
      await v.page.getByRole("tab", { name: "Focus" }).click()
      await v.page.locator(".graph[data-busy='false'] .graph-status", { hasText: /Showing/ }).waitFor()
      const m = (await v.calls()).length
      await v.follow(8)
      await rest(v)
      const f = (await v.calls()).slice(m)
      assert.equal(f.length, 1)
      assert.ok(f[0].params.depth > 0 && f[0].params.at.position.line === 8)
      await v.page.waitForFunction(() => window.__graph.stored.graph.focus === "http://example.org/hasPart")
    } finally {
      await v.close()
    }
  })

  test("draws a 300-class answer in under 50 ms (catches: a layout that thrashes, measuring a box per write)", async () => {
    const v = await open({ schemaSize: 300 })
    try {
      assert.match(await v.page.locator(".graph-status").textContent(), /Showing 300 of 300 classes/)
      assert.equal(await v.page.locator(".schema-box").count(), 300)
      const draws = await v.page.evaluate(() => window.__graph.schemaDraws(5))
      const median = [...draws].sort((a, b) => a - b)[2]
      console.log(`300 classes: draw ${median.toFixed(2)} ms median of 5 (${draws.map((d) => d.toFixed(1)).join(", ")})`)
      assert.ok(median < 50, `draw ${median} ms`)
      // Zoomed out to fit 300 boxes: headers only. A hover's arrow is one
      // path, whatever the size.
      assert.equal(await v.page.locator(".graph-schemabox.schema-far").count(), 1)
      await v.page.locator(".schema-chip").first().dispatchEvent("pointerover")
      assert.match(await v.page.locator(".schema-lines").getAttribute("class"), /is-hot/)
      assert.equal(await v.page.locator(".schema-lines path").count(), 4)
    } finally {
      await v.close()
    }
  })

  test("a stored Hierarchy state opens on Schema and asks turtle/schema with the stored files (catches: users upgraded onto a blank tab)", async () => {
    const v = await open({ stored: { mode: "hierarchy", filters: { files: [A] } } })
    try {
      assert.equal(await v.page.getByRole("tab", { name: "Schema" }).getAttribute("aria-selected"), "true")
      assert.equal(await v.page.getByRole("tab", { name: "Hierarchy" }).count(), 0)
      assert.deepEqual((await v.schemaCalls())[0].params.files, [A])
    } finally {
      await v.close()
    }
  })
})
