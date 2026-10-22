// The Settings page (Ctrl+,), as VS Code's Settings editor: the turtle-lsp
// settings, sent to the server as one `workspace/didChangeConfiguration` with
// the whole `turtle` section, and editor options, applied to every editor and
// model. Values are kept as { key: value } for the keys that differ from the
// default; the caller stores that object with the workspace.

import { esc } from "./dom.js"

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b)
const LABELS = "turtle.inlayHints.labels"

// Settings in the order shown. `type`: bool, number, enum or prefixes.
const SETTINGS = [
  { key: "turtle.format.lineWidth", group: "turtle-lsp", cat: "Format", title: "Line Width", type: "number", def: 100, min: 20, max: 1000, desc: "Line width the formatter aims for." },
  { key: "turtle.shacl.enabled", group: "turtle-lsp", cat: "SHACL", title: "Enabled", type: "bool", def: true, desc: "Validate data against the SHACL shapes in the workspace." },
  { key: "turtle.shacl.onType", group: "turtle-lsp", cat: "SHACL", title: "Validate While Typing", type: "bool", def: true, desc: "Also validate about a second after you stop typing, on the unsaved text. When off, validation runs on save only." },
  { key: "turtle.shacl.maxTriples", group: "turtle-lsp", cat: "SHACL", title: "Max Triples", type: "number", def: 2000000, min: 0, max: Number.MAX_SAFE_INTEGER, desc: "Skip SHACL validation when the workspace holds more triples than this." },
  {
    key: LABELS, group: "turtle-lsp", cat: "Inlay Hints", title: "Labels", type: "enum", def: "opaque",
    options: [["opaque", "Only for terms whose IRI does not say what they are, such as ex:P1234"], ["all", "For every term that has a label"], ["off", "No label hints"]],
    desc: "Which terms get their rdfs:label (or skos:prefLabel) shown as an inlay hint. The Labels button in the status bar (Alt+L) switches between this and all.",
  },
  { key: "turtle.lint.undescribedTerms", group: "turtle-lsp", cat: "Lint", title: "Undescribed Terms", type: "bool", def: true, desc: "Warn about terms in your own namespaces that are used but described nowhere in the workspace." },
  { key: "turtle.prefixes", group: "turtle-lsp", cat: "", title: "Prefixes", type: "prefixes", def: {}, desc: "Prefixes available in every file. A prefixes.ttl in the workspace does the same per folder." },
  { key: "editor.tabSize", group: "Editor", cat: "Editor", title: "Tab Size", type: "number", def: 4, min: 1, max: 16, desc: "The number of spaces a tab is equal to. Overridden by the file's content when Detect Indentation is on." },
  { key: "editor.insertSpaces", group: "Editor", cat: "Editor", title: "Insert Spaces", type: "bool", def: true, desc: "Insert spaces when pressing Tab. Overridden by the file's content when Detect Indentation is on." },
  { key: "editor.detectIndentation", group: "Editor", cat: "Editor", title: "Detect Indentation", type: "bool", def: true, desc: "Set Tab Size and Insert Spaces from each file's content." },
  {
    key: "editor.wordWrap", group: "Editor", cat: "Editor", title: "Word Wrap", type: "enum", def: "off",
    options: [["off", "Lines never wrap"], ["on", "Lines wrap at the viewport width"], ["wordWrapColumn", "Lines wrap at column 80"], ["bounded", "Lines wrap at the smaller of the viewport width and column 80"]],
    desc: "How lines wrap.",
  },
  { key: "editor.minimap.enabled", group: "Editor", cat: "Minimap", title: "Enabled", type: "bool", def: true, desc: "Show the minimap." },
  { key: "editor.fontSize", group: "Editor", cat: "Editor", title: "Font Size", type: "number", def: 13, min: 6, max: 100, desc: "The font size in pixels." },
  { key: "editor.vimMode", group: "Editor", cat: "Editor", title: "Vim Mode", type: "bool", def: false, desc: "Edit with Vim keys. The mode and the : command line show in the status bar; :w saves, :wa saves all, :q closes the tab, :e opens a file." },
  {
    key: "workbench.colorTheme", group: "Workbench", cat: "Appearance", title: "Color Theme", type: "enum", def: "dark",
    options: [["dark", "Dark"], ["light", "Light"], ["auto", "Follow the system's light or dark setting"]],
    desc: "The colours of the page and the editors.",
  },
]
const BY_KEY = new Map(SETTINGS.map((s) => [s.key, s]))
const label = (s) => (s.cat ? `${s.cat}: ${s.title}` : s.title)

function valid(s, v) {
  if (s.type === "bool") return typeof v === "boolean"
  if (s.type === "number") return Number.isInteger(v) && v >= s.min && v <= s.max
  if (s.type === "enum") return s.options.some(([o]) => o === v)
  return v && typeof v === "object" && !Array.isArray(v) && Object.values(v).every((x) => typeof x === "string")
}

/**
 * Build the Settings page and apply the saved settings at once.
 * @param {object} o
 * @param {object} o.monaco
 * @param {{ notify(method: string, params: object): void }} o.client the language server
 * @param {HTMLElement} o.mount the element the page covers (the editor area)
 * @param {HTMLElement} [o.opener] a button that opens the page
 * @param {object} [o.saved] the object `values()` returned in an earlier visit
 * @param {() => void} o.onChange called after every change (to keep the values and redraw the status bar)
 * @param {() => void} [o.onClose] called when the page closes (to give the focus back)
 * @param {Record<string, (value: any) => void>} [o.effects] per key, called with its value at once and after every change
 * @returns {{ open(): void, close(): void, isOpen(): boolean, values(): object, get(key: string): any,
 *   set(key: string, value: any): void, replace(saved: object): void, toggleLabels(): void, labelsText(): string, applyTo(editor: object): void }}
 */
export function createSettings({ monaco, client, mount, opener, saved = {}, onChange, onClose, effects = {} }) {
  const vals = {}
  const load = (saved) => {
    for (const k of Object.keys(vals)) delete vals[k]
    for (const [k, v] of Object.entries(saved ?? {})) if (BY_KEY.has(k) && valid(BY_KEY.get(k), v) && !same(v, BY_KEY.get(k).def)) vals[k] = v
  }
  load(saved)
  const get = (key) => (key in vals ? vals[key] : BY_KEY.get(key).def)
  let otherLabels = get(LABELS) === "all" ? "opaque" : get(LABELS) // the mode the status-bar button goes back to
  let prefixRows = Object.entries(get("turtle.prefixes")) // rows being edited, complete or not

  // ---- applying ----

  const sendTurtle = () =>
    client.notify("workspace/didChangeConfiguration", {
      settings: {
        turtle: {
          prefixes: get("turtle.prefixes"),
          inlayHints: { labels: get(LABELS) },
          shacl: { enabled: get("turtle.shacl.enabled"), onType: get("turtle.shacl.onType"), maxTriples: get("turtle.shacl.maxTriples") },
          lint: { undescribedTerms: get("turtle.lint.undescribedTerms") },
          format: { lineWidth: get("turtle.format.lineWidth") },
        },
      },
    })
  const editorOptions = () => ({
    fontSize: get("editor.fontSize"),
    wordWrap: get("editor.wordWrap"),
    minimap: { enabled: get("editor.minimap.enabled") },
    tabSize: get("editor.tabSize"),
    insertSpaces: get("editor.insertSpaces"),
    detectIndentation: get("editor.detectIndentation"),
  })
  const applyTo = (ed) => ed.updateOptions(editorOptions())
  const applyModel = (m) => {
    if (get("editor.detectIndentation")) m.detectIndentation(get("editor.insertSpaces"), get("editor.tabSize"))
    else m.updateOptions({ tabSize: get("editor.tabSize"), insertSpaces: get("editor.insertSpaces") })
  }
  const applyEditors = () => {
    for (const ed of monaco.editor.getEditors()) applyTo(ed)
    for (const m of monaco.editor.getModels()) applyModel(m)
  }
  const applyEffects = () => {
    for (const [k, fn] of Object.entries(effects)) fn(get(k))
  }
  monaco.editor.onDidCreateModel(applyModel)
  applyEditors()
  sendTurtle()
  applyEffects()

  function set(key, value) {
    const s = BY_KEY.get(key)
    if (!valid(s, value) || same(value, get(key))) return
    if (same(value, s.def)) delete vals[key]
    else vals[key] = value
    if (key === LABELS && value !== "all") otherLabels = value
    if (key.startsWith("turtle.")) sendTurtle()
    else if (effects[key]) effects[key](value)
    else applyEditors()
    if (!page.hidden) markModified()
    onChange()
  }

  // Every value replaced with `saved` (what `values()` returned, in this tab
  // or another) and applied at once. Not a change: `onChange` is not called.
  function replace(saved) {
    load(saved)
    otherLabels = get(LABELS) === "all" ? "opaque" : get(LABELS)
    prefixRows = Object.entries(get("turtle.prefixes"))
    applyEditors()
    sendTurtle()
    applyEffects()
    if (!page.hidden) render()
  }

  // ---- the page ----

  const page = document.createElement("section")
  page.className = "stg"
  page.hidden = true
  page.setAttribute("aria-label", "Settings")
  page.innerHTML = `
    <div class="stg-hd">
      <div class="stg-search"><span class="codicon codicon-search"></span><input type="search" aria-label="Search settings" placeholder="Search settings" autocomplete="off" spellcheck="false"></div>
      <button class="stg-btn" data-act="reset-all" title="Reset every setting to its default">Reset All</button>
      <button class="stg-x" data-act="close" aria-label="Close Settings" title="Close (Escape)"><span class="codicon codicon-close"></span></button>
    </div>
    <div class="stg-count" aria-live="polite"></div>
    <div class="stg-body"></div>`
  mount.append(page)
  const search = page.querySelector(".stg-search input")
  const body = page.querySelector(".stg-body")
  const count = page.querySelector(".stg-count")

  const control = (s) => {
    const v = get(s.key)
    const id = `stg-${s.key.replace(/\W/g, "-")}`
    if (s.type === "bool") return `<label class="stg-check"><input type="checkbox" id="${id}" aria-label="${esc(label(s))}"${v ? " checked" : ""}><span>${esc(s.desc)}</span></label>`
    const desc = `<div class="stg-desc">${esc(s.desc)}</div>`
    if (s.type === "number") return `${desc}<input class="stg-num" type="number" id="${id}" aria-label="${esc(label(s))}" min="${s.min}" max="${s.max}" step="1" value="${v}">`
    if (s.type === "enum") {
      const opts = s.options.map(([o, d]) => `<option value="${esc(o)}" title="${esc(d)}"${o === v ? " selected" : ""}>${esc(o)}</option>`).join("")
      return `${desc}<select id="${id}" aria-label="${esc(label(s))}">${opts}</select><div class="stg-opt-desc">${esc(s.options.find(([o]) => o === v)[1])}</div>`
    }
    return `${desc}<div class="stg-prefixes" role="table" aria-label="Prefixes">${prefixHtml()}</div><button class="stg-btn" data-act="add-prefix">Add Prefix</button>`
  }
  const prefixHtml = () =>
    (prefixRows.length ? `<div class="stg-prow stg-phd" role="row"><span role="columnheader">Prefix</span><span role="columnheader">Namespace</span><span></span></div>` : "") +
    prefixRows
      .map(
        ([n, iri], i) =>
          `<div class="stg-prow" role="row" data-i="${i}"><input class="stg-pname" aria-label="Prefix name" placeholder="ex" value="${esc(n)}" spellcheck="false"><input class="stg-piri" aria-label="Namespace IRI" placeholder="http://example.org/" value="${esc(iri)}" spellcheck="false"><button class="stg-x" data-act="remove-prefix" aria-label="Remove Prefix ${esc(n)}" title="Remove"><span class="codicon codicon-trash"></span></button></div>`
      )
      .join("")
  const prefixValue = () => Object.fromEntries(prefixRows.map(([n, iri]) => [n.trim().replace(/:$/, ""), iri.trim()]).filter(([n, iri]) => iri && !/[\s:]/.test(n)))

  function render() {
    let html = ""
    for (const group of ["turtle-lsp", "Editor", "Workbench"]) {
      html += `<h2 class="stg-group" data-group="${esc(group)}">${esc(group)}</h2>`
      for (const s of SETTINGS.filter((x) => x.group === group)) {
        html += `<div class="stg-item" data-key="${esc(s.key)}" data-group="${esc(group)}">
          <div class="stg-title"><button class="stg-reset" data-act="reset" aria-label="Reset ${esc(label(s))}" title="Reset Setting"><span class="codicon codicon-discard"></span></button>${s.cat ? `<span class="stg-cat">${esc(s.cat)}: </span>` : ""}<b>${esc(s.title)}</b><span class="stg-key">${esc(s.key)}</span></div>
          ${control(s)}</div>`
      }
    }
    body.innerHTML = html + `<p class="stg-none" hidden>No settings found.</p>`
    markModified()
    filter()
  }
  function markModified() {
    for (const item of body.querySelectorAll(".stg-item")) item.classList.toggle("modified", item.dataset.key in vals)
  }
  function filter() {
    const words = search.value.toLowerCase().split(/\s+/).filter(Boolean)
    let shown = 0
    for (const item of body.querySelectorAll(".stg-item")) {
      const s = BY_KEY.get(item.dataset.key)
      const hay = `${s.group} ${s.cat} ${s.title} ${s.key} ${s.desc}`.toLowerCase()
      item.hidden = !words.every((w) => hay.includes(w))
      if (!item.hidden) shown++
    }
    for (const h of body.querySelectorAll(".stg-group")) h.hidden = !body.querySelector(`.stg-item[data-group="${h.dataset.group}"]:not([hidden])`)
    body.querySelector(".stg-none").hidden = shown > 0
    count.textContent = words.length ? `${shown} Setting${shown === 1 ? "" : "s"} Found` : ""
  }

  const keyOf = (el) => el.closest(".stg-item")?.dataset.key
  body.addEventListener("input", (e) => {
    const t = e.target
    if (t.classList.contains("stg-pname") || t.classList.contains("stg-piri")) {
      const row = t.closest(".stg-prow")
      prefixRows[+row.dataset.i][t.classList.contains("stg-pname") ? 0 : 1] = t.value
      row.querySelector("[data-act]").setAttribute("aria-label", `Remove Prefix ${prefixRows[+row.dataset.i][0]}`.trim())
      return set("turtle.prefixes", prefixValue())
    }
    const key = keyOf(t)
    if (!key) return
    const s = BY_KEY.get(key)
    if (s.type === "number") {
      const n = Number(t.value)
      const ok = t.value !== "" && valid(s, n)
      t.classList.toggle("bad", !ok)
      if (ok) set(key, n)
    } else if (s.type === "bool") set(key, t.checked)
    else if (s.type === "enum") {
      set(key, t.value)
      t.nextElementSibling.textContent = s.options.find(([o]) => o === t.value)[1]
    }
  })
  page.addEventListener("click", (e) => {
    const b = e.target.closest("[data-act]")
    if (!b) return
    const act = b.dataset.act
    if (act === "close") close()
    else if (act === "reset-all") {
      for (const k of Object.keys(vals)) delete vals[k]
      otherLabels = "opaque"
      prefixRows = []
      sendTurtle()
      applyEditors()
      applyEffects()
      onChange()
      render()
    } else if (act === "reset") {
      const key = keyOf(b)
      if (key === "turtle.prefixes") prefixRows = []
      set(key, BY_KEY.get(key).def)
      render()
    } else if (act === "add-prefix") {
      prefixRows.push(["", ""])
      page.querySelector(".stg-prefixes").innerHTML = prefixHtml()
      page.querySelector(".stg-prow:last-child .stg-pname").focus()
    } else if (act === "remove-prefix") {
      prefixRows.splice(+b.closest(".stg-prow").dataset.i, 1)
      page.querySelector(".stg-prefixes").innerHTML = prefixHtml()
      set("turtle.prefixes", prefixValue())
    }
  })
  search.addEventListener("input", filter)
  page.addEventListener("keydown", (e) => {
    if (e.key !== "Escape") return
    if (e.target === search && search.value) (search.value = ""), filter()
    else close()
    e.stopPropagation()
  })
  document.addEventListener("keydown", (e) => {
    if ((e.ctrlKey || e.metaKey) && !e.shiftKey && !e.altKey && e.key === ",") (e.preventDefault(), open())
  })
  opener?.addEventListener("click", () => (page.hidden ? open() : close()))

  function open() {
    if (page.hidden) {
      prefixRows = Object.entries(get("turtle.prefixes"))
      render()
      page.hidden = false
      opener?.classList.add("on")
    }
    search.focus()
    search.select()
  }
  function close() {
    if (page.hidden) return
    page.hidden = true
    opener?.classList.remove("on")
    onClose ? onClose() : monaco.editor.getEditors()[0]?.focus()
  }

  return {
    open,
    close,
    isOpen: () => !page.hidden,
    values: () => structuredClone(vals),
    get,
    set,
    replace,
    applyTo,
    toggleLabels() {
      set(LABELS, get(LABELS) === "all" ? otherLabels : "all")
      const select = body.querySelector(`[data-key="${LABELS}"] select`)
      if (select) (select.value = get(LABELS)), (select.nextElementSibling.textContent = BY_KEY.get(LABELS).options.find(([o]) => o === select.value)[1])
    },
    labelsText: () => `Labels: ${{ opaque: "Opaque", all: "All", off: "Off" }[get(LABELS)]}`,
  }
}
