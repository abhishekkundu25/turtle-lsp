// The RDF Editor: Monaco with turtle-lsp (in a worker) over a workspace of
// files and folders kept in this browser. VS Code's arrangement: explorer,
// tabs, editor, Problems, status bar, command palette (F1).
import { WORKSPACE } from "./samples.js"
import { LspClient, connect, markers, registerSyntax, languageOf, quietJson } from "./editor-lsp.js"
import { zip } from "./zip.js"
import { createWorkspaceStore, claim } from "./workspace-store.js"
import { $, esc } from "./dom.js"
import { dirOf, nameOf, join, under, uriOf } from "./paths.js"
import { LIMITS, picked, walkDropped, walkHandle, admit, limitsNote, readTextCapped } from "./files.js"
import { toMonacoRange, fromMonacoRange } from "./text.js"
import { createSearch } from "./search.js"
import { createSettings } from "./settings.js"
import { createTheme } from "./theme.js"
import { createVim } from "./vim.js"
import { createHierarchy } from "./hierarchy.js"
import { createDiff } from "./diff.js"
import { createQueryPane, isQuery } from "./query-pane.js"

const SAVE_DELAY_MS = 300
// A fetched ontology larger than this is refused.
const MAX_FETCH_BYTES = LIMITS.fileBytes
const FETCH_TIMEOUT_MS = 30_000
const ICON = { turtle: "TTL", trig: "TRIG", ntriples: "NT", nquads: "NQ", sparql: "RQ", json: "LD" }

// ---- state --------------------------------------------------------------------------

const W = {
  // path -> { model, disk, saved }. `disk` is the file's text on disk for a
  // file in a folder opened in place (null: not on disk yet); undefined
  // otherwise. `saved`: the model's alternative version id when its text was
  // the disk text (NaN: none known).
  files: new Map(),
  folders: new Set(), // folders kept even when empty
  expanded: new Set(),
  selected: null, // the tree's selection (a file or folder path)
  // Folders opened in place: top folder path -> directory handle. The path
  // is the folder's name, made unique ("data (2)") when the workspace has it.
  roots: new Map(),
  // Editor groups (the split editor), each { tabs, active, views, editor, el };
  // `g` is the focused one, which W.tabs and W.active stand for.
  groups: [],
  g: 0,
  diags: new Map(), // uri -> LSP diagnostics
  tally: new Map(), // uri -> { e, w }: its errors and warnings
  filter: "all",
  panel: true,
  editing: null, // { mode: "newFile" | "newFolder" | "rename", dir, path }
  folded: new Set(), // files whose problems are folded in the Problems panel
  readOnly: false, // another tab owns the workspace: this one shows it and writes nothing
}
for (const key of ["tabs", "active"]) {
  Object.defineProperty(W, key, { get: () => W.groups[W.g][key], set: (v) => (W.groups[W.g][key] = v) })
}
let monaco, editor, client, hooks, search, settings, hierarchy
let diff, theme, vim, queries
const pathByUri = new Map()

// What the browser keeps of the workspace (workspace-store.js): the files
// touched since the last write, and the layout when it changed. A cursor
// move or a diagnostics publish is neither.
const ws = createWorkspaceStore({
  delay: SAVE_DELAY_MS,
  source: {
    file(path) {
      const f = W.files.get(path)
      return f ? { text: f.model.getValue(), disk: f.disk } : null
    },
    layout: () => ({
      folders: [...W.folders],
      expanded: [...W.expanded],
      roots: [...W.roots].map(([path, handle]) => ({ path, handle })),
      groups: W.groups.map((g) => ({ tabs: g.tabs, active: g.active })),
      focused: W.g,
      settings: settings.values(),
    }),
  },
  // `body[data-saved]`: "true" once everything is written, "false" while a
  // change waits or is being written, "error" when storage refuses, "readonly"
  // in a tab that does not own the workspace.
  onState(state) {
    document.body.dataset.saved = state === "clean" ? "true" : state === "pending" || state === "saving" ? "false" : state
    if (state === "clean") $("status-storage").textContent = "Kept in this browser"
    else if (state === "error") $("status-storage").textContent = "Not kept: this browser refuses storage"
    else if (state === "readonly") $("status-storage").textContent = "Read-only: open in another tab"
  },
  onConflict: (paths) => toast(`${paths.length} file${paths.length === 1 ? " was" : "s were"} changed in another tab and not saved here: ${paths.join(", ")}`),
})
let restoring = false // the workspace is being read back: nothing to write
const touch = (path) => restoring || ws.touch(path)
const touchLayout = () => restoring || ws.touchLayout()

// A change to the workspace in a tab that does not own it is refused.
function writable() {
  if (!W.readOnly) return true
  toast("This workspace is open in another tab, so this tab is read-only. Close the other tab to edit here.")
  return false
}

// The folder opened in place that `path` is in, if any.
function rootOf(path) {
  for (const [root, handle] of W.roots) if (under(path, root)) return { root, handle }
  return null
}
// Whether `path` has changes not on disk (only files opened in place can).
// Undoing back to the saved text makes the file clean again.
function dirty(path) {
  const f = W.files.get(path)
  return Boolean(f && f.disk !== undefined && (f.disk === null || f.model.getAlternativeVersionId() !== f.saved))
}
// `f`'s text on disk is now `disk`; `version`: the alternative version id the
// model had with that text, if known.
function setDisk(f, disk, version) {
  f.disk = disk
  if (version === undefined) version = disk !== null && f.model.getValue() === disk ? f.model.getAlternativeVersionId() : NaN
  f.saved = version
  touch(f.path)
}

// ---- the workspace ---------------------------------------------------------------------

function folderList() {
  const all = new Set(W.folders)
  for (const p of W.files.keys()) for (let d = dirOf(p); d; d = dirOf(d)) all.add(d)
  return all
}

// `disk`: the text on disk, for a file in a folder opened in place.
function addFile(path, text, { disk } = {}) {
  const existing = W.files.get(path)
  if (existing) {
    // Importing a file again replaces its text (undoable).
    existing.model.pushEditOperations([], [{ range: existing.model.getFullModelRange(), text }], () => null)
    if (disk !== undefined) setDisk(existing, disk)
    return
  }
  const uri = monaco.Uri.parse(uriOf(path))
  const model = monaco.editor.createModel(text, languageOf(path), uri)
  const key = model.uri.toString()
  pathByUri.set(key, path)
  // A file created inside a folder opened in place is not on disk yet.
  const f = { model, path }
  setDisk(f, disk !== undefined ? disk : rootOf(path) ? null : undefined)
  W.files.set(path, f)
  // Each change goes as its range and text (Monaco orders them from the end
  // of the document, so they apply in sequence).
  model.onDidChangeContent((e) => {
    const contentChanges = e.changes.map((c) => ({ range: fromMonacoRange(c.range), text: c.text }))
    client.notify("textDocument/didChange", { textDocument: { uri: key, version: model.getVersionId() }, contentChanges })
    touch(path)
    changed({ tabs: true })
  })
  // The model's text, which the server's copy must equal: Monaco may have
  // normalised the line endings of `text`.
  client.notify("textDocument/didOpen", { textDocument: { uri: key, languageId: languageOf(path), version: model.getVersionId(), text: model.getValue() } })
  touch(path)
}

// `keepGroups`: a group left without tabs stays (a move puts the tab back).
function removeFile(path, { keepGroups = false } = {}) {
  const f = W.files.get(path)
  if (!f) return
  const key = f.model.uri.toString()
  client.notify("textDocument/didClose", { textDocument: { uri: key } })
  W.diags.delete(key)
  W.tally.delete(key)
  pathByUri.delete(key)
  f.model.dispose()
  W.files.delete(path)
  touch(path)
  for (let i = W.groups.length - 1; i >= 0; i--) {
    W.groups[i].views.delete(path)
    closeTab(path, { group: i, quiet: true, force: true, keepGroup: keepGroups })
  }
}

// Move every file (and kept folder) at or under `from` to `to`.
// A file moved within a folder opened in place is new on disk at its new
// path (saving writes it there; the old file stays). Each editor group keeps
// its tabs, in place, and its active file, even a group showing only moved
// files.
function movePath(from, to) {
  const reopen = new Map() // group -> its active file's new path
  for (const [p, f] of [...W.files]) {
    if (!under(p, from)) continue
    const dest = to + p.slice(from.length)
    const text = f.model.getValue()
    const was = W.groups.map((g) => ({ g, tab: g.tabs.indexOf(p), active: g.active === p }))
    removeFile(p, { keepGroups: true })
    addFile(dest, text)
    for (const { g, tab, active } of was) {
      if (tab >= 0) g.tabs.splice(Math.min(tab, g.tabs.length), 0, dest)
      if (active) reopen.set(g, dest)
    }
  }
  const focused = W.g
  for (const [g, dest] of reopen) (focusGroup(W.groups.indexOf(g)), openTab(dest, { focus: false }))
  focusGroup(focused)
  for (const d of [...W.folders]) if (under(d, from)) (W.folders.delete(d), W.folders.add(to + d.slice(from.length)))
  // A folder opened in place keeps its disk folder under its new name.
  for (const [r, handle] of [...W.roots]) if (under(r, from)) (W.roots.delete(r), W.roots.set(to + r.slice(from.length), handle))
  for (const d of [...W.expanded]) if (under(d, from)) (W.expanded.delete(d), W.expanded.add(to + d.slice(from.length)))
  if (W.selected && under(W.selected, from)) W.selected = to + W.selected.slice(from.length)
}

// Drag and drop within the explorer: `path` into folder `dir`.
function moveInto(path, dir) {
  if (!writable()) return
  const dest = join(dir, nameOf(path))
  if (dest === path || under(dir, path)) return
  if (W.files.has(dest) || folderList().has(dest)) return toast(`${dest} already exists.`)
  const folder = !W.files.has(path)
  if (folder) W.folders.add(dest)
  movePath(path, dest)
  if (folder) W.folders.delete(path)
  if (dir) W.expanded.add(dir)
  changed({ tree: true, tabs: true, layout: true })
}

function deletePath(path) {
  if (!writable()) return
  const removed = [...W.files].filter(([p]) => under(p, path)).map(([p, f]) => ({ path: p, text: f.model.getValue() }))
  const folders = [...W.folders].filter((d) => under(d, path))
  const roots = [...W.roots].filter(([r]) => under(r, path))
  for (const { path: p } of removed) removeFile(p)
  for (const d of folders) W.folders.delete(d)
  for (const [r] of roots) W.roots.delete(r)
  if (W.selected && under(W.selected, path)) W.selected = null
  changed({ tree: true, tabs: true, layout: true })
  toast(`Deleted ${nameOf(path)}`, "Undo", () => {
    for (const d of folders) W.folders.add(d)
    for (const [r, handle] of roots) W.roots.set(r, handle)
    for (const f of removed) addFile(f.path, f.text)
    changed({ tree: true, layout: true })
  })
}

// ---- rendering ------------------------------------------------------------------------------

let frame = null
const due = { tree: false, tabs: false, problems: false, status: false }
// Something changed: redraw what it affects on the next frame. `layout`: the
// folders, tabs or settings changed, which the browser keeps.
function changed(what = {}) {
  if (what.layout) touchLayout()
  for (const k of Object.keys(due)) due[k] ||= Boolean(what[k])
  if (!frame) {
    frame = requestAnimationFrame(() => {
      frame = null
      if (due.tree) renderTree()
      if (due.tabs) renderTabs()
      if (due.problems) renderProblems()
      if (due.status) renderStatus()
      for (const k of Object.keys(due)) due[k] = false
    })
  }
}

const NONE = { e: 0, w: 0 }
function counts(path) {
  const f = W.files.get(path)
  return (f && W.tally.get(f.model.uri.toString())) ?? NONE
}

function renderTree() {
  const folders = folderList()
  const children = (dir) => {
    const subs = [...folders].filter((d) => dirOf(d) === dir).sort((a, b) => a.localeCompare(b))
    const files = [...W.files.keys()].filter((p) => dirOf(p) === dir).sort((a, b) => a.localeCompare(b))
    return { subs, files }
  }
  // A render during an edit (diagnostics arrived, say) keeps what was typed.
  const typed = $("tree").querySelector(".tree-input")?.value
  const input = (depth, value) =>
    `<div class="row" style="padding-left:${8 + depth * 12}px"><span class="tw"></span><input class="tree-input" value="${esc(typed ?? value)}" spellcheck="false" aria-label="Name"></div>`
  // Each folder's errors, the files under it summed once.
  const folderErrs = new Map()
  for (const p of W.files.keys()) {
    const { e } = counts(p)
    if (e) for (let d = dirOf(p); d; d = dirOf(d)) folderErrs.set(d, (folderErrs.get(d) ?? 0) + e)
  }
  let html = ""
  const walk = (dir, depth) => {
    const { subs, files } = children(dir)
    const e = W.editing
    if (e && e.mode !== "rename" && e.dir === dir) html += input(depth, "")
    for (const d of subs) {
      if (e?.mode === "rename" && e.path === d) {
        html += input(depth, nameOf(d))
      } else {
        const open = W.expanded.has(d)
        const errs = folderErrs.get(d) ?? 0
        html += `<div class="row${W.selected === d ? " sel" : ""}${errs ? " has-e" : ""}" role="treeitem" draggable="true" data-path="${esc(d)}" aria-level="${depth + 1}" aria-expanded="${open}" aria-selected="${W.selected === d}" style="padding-left:${8 + depth * 12}px"><span class="tw codicon codicon-chevron-${open ? "down" : "right"}"></span><span class="nm">${esc(nameOf(d))}</span>${errs ? `<span class="dec e">●</span>` : ""}</div>`
      }
      if (W.expanded.has(d)) walk(d, depth + 1)
    }
    for (const p of files) {
      if (e?.mode === "rename" && e.path === p) {
        html += input(depth, nameOf(p))
        continue
      }
      const c = counts(p)
      const cls = c.e ? " has-e" : c.w ? " has-w" : ""
      const dec = c.e ? `<span class="dec e">${c.e}</span>` : c.w ? `<span class="dec w">${c.w}</span>` : ""
      html += `<div class="row${W.selected === p ? " sel" : ""}${cls}" role="treeitem" draggable="true" data-path="${esc(p)}" aria-level="${depth + 1}" aria-selected="${W.selected === p}" style="padding-left:${8 + depth * 12}px"><span class="tw"></span><span class="fi">${ICON[languageOf(p)]}</span><span class="nm">${esc(nameOf(p))}</span>${dec}</div>`
    }
  }
  walk("", 0)
  if (!W.files.size && !folders.size && !W.editing) html = `<p class="empty" style="padding:8px 20px;color:var(--muted)">No files. Import files or a folder, or drop them here.</p>`
  $("tree").innerHTML = html
  const box = $("tree").querySelector(".tree-input")
  if (box) {
    box.focus()
    const dot = box.value.lastIndexOf(".")
    if (typed === undefined) box.setSelectionRange(0, dot > 0 ? dot : box.value.length)
    else box.setSelectionRange(box.value.length, box.value.length)
  }
}

function renderTabs() {
  W.groups.forEach((g, i) => {
    const q = (sel) => g.el.querySelector(sel)
    q(".tabs").innerHTML = g.tabs
      .map((p) => {
        const c = counts(p)
        return `<div class="tab${dirty(p) ? " dirty" : ""}" role="tab" aria-label="${esc(nameOf(p))}" aria-selected="${g.active === p}" data-path="${esc(p)}" title="${esc(p)}"><span class="${c.e ? "e" : ""}">${esc(nameOf(p))}</span><span class="x" data-close="${esc(p)}" aria-hidden="true"><span class="codicon codicon-close"></span></span></div>`
      })
      .join("")
    q(".welcome").hidden = Boolean(g.active)
    q(".crumbs-path").innerHTML = g.active ? g.active.split("/").map(esc).join(` <span class="codicon codicon-chevron-right"></span> `) : ""
    g.el.classList.toggle("focused", i === W.g && W.groups.length > 1)
    g.isQuery.set(isQuery(g.active))
    queries.update(g)
  })
}

function renderProblems() {
  const keep = {
    all: () => true,
    errors: (d) => d.severity === 1,
    warnings: (d) => d.severity === 2,
    shacl: (d) => d.code === "shacl-violation",
  }[W.filter]
  const icon = { 1: "error", 2: "warning", 3: "info", 4: "lightbulb" }
  let html = ""
  let total = 0
  for (const p of [...W.files.keys()].sort((a, b) => a.localeCompare(b))) {
    const list = (W.diags.get(W.files.get(p).model.uri.toString()) ?? []).filter(keep)
    total += (W.diags.get(W.files.get(p).model.uri.toString()) ?? []).length
    if (!list.length) continue
    const open = !W.folded.has(p)
    html += `<div class="problem-file" data-fold="${esc(p)}" aria-expanded="${open}"><span class="codicon codicon-chevron-${open ? "down" : "right"}"></span>${esc(nameOf(p))} <span class="path">${esc(p)}</span><span class="badge">${list.length}</span></div>`
    if (!open) continue
    list.forEach((d, i) => {
      html += `<div class="problem" data-file="${esc(p)}" data-i="${i}"><span class="codicon codicon-${icon[d.severity ?? 1]}"></span><span class="msg">${esc(d.message)}</span><span class="src">${esc(d.code ?? "")} [Ln ${d.range.start.line + 1}, Col ${d.range.start.character + 1}]</span></div>`
    })
  }
  $("problems").innerHTML = html || `<p class="empty">No problems${W.filter === "all" ? " have been detected in the workspace" : " of this kind"}.</p>`
  $("problems-count").textContent = total
  $("problems").dataset.filter = W.filter
}

function renderStatus() {
  let e = 0, w = 0, s = 0
  for (const list of W.diags.values()) {
    for (const d of list) {
      if (d.severity === 1) e++
      if (d.severity === 2) w++
      if (d.code === "shacl-violation") s++
    }
  }
  $("status-errors").textContent = e
  $("status-warnings").textContent = w
  $("status-shacl").textContent = s
  $("status-labels").textContent = settings.labelsText()
  $("status-lang").textContent = W.active ? { turtle: "Turtle", trig: "TriG", ntriples: "N-Triples", nquads: "N-Quads", sparql: "SPARQL", json: "JSON-LD" }[languageOf(W.active)] : ""
  const pos = W.active && editor.getPosition()
  $("status-pos").textContent = pos ? `Ln ${pos.lineNumber}, Col ${pos.column}` : ""
}

// ---- tabs and the editor ----------------------------------------------------------------------

// Open `path` in the focused group.
function openTab(path, { focus = true } = {}) {
  const f = W.files.get(path)
  if (!f) return
  const g = W.groups[W.g]
  if (g.active && W.files.has(g.active)) g.views.set(g.active, editor.saveViewState())
  if (!g.tabs.includes(path)) g.tabs.push(path)
  g.active = path
  W.selected = path
  for (let d = dirOf(path); d; d = dirOf(d)) W.expanded.add(d)
  editor.setModel(f.model)
  const view = g.views.get(path)
  if (view) editor.restoreViewState(view)
  if (focus) editor.focus()
  changed({ tree: true, tabs: true, status: true, layout: true })
  if (!$("view-outline").hidden) renderOutline()
}

// Close `path` in group `group` (the focused one by default). A file with
// unsaved changes shown in no other group asks first, unless `force`.
async function closeTab(path, { group = W.g, quiet = false, force = false, keepGroup = false } = {}) {
  const g = W.groups[group]
  const i = g?.tabs.indexOf(path) ?? -1
  if (i < 0) return
  const elsewhere = W.groups.some((o, k) => k !== group && o.tabs.includes(path))
  if (!force && dirty(path) && !elsewhere) {
    const answer = await ask(`Do you want to save the changes you made to ${nameOf(path)}?`, "Your changes will be lost if you don't save them.", ["Save", "Don't Save", "Cancel"])
    if (answer === "Cancel" || answer == null) return
    if (answer === "Save" && !(await save(path))) return
    if (answer === "Don't Save") await discard(path)
  }
  if (!g.tabs.includes(path)) return
  g.tabs.splice(g.tabs.indexOf(path), 1)
  g.views.delete(path)
  if (!g.tabs.length && W.groups.length > 1 && !keepGroup) {
    closeGroup(group)
  } else if (g.active === path) {
    g.active = null
    const next = g.tabs[Math.min(i, g.tabs.length - 1)]
    const focused = W.g
    focusGroup(group)
    if (next) openTab(next, { focus: !quiet })
    else editor.setModel(null)
    if (W.groups[focused]) focusGroup(focused)
  }
  changed({ tabs: true, status: true, layout: true })
}

// ---- editor groups (the split editor) ----------------------------------------------------

function editorOptions() {
  return {
    model: null,
    readOnly: W.readOnly,
    // Monaco's default ("editable") hides the squiggles in a read-only tab.
    renderValidationDecorations: "on",
    automaticLayout: true,
    theme: theme.monacoTheme(),
    fontSize: 13,
    "semanticHighlighting.enabled": true,
    inlayHints: { enabled: "on" },
    renameOnType: false,
    fixedOverflowWidgets: true,
  }
}

// A new editor group at the end, with its tabs bar, breadcrumbs and editor.
function addGroup() {
  const el = document.createElement("section")
  el.className = "group"
  el.innerHTML = `<div class="tabs" role="tablist" aria-label="Open files"></div><div class="crumbs"><span class="crumbs-path"></span></div><div class="editor-host"></div><div class="welcome" hidden><p><b>No file open.</b> Pick one in the explorer.</p><p><kbd>Ctrl</kbd>+<kbd>P</kbd> Go to file · <kbd>F1</kbd> Commands · <kbd>Shift</kbd>+<kbd>Alt</kbd>+<kbd>F</kbd> Format · <kbd>F2</kbd> Rename · <kbd>Ctrl</kbd>+<kbd>.</kbd> Quick fix</p></div>`
  $("groups").append(el)
  const ed = monaco.editor.create(el.querySelector(".editor-host"), editorOptions())
  const g = { tabs: [], active: null, views: new Map(), editor: ed, el }
  W.groups.push(g)
  ed.onDidFocusEditorText(() => focusGroup(W.groups.indexOf(g)))
  ed.onDidChangeCursorPosition(() => changed({ status: true }))
  // Suggestions pop up in Turtle and TriG strings, where the SPARQL queries
  // of SHACL shapes are typed; other languages keep Monaco's default.
  const stringSuggestions = () => {
    const on = ["turtle", "trig"].includes(ed.getModel()?.getLanguageId())
    ed.updateOptions({ quickSuggestions: { other: "on", comments: "off", strings: on ? "on" : "off" } })
  }
  ed.onDidChangeModel(stringSuggestions)
  ed.onDidChangeModelLanguage(stringSuggestions)
  g.isQuery = ed.createContextKey("rdf.isQuery", false)
  for (const c of COMMANDS) ed.addAction({ id: c.id, label: c.label, keybindings: c.key ? [c.key()] : [], precondition: c.when, contextMenuGroupId: c.menu, run: () => c.run() })
  queries.attach(g, () => (focusGroup(W.groups.indexOf(g)), queries.run(g)))
  settings?.applyTo(ed)
  vim.attach(ed)
  el.addEventListener("mousedown", () => focusGroup(W.groups.indexOf(g)))
  el.querySelector(".tabs").addEventListener("click", (e) => {
    const close = e.target.closest("[data-close]")
    if (close) return closeTab(close.dataset.close, { group: W.groups.indexOf(g) })
    const tab = e.target.closest("[role=tab]")
    if (tab) (focusGroup(W.groups.indexOf(g)), openTab(tab.dataset.path))
  })
  el.querySelector(".tabs").addEventListener("auxclick", (e) => {
    const tab = e.target.closest("[role=tab]")
    if (tab && e.button === 1) closeTab(tab.dataset.path, { group: W.groups.indexOf(g) })
  })
  return g
}

function focusGroup(i) {
  if (!W.groups[i]) return
  W.g = i
  editor = W.groups[i].editor
  for (const [k, g] of W.groups.entries()) g.el.classList.toggle("focused", k === i && W.groups.length > 1)
}

// Split: a second group showing the focused file (two groups at most).
function split() {
  const path = W.active
  if (W.groups.length === 1) addGroup()
  focusGroup(W.groups.length - 1)
  if (path) openTab(path)
  changed({ tabs: true, layout: true })
}

function closeGroup(i) {
  const [g] = W.groups.splice(i, 1)
  vim.detach(g.editor)
  g.editor.dispose()
  g.el.remove()
  focusGroup(Math.min(i, W.groups.length - 1))
  editor.focus()
}

function reveal(path, range) {
  openTab(path)
  const r = toMonacoRange(monaco, range)
  editor.setSelection(r)
  editor.revealRangeInCenter(r)
  editor.focus()
}

async function renderOutline() {
  const box = $("outline")
  if (!W.active) return (box.innerHTML = "")
  const model = W.files.get(W.active).model
  const symbols = (await client.request("textDocument/documentSymbol", { textDocument: { uri: model.uri.toString() } }).catch(() => null)) ?? []
  let html = ""
  const walk = (list, depth) => {
    for (const s of list) {
      const r = s.selectionRange ?? s.range ?? s.location?.range
      html += `<div class="row" data-line="${r.start.line}" data-ch="${r.start.character}" style="padding-left:${12 + depth * 12}px"><span class="nm">${esc(s.name)}</span></div>`
      walk(s.children ?? [], depth + 1)
    }
  }
  walk(symbols, 0)
  box.innerHTML = html || `<p style="padding:8px 20px;color:var(--muted)">No symbols in this file.</p>`
}

// ---- import and export --------------------------------------------------------------------------

// `entries`: [{ file, path }] with paths relative to the target folder.
// `skipped`: files already left out by the walk that found them.
async function importEntries(entries, dir = "", skipped = 0) {
  if (!writable()) return
  let added = 0, replaced = 0
  let first = null
  const { take, skipped: refused } = admit(entries, { have: W.files.size })
  skipped += refused
  for (const { file, path, onDisk } of take) {
    const dest = join(dir, path)
    if (W.files.has(dest)) replaced++
    else added++
    const text = await file.text()
    addFile(dest, text, { disk: onDisk ? text : undefined })
    first ??= dest
    for (let d = dirOf(dest); d; d = dirOf(d)) W.expanded.add(d)
  }
  if (first) openTab(first, { focus: false })
  changed({ tree: true, layout: true })
  const parts = [`${added} file${added === 1 ? "" : "s"} imported`]
  if (replaced) parts.push(`${replaced} replaced`)
  if (skipped) parts.push(`${skipped} skipped (${limitsNote()})`)
  toast(parts.join(", "))
}
// Items dropped from the desktop, imported into `dir`.
async function importDropped(items, dir) {
  const { entries, skipped } = await walkDropped(items)
  return importEntries(entries, dir, skipped)
}

// A folder on the disk, opened for editing in place: Ctrl+S writes back.
async function openInPlace() {
  if (!writable()) return
  let dir
  try {
    dir = await window.showDirectoryPicker({ mode: "readwrite" })
  } catch {
    return
  }
  // The same folder again updates its files; another one never takes in the
  // files already in the workspace: a name in use gets a number, as in VS Code.
  let root = null
  for (const [path, handle] of W.roots) if (await handle.isSameEntry?.(dir)) root = path
  if (!root) {
    const folders = folderList()
    const taken = (name) => W.roots.has(name) || folders.has(name) || W.files.has(name)
    root = dir.name
    for (let k = 2; taken(root); k++) root = `${dir.name} (${k})`
  }
  const { entries, skipped } = await walkHandle(dir, root)
  W.roots.set(root, dir)
  await importEntries(entries, "", skipped)
  $("status-storage").textContent = `${root}: Ctrl+S saves to disk`
}

// The handle of `path` inside its folder opened in place, made if missing.
async function fileHandle(path, { create = false } = {}) {
  const r = rootOf(path)
  if (!r) return null
  const mode = { mode: "readwrite" }
  // After a reload the browser asks again, once, on this user action.
  if ((await r.handle.queryPermission?.(mode)) !== "granted" && (await r.handle.requestPermission?.(mode)) !== "granted") {
    throw new Error("the browser did not allow writing to the folder")
  }
  const parts = path.slice(r.root.length + 1).split("/")
  let dir = r.handle
  for (const d of parts.slice(0, -1)) dir = await dir.getDirectoryHandle(d, { create })
  return dir.getFileHandle(parts.at(-1), { create })
}

// Save `path` to disk; whether it was. Files not opened in place are kept in
// the browser as they are typed.
async function save(path = W.active, { quiet = false } = {}) {
  const f = W.files.get(path)
  if (!f) return false
  // A save is a save for the server too (SHACL validates on save).
  const saved = () => client.notify("textDocument/didSave", { textDocument: { uri: f.model.uri.toString() } })
  if (!rootOf(path)) {
    saved()
    if (!quiet) toast("Kept in this browser: files are saved here as you type. Download or export them to keep a copy.")
    return true
  }
  try {
    const text = f.model.getValue()
    const version = f.model.getAlternativeVersionId()
    const w = await (await fileHandle(path, { create: true })).createWritable()
    await w.write(text)
    await w.close()
    setDisk(f, text, version)
    saved()
    changed({ tabs: true })
    if (!quiet) toast(`Saved ${path} to disk`)
    return true
  } catch (e) {
    toast(`Could not save ${path}: ${e.message}`)
    return false
  }
}

async function saveAll() {
  const paths = [...W.files.keys()].filter(dirty)
  let n = 0
  for (const p of paths) if (await save(p, { quiet: true })) n++
  toast(paths.length ? `Saved ${n} of ${paths.length} file${paths.length === 1 ? "" : "s"} to disk` : "No unsaved changes")
}

// Put the disk text back into `path` (undoable).
async function revert(path = W.active) {
  if (!writable()) return
  const f = W.files.get(path)
  if (!f || f.disk === undefined) return toast("Only files of a folder opened in place can be reverted.")
  try {
    const text = f.disk === null ? "" : await (await (await fileHandle(path)).getFile()).text()
    f.model.pushEditOperations([], [{ range: f.model.getFullModelRange(), text }], () => null)
    setDisk(f, f.disk === null ? null : text)
    changed({ tabs: true })
  } catch (e) {
    toast(`Could not read ${path}: ${e.message}`)
  }
}

// Don't Save: drop the unsaved changes (a file never saved goes away).
async function discard(path) {
  const f = W.files.get(path)
  if (f?.disk === null) return removeFile(path)
  if (f) (f.model.pushEditOperations([], [{ range: f.model.getFullModelRange(), text: f.disk }], () => null), setDisk(f, f.disk))
}

// A question in the page; resolves to the button chosen, null on Escape.
function ask(message, detail, buttons) {
  return new Promise((resolve) => {
    const box = $("dialog")
    box.innerHTML = `<p class="dq">${esc(message)}</p><p class="dd">${esc(detail)}</p><div class="db">${buttons.map((b, i) => `<button class="${i === 0 ? "pri" : ""}" data-b="${esc(b)}">${esc(b)}</button>`).join("")}</div>`
    box.hidden = false
    const before = document.activeElement
    const done = (answer) => {
      box.hidden = true
      box.onclick = box.onkeydown = null
      // The focus goes back to the file shown (as VS Code's dialogs do), else where it was.
      ;(W.active ? editor : before)?.focus()
      resolve(answer)
    }
    box.onclick = (e) => {
      const b = e.target.closest("[data-b]")
      if (b) done(b.dataset.b)
    }
    box.onkeydown = (e) => e.key === "Escape" && done(null)
    box.querySelector("button").focus()
  })
}

// The fetch quick fix on an unresolved import ("turtle.fetchImport"): the
// page fetches the ontology and adds it under imports/<host>/, with a line
// declaring its IRI, which is what makes the import present here.
const FETCHED = { "text/turtle": "ttl", "application/x-turtle": "ttl", "application/n-triples": "nt", "application/trig": "trig", "application/n-quads": "nq" }
async function fetchImport(iri) {
  if (!writable()) return
  let u
  try {
    u = new URL(iri)
    // The IRI also goes into a Turtle line below: nothing that ends its brackets.
    if ((u.protocol !== "http:" && u.protocol !== "https:") || /[\u0000-\u0020<>"{}|^`\\]/.test(iri)) throw new Error("not an address")
  } catch {
    return toast(`Could not fetch ${String(iri).slice(0, 200)}: it is not an http or https address.`)
  }
  const abort = new AbortController()
  const timer = setTimeout(() => abort.abort(), FETCH_TIMEOUT_MS)
  try {
    let r
    try {
      r = await fetch(u, { headers: { Accept: "text/turtle, application/n-triples;q=0.9, application/trig;q=0.8, application/n-quads;q=0.7" }, signal: abort.signal, credentials: "omit", referrerPolicy: "no-referrer" })
    } catch {
      return toast(`Could not fetch ${iri}: the site does not allow pages to read it, or it is offline. Download the file and import it instead.`)
    }
    if (!r.ok) return toast(`Could not fetch ${iri}: HTTP ${r.status}`)
    const type = (r.headers.get("content-type") ?? "").split(";")[0].trim().toLowerCase()
    const ext = FETCHED[type] ?? (type === "" || type === "text/plain" ? u.pathname.match(/\.(ttl|nt|trig|nq)$/i)?.[1]?.toLowerCase() : undefined)
    if (!ext) return toast(`${iri} came back as ${type || "an unknown type"}: only Turtle, N-Triples, TriG and N-Quads can be loaded here.`)
    let text
    try {
      text = await readTextCapped(r, MAX_FETCH_BYTES)
    } catch {
      return toast(`Could not fetch ${iri}: the connection broke or took too long. Download the file and import it instead.`)
    }
    if (text === null) return toast(`${iri} is larger than ${MAX_FETCH_BYTES / 1024 / 1024} MB: download it and import it instead.`)
    const stem = (u.pathname.split("/").filter(Boolean).pop() ?? "").replace(/\.[^.]*$/, "") || "ontology"
    let path = `imports/${u.host}/${stem}.${ext}`
    for (let k = 2; W.files.has(path); k++) path = `imports/${u.host}/${stem}-${k}.${ext}`
    const head = `# Fetched from <${iri}> on ${new Date().toISOString().slice(0, 10)}.\n<${iri}> a <http://www.w3.org/2002/07/owl#Ontology> .\n\n`
    addFile(path, head + text)
    for (let d = dirOf(path); d; d = dirOf(d)) W.expanded.add(d)
    changed({ tree: true, layout: true })
    toast(`Fetched ${iri} into ${path}`)
  } finally {
    clearTimeout(timer)
  }
}

function copy(text) {
  navigator.clipboard?.writeText(text).then(
    () => toast(`Copied ${text}`),
    () => toast(`Copying is not allowed here: ${text}`)
  )
}

function download(name, blob) {
  const url = URL.createObjectURL(blob)
  const a = Object.assign(document.createElement("a"), { href: url, download: name })
  document.body.append(a)
  a.click()
  a.remove()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}
function downloadFile(path = W.active ?? (W.files.has(W.selected) ? W.selected : null)) {
  const f = W.files.get(path)
  if (!f) return toast("Open a file to download it.")
  download(nameOf(path), new Blob([f.model.getValue()], { type: "text/plain" }))
}
function exportZip() {
  const files = [...W.files].map(([path, f]) => ({ path, text: f.model.getValue() })).sort((a, b) => a.path.localeCompare(b.path))
  download("workspace.zip", zip(files))
}

// ---- commands --------------------------------------------------------------------------------------

// Put `text` into `model` as one undoable step.
function replaceText(model, text) {
  model.pushStackElement()
  model.pushEditOperations([], [{ range: model.getFullModelRange(), text }], () => null)
  model.pushStackElement()
}

// The model of `path` if it is still the one that had version `version`: a
// result computed for an older text must not overwrite newer edits.
function unchanged(path, model, version) {
  const f = W.files.get(path)
  return Boolean(f && f.model === model && !model.isDisposed() && model.getVersionId() === version)
}

async function fixAll() {
  if (!writable()) return
  const snapshot = [...W.files].map(([path, f]) => ({ path, model: f.model, version: f.model.getVersionId(), text: f.model.getValue() }))
  try {
    const { files: fixed, applied } = await client.fixAll(snapshot.map(({ path, text }) => ({ name: path, text })))
    const at = new Map(snapshot.map((s) => [s.path, s]))
    let stale = 0
    for (const { name, text } of fixed) {
      const s = at.get(name)
      if (!s || s.text === text) continue
      if (!unchanged(name, s.model, s.version)) stale++
      else replaceText(s.model, text)
    }
    const note = stale ? `; ${stale} file${stale === 1 ? "" : "s"} changed meanwhile and ${stale === 1 ? "was" : "were"} left as ${stale === 1 ? "it is" : "they are"}: run Fix All again` : ""
    toast((applied ? `Applied ${applied} fix${applied === 1 ? "" : "es"}` : "Nothing to fix") + note)
  } catch (e) {
    toast(`Fixing failed: ${e.message}`)
  }
}

async function formatAll() {
  if (!writable()) return
  let n = 0
  let stale = 0
  for (const [path, f] of [...W.files]) {
    const { model } = f
    const version = model.getVersionId()
    const r = await client.request("textDocument/formatting", { textDocument: { uri: model.uri.toString() }, options: { tabSize: 4, insertSpaces: true } })
    if (!r?.length) continue
    if (!unchanged(path, model, version)) {
      stale++
      continue
    }
    model.pushStackElement()
    model.pushEditOperations([], r.map((e) => ({ range: toMonacoRange(monaco, e.range), text: e.newText })), () => null)
    model.pushStackElement()
    n++
  }
  const note = stale ? ` (${stale} file${stale === 1 ? "" : "s"} changed meanwhile and ${stale === 1 ? "was" : "were"} skipped: run it again)` : ""
  toast((n ? `Formatted ${n} file${n === 1 ? "" : "s"}` : "Every file is already formatted (or has syntax errors)") + note)
}

function toggleLabels() {
  settings.toggleLabels()
}

// The folder new files go to: the selected folder, or the selected file's.
const targetDir = () => (W.selected == null ? "" : W.files.has(W.selected) ? dirOf(W.selected) : W.selected)

function startNew(mode, dir = targetDir()) {
  if (!writable()) return
  if (dir) W.expanded.add(dir)
  W.editing = { mode, dir }
  renderTree()
}
function startRename(path = W.selected) {
  if (!path || !writable()) return
  W.editing = { mode: "rename", path, dir: dirOf(path) }
  renderTree()
}
function finishEdit(value) {
  const e = W.editing
  W.editing = null
  const name = value.trim()
  if (!e || !name) return renderTree()
  if (name.includes("/") || name === "." || name === "..") return toast("A name cannot contain a slash."), renderTree()
  const dest = join(e.dir, name)
  if ((W.files.has(dest) || folderList().has(dest)) && dest !== e.path) return toast(`${dest} already exists.`), renderTree()
  if (e.mode === "newFile") {
    addFile(dest, "")
    openTab(dest)
  } else if (e.mode === "newFolder") {
    W.folders.add(dest)
    W.expanded.add(dest)
    W.selected = dest
  } else if (dest !== e.path) {
    if (folderList().has(e.path) && !W.files.has(e.path)) W.folders.add(dest)
    movePath(e.path, dest)
    W.folders.delete(e.path)
  }
  changed({ tree: true, tabs: true, layout: true })
}

// Go to File (Ctrl+P), as VS Code's Quick Open: a name finds files, "#name"
// finds symbols across the workspace, "@name" symbols in the current file.
function quickOpen(prefix = "") {
  const box = $("quick")
  const input = $("quick-input")
  box.hidden = false
  input.value = prefix
  let on = 0
  let items = []
  let seq = 0
  const symbolItem = (s, path) => {
    const r = s.location?.range ?? s.selectionRange ?? s.range
    return { label: s.name, hint: s.containerName || path, path, range: r }
  }
  const flatten = (list, out = []) => {
    for (const s of list ?? []) out.push(s), flatten(s.children, out)
    return out
  }
  const compute = async () => {
    const q = input.value
    if (q.startsWith("#")) {
      const found = (await client.request("workspace/symbol", { query: q.slice(1) }).catch(() => [])) ?? []
      return found.flatMap((s) => {
        const path = pathByUri.get(s.location?.uri)
        return path ? [symbolItem(s, path)] : []
      })
    }
    if (q.startsWith("@")) {
      if (!W.active) return []
      const uri = W.files.get(W.active).model.uri.toString()
      const list = flatten(await client.request("textDocument/documentSymbol", { textDocument: { uri } }).catch(() => []))
      const want = q.slice(1).toLowerCase()
      return list.filter((s) => s.name.toLowerCase().includes(want)).map((s) => symbolItem(s, W.active))
    }
    const want = q.toLowerCase()
    return [...W.files.keys()]
      .filter((p) => p.toLowerCase().includes(want))
      .sort((a, b) => nameOf(a).localeCompare(nameOf(b)))
      .map((p) => ({ label: nameOf(p), hint: dirOf(p), path: p }))
  }
  const draw = () => {
    on = Math.min(on, Math.max(0, items.length - 1))
    $("quick-list").innerHTML = items
      .slice(0, 100)
      .map((it, i) => `<div class="quick-item${i === on ? " on" : ""}" role="option" data-i="${i}"><span>${esc(it.label)}</span><span class="dir">${esc(it.hint)}</span></div>`)
      .join("")
  }
  const refresh = async () => {
    const mine = ++seq
    const found = await compute()
    if (mine !== seq) return
    items = found
    draw()
  }
  const close = () => {
    box.hidden = true
    input.oninput = input.onkeydown = input.onblur = null
    $("quick-list").onclick = null
  }
  const pick = (it) => {
    close()
    if (it.range) reveal(it.path, it.range)
    else openTab(it.path)
  }
  input.oninput = () => ((on = 0), refresh())
  input.onkeydown = async (e) => {
    if (e.key === "ArrowDown") (on = Math.min(on + 1, items.length - 1)), draw(), e.preventDefault()
    else if (e.key === "ArrowUp") (on = Math.max(on - 1, 0)), draw(), e.preventDefault()
    else if (e.key === "Enter") {
      e.preventDefault()
      await refresh()
      if (items[on]) pick(items[on])
    } else if (e.key === "Escape") close(), editor.focus()
  }
  $("quick-list").onclick = (e) => {
    const it = e.target.closest("[data-i]")
    if (it) pick(items[+it.dataset.i])
  }
  input.onblur = () => setTimeout(close, 150)
  refresh()
  input.focus()
}

// Folders opened in place are let go too: none of the new files is on disk.
function resetWorkspace(files) {
  for (const p of [...W.files.keys()]) removeFile(p)
  W.folders.clear()
  W.roots.clear()
  W.folded.clear()
  W.expanded = new Set(files.map((f) => dirOf(f.path)).filter(Boolean))
  W.selected = null
  for (const f of files) addFile(f.path, f.text)
  const first = files.find((f) => f.path === "start-here.ttl") ?? files[0]
  if (first) openTab(first.path)
  changed({ tree: true, tabs: true, problems: true, status: true, layout: true })
}

const COMMANDS = [
  { id: "rdf.fixAll", label: "RDF: Fix All Problems", run: fixAll },
  { id: "rdf.formatAll", label: "RDF: Format All Files", run: formatAll },
  { id: "sparql.run", label: "SPARQL: Run Query", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.Enter, when: "rdf.isQuery", run: () => queries.run(W.groups[W.g]) },
  { id: "rdf.labels", label: "View: Toggle Labels", key: () => monaco.KeyMod.Alt | monaco.KeyCode.KeyL, run: toggleLabels },
  { id: "file.goto", label: "Go to File…", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyP, run: () => quickOpen() },
  { id: "file.gotoSymbol", label: "Go to Symbol in Workspace…", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyT, run: () => quickOpen("#") },
  { id: "file.save", label: "File: Save", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, run: () => save() },
  { id: "file.saveAll", label: "File: Save All", key: () => monaco.KeyMod.chord(monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyK, monaco.KeyCode.KeyS), run: saveAll },
  { id: "file.revert", label: "File: Revert File", run: () => revert() },
  { id: "file.compareSaved", label: "File: Compare with Saved", run: () => W.active && diff.compareWithSaved(W.active, W.files.get(W.active)) },
  { id: "rdf.previewFormat", label: "RDF: Preview Formatting", run: () => W.active && writable() && diff.previewFormatting(W.active, W.files.get(W.active)) },
  { id: "view.split", label: "View: Split Editor", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.Backslash, run: split },
  { id: "file.new", label: "File: New File", run: () => startNew("newFile") },
  { id: "file.newFolder", label: "File: New Folder", run: () => startNew("newFolder") },
  { id: "file.importFiles", label: "File: Import Files…", run: () => $("files-input").click() },
  { id: "file.importFolder", label: "File: Import Folder…", run: () => $("folder-input").click() },
  { id: "file.download", label: "File: Download File", run: () => downloadFile() },
  { id: "file.exportZip", label: "File: Export Workspace as Zip", run: exportZip },
  { id: "view.search", label: "View: Search", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyF, run: () => showView("search") },
  { id: "view.typeHierarchy", label: "Show Type Hierarchy", key: () => monaco.KeyMod.Shift | monaco.KeyMod.Alt | monaco.KeyCode.KeyH, menu: "navigation", run: () => (showView("hierarchy"), hierarchy.show()) },
  { id: "settings.open", label: "Preferences: Open Settings", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.Comma, run: () => settings.open() },
  { id: "view.panel", label: "View: Toggle Panel", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyJ, run: () => togglePanel() },
  { id: "workspace.sample", label: "Workspace: Reset to the Sample", run: () => writable() && resetWorkspace(WORKSPACE) },
  { id: "workspace.clear", label: "Workspace: Remove All Files", run: () => writable() && resetWorkspace([]) },
]

function togglePanel(open = !W.panel) {
  W.panel = open
  document.querySelector(".wb").classList.toggle("panel-closed", !open)
  $("panel-toggle").firstElementChild.className = `codicon codicon-chevron-${open ? "down" : "up"}`
}

// ---- menus and toasts ------------------------------------------------------------------------------------

let menuFrom = null // what had the focus before the menu took it
function menu(x, y, items) {
  const m = $("menu")
  if (m.hidden) menuFrom = document.activeElement
  m.innerHTML = items.map((it, i) => (it === "-" ? "<hr>" : `<button role="menuitem" data-i="${i}">${esc(it.label)}${it.key ? `<span class="k">${esc(it.key)}</span>` : ""}</button>`)).join("")
  m.hidden = false
  m.style.left = `${Math.min(x, innerWidth - m.offsetWidth - 8)}px`
  m.style.top = `${Math.min(y, innerHeight - m.offsetHeight - 8)}px`
  m.onclick = (e) => {
    const b = e.target.closest("[data-i]")
    if (!b) return
    m.hidden = true
    items[b.dataset.i].run()
  }
  m.querySelector("button")?.focus()
}
// Escape: the menu closes and the focus goes back.
function closeMenu() {
  const m = $("menu")
  if (m.hidden) return
  m.hidden = true
  if (m.contains(document.activeElement) || document.activeElement === document.body) menuFrom?.focus()
}
// The note on privacy, shown on the first visit and from the status bar's
// "Kept in this browser". Closing it gives the editor the focus.
const INTRO_SEEN = "rdf-editor-intro-seen"
function intro(show = true) {
  $("intro").hidden = !show
  if (show) $("intro").querySelector("button").focus()
  else editor?.focus()
}
let toastTimer
function toast(message, action, run) {
  const t = $("toast")
  t.innerHTML = `<span>${esc(message)}</span>${action ? `<button>${esc(action)}</button>` : ""}`
  t.hidden = false
  if (action) t.querySelector("button").onclick = () => ((t.hidden = true), run())
  clearTimeout(toastTimer)
  toastTimer = setTimeout(() => (t.hidden = true), action ? 8000 : 4000)
}

// ---- wiring ------------------------------------------------------------------------------------------------

function wire() {
  const tree = $("tree")
  tree.addEventListener("click", (e) => {
    const row = e.target.closest("[role=treeitem]")
    if (!row) return
    const p = row.dataset.path
    W.selected = p
    if (W.files.has(p)) openTab(p)
    else W.expanded.has(p) ? W.expanded.delete(p) : W.expanded.add(p)
    changed({ tree: true, layout: true })
  })
  tree.addEventListener("contextmenu", (e) => {
    e.preventDefault()
    const row = e.target.closest("[role=treeitem]")
    const p = row?.dataset.path ?? null
    W.selected = p
    renderTree()
    const isFile = p && W.files.has(p)
    const dir = p == null ? "" : isFile ? dirOf(p) : p
    const items = [
      { label: "New File…", run: () => startNew("newFile", dir) },
      { label: "New Folder…", run: () => startNew("newFolder", dir) },
      { label: "Import Files Here…", run: () => (($("files-input").dataset.dir = dir), $("files-input").click()) },
    ]
    if (p) {
      items.push("-")
      if (isFile) items.push({ label: "Download", run: () => downloadFile(p) })
      items.push(
        { label: "Copy Path", run: () => copy(uriOf(p)) },
        { label: "Copy Relative Path", run: () => copy(p) },
        "-",
        { label: "Rename", key: "F2", run: () => startRename(p) },
        { label: "Delete", key: "Delete", run: () => deletePath(p) }
      )
    }
    menu(e.clientX, e.clientY, items)
  })
  tree.addEventListener("keydown", (e) => {
    if (e.target.classList.contains("tree-input")) {
      // The Enter must not reach the new file's editor, which takes focus.
      if (e.key === "Enter") (e.preventDefault(), finishEdit(e.target.value))
      if (e.key === "Escape") (W.editing = null), renderTree(), tree.focus()
      return
    }
    const rows = [...tree.querySelectorAll("[role=treeitem]")]
    const i = rows.findIndex((r) => r.dataset.path === W.selected)
    if (e.key === "F2") startRename()
    else if (e.key === "Delete" || (e.key === "Backspace" && e.metaKey)) W.selected && deletePath(W.selected)
    else if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      const next = rows[Math.max(0, Math.min(rows.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))]
      if (next) (W.selected = next.dataset.path), renderTree()
      e.preventDefault()
    } else if (e.key === "Enter" && W.selected) W.files.has(W.selected) ? openTab(W.selected) : (W.expanded.has(W.selected) ? W.expanded.delete(W.selected) : W.expanded.add(W.selected), changed({ tree: true, layout: true }))
  })
  tree.addEventListener("focusout", (e) => {
    // Not when a render replaced the input: the new one has the focus.
    if (e.target.classList?.contains("tree-input")) setTimeout(() => W.editing && e.target.isConnected && finishEdit(e.target.value), 0)
  })
  // A row dragged within the tree moves; files and folders dropped from the
  // desktop are imported. Either lands in the folder under the pointer.
  const MOVE = "application/x-rdf-editor-path"
  tree.addEventListener("dragstart", (e) => {
    const row = e.target.closest("[role=treeitem]")
    if (!row) return
    e.dataTransfer.setData(MOVE, row.dataset.path)
    e.dataTransfer.effectAllowed = "move"
  })
  tree.addEventListener("dragover", (e) => {
    e.preventDefault()
    tree.classList.add("drop")
  })
  tree.addEventListener("dragleave", () => tree.classList.remove("drop"))
  tree.addEventListener("drop", async (e) => {
    e.preventDefault()
    tree.classList.remove("drop")
    const row = e.target.closest("[role=treeitem]")
    const p = row?.dataset.path ?? ""
    const dir = W.files.has(p) ? dirOf(p) : p
    const moving = e.dataTransfer?.getData(MOVE)
    if (moving) return moveInto(moving, dir)
    const items = e.dataTransfer?.items
    if (items?.length && items[0].webkitGetAsEntry) importDropped(items, dir)
    else if (e.dataTransfer?.files?.length) importEntries(picked(e.dataTransfer.files), dir)
  })

  const toolbar = {
    "New File": () => startNew("newFile"),
    "New Folder": () => startNew("newFolder"),
    "Import Files": () => (($("files-input").dataset.dir = ""), $("files-input").click()),
    "Import Folder": () => $("folder-input").click(),
    "Open Folder in Place": openInPlace,
    "Download File": () => downloadFile(),
    "Export Workspace as Zip": exportZip,
    "Collapse Folders": () => (W.expanded.clear(), changed({ tree: true, layout: true })),
    "Fix All Problems": fixAll,
  }
  document.addEventListener("click", (e) => {
    const b = e.target.closest("button[aria-label]")
    if (b && toolbar[b.getAttribute("aria-label")]) toolbar[b.getAttribute("aria-label")]()
    if (!e.target.closest("#menu")) $("menu").hidden = true
  })
  $("open-in-place").hidden = !("showDirectoryPicker" in window)
  $("files-input").addEventListener("change", (e) => {
    importEntries([...e.target.files].map((file) => ({ file, path: file.name })), e.target.dataset.dir ?? "")
    e.target.value = ""
  })
  $("folder-input").addEventListener("change", (e) => {
    importEntries(picked(e.target.files))
    e.target.value = ""
  })

  $("problems").addEventListener("click", (e) => {
    const row = e.target.closest(".problem")
    if (row) {
      const p = row.dataset.file
      const keep = { all: () => true, errors: (d) => d.severity === 1, warnings: (d) => d.severity === 2, shacl: (d) => d.code === "shacl-violation" }[W.filter]
      const d = (W.diags.get(W.files.get(p).model.uri.toString()) ?? []).filter(keep)[row.dataset.i]
      if (d) reveal(p, d.range)
      return
    }
    const file = e.target.closest("[data-fold]")
    if (!file) return
    const p = file.dataset.fold
    W.folded.has(p) ? W.folded.delete(p) : W.folded.add(p)
    renderProblems()
  })
  document.querySelector(".pfilter").addEventListener("click", (e) => {
    const b = e.target.closest("[data-filter]")
    if (!b) return
    W.filter = b.dataset.filter
    for (const x of document.querySelectorAll(".pf")) x.classList.toggle("on", x === b)
    renderProblems()
  })
  $("tab-problems").addEventListener("click", () => togglePanel(true))
  $("panel-toggle").addEventListener("click", () => togglePanel())
  $("status-problems").addEventListener("click", () => togglePanel())
  $("status-labels").addEventListener("click", toggleLabels)
  $("status-storage").addEventListener("click", () => intro())
  $("intro").addEventListener("click", (e) => {
    const act = e.target.closest("[data-act]")?.dataset.act
    if (act === "start" || e.target === $("intro")) intro(false)
    else if (act === "open") (intro(false), ($("files-input").dataset.dir = ""), $("files-input").click())
  })
  $("intro").addEventListener("keydown", (e) => e.key === "Escape" && (e.stopPropagation(), intro(false)))

  $("act-explorer").addEventListener("click", () => showView("explorer"))
  $("act-outline").addEventListener("click", () => showView("outline"))
  $("act-search").addEventListener("click", () => showView("search"))
  search = createSearch({ monaco, container: $("view-search"), files: () => [...W.files].map(([path, f]) => ({ path, model: f.model })), reveal, toast })
  $("act-hierarchy").addEventListener("click", () => showView("hierarchy"))
  hierarchy = createHierarchy({
    container: $("view-hierarchy"),
    request: (method, params) => client.request(method, params),
    at: () => (W.active && editor.getModel() ? { uri: editor.getModel().uri.toString(), position: { line: editor.getPosition().lineNumber - 1, character: editor.getPosition().column - 1 } } : null),
    pathOf: (uri) => pathByUri.get(uri),
    reveal,
    toast,
  })
  $("outline").addEventListener("click", (e) => {
    const row = e.target.closest("[data-line]")
    if (row && W.active) reveal(W.active, { start: { line: +row.dataset.line, character: +row.dataset.ch }, end: { line: +row.dataset.line, character: +row.dataset.ch } })
  })

  // Shortcuts that work outside the editor too.
  document.addEventListener("keydown", (e) => {
    const mod = e.ctrlKey || e.metaKey
    if (mod && e.key.toLowerCase() === "p" && !e.shiftKey) (e.preventDefault(), quickOpen())
    // Chrome keeps Ctrl+T for itself; where it reaches the page, it finds symbols.
    else if (mod && e.key.toLowerCase() === "t" && !e.shiftKey) (e.preventDefault(), quickOpen("#"))
    else if (mod && e.key.toLowerCase() === "s") (e.preventDefault(), save())
    // Splitting twice only refocuses the second group, so Monaco's binding
    // and this one may both run.
    else if (mod && e.key === "\\") (e.preventDefault(), split())
    else if (mod && e.shiftKey && e.key.toLowerCase() === "f") (e.preventDefault(), showView("search"))
    else if (e.key === "F1" || (mod && e.shiftKey && e.key.toLowerCase() === "p")) {
      e.preventDefault()
      if (W.active) (editor.focus(), editor.trigger("keyboard", "editor.action.quickCommand", {}))
    } else if (e.key === "Escape") closeMenu()
  })
}

function showView(which) {
  $("view-explorer").hidden = which !== "explorer"
  $("view-outline").hidden = which !== "outline"
  $("view-search").hidden = which !== "search"
  $("view-hierarchy").hidden = which !== "hierarchy"
  $("act-explorer").classList.toggle("on", which === "explorer")
  $("act-outline").classList.toggle("on", which === "outline")
  $("act-search").classList.toggle("on", which === "search")
  $("act-hierarchy").classList.toggle("on", which === "hierarchy")
  if (which === "outline") renderOutline()
  if (which === "search") search.focus()
  if (which === "hierarchy") hierarchy.refresh()
}

// ---- start -----------------------------------------------------------------------------------------------------

function defineThemes() {
  const rules = (dark) => [
    { token: "namespace", foreground: dark ? "4ec9b0" : "267f99" },
    { token: "class", foreground: dark ? "4ec9b0" : "267f99", fontStyle: "bold" },
    { token: "property", foreground: dark ? "9cdcfe" : "001080" },
    { token: "enumMember", foreground: dark ? "4fc1ff" : "0070c1" },
    { token: "type", foreground: dark ? "c586c0" : "af00db" },
    { token: "keyword", foreground: dark ? "569cd6" : "0000ff" },
    { token: "variable", foreground: dark ? "9cdcfe" : "795e26" },
    { token: "function", foreground: dark ? "dcdcaa" : "795e26" },
    { token: "*.deprecated", fontStyle: "strikethrough" },
    { token: "*.undescribed", fontStyle: "italic" },
  ]
  monaco.editor.defineTheme("rdf-light", { base: "vs", inherit: true, rules: rules(false), colors: {} })
  monaco.editor.defineTheme("rdf-dark", { base: "vs-dark", inherit: true, rules: rules(true), colors: {} })
}

// The workspace as the browser kept it replaces the one shown (nothing of it
// is written back: it is what is already kept).
function restoreWorkspace(kept) {
  restoring = true
  try {
    for (const p of [...W.files.keys()]) removeFile(p)
    while (W.groups.length > 1) closeGroup(W.groups.length - 1)
    Object.assign(W.groups[0], { tabs: [], active: null })
    W.groups[0].views.clear()
    focusGroup(0)
    W.folders = new Set(kept.folders ?? [])
    W.roots.clear()
    W.folded.clear()
    W.selected = null
    for (const r of kept.roots ?? []) W.roots.set(r.path, r.handle)
    for (const f of kept.files) addFile(f.path, f.text, { disk: f.disk })
    W.expanded = new Set(kept.expanded ?? [])
    const groups = kept.groups ?? [{ tabs: kept.tabs ?? [], active: kept.active }]
    groups.slice(0, 2).forEach((saved, i) => {
      if (i > 0) addGroup()
      focusGroup(i)
      W.tabs = (saved.tabs ?? []).filter((p) => W.files.has(p))
      const active = saved.active && W.files.has(saved.active) ? saved.active : W.tabs[0]
      if (active) openTab(active, { focus: false })
      if (i > 0 && !W.tabs.length) closeGroup(i)
    })
    focusGroup(Math.min(kept.focused ?? 0, W.groups.length - 1))
  } finally {
    restoring = false
  }
}

// The settings the browser kept (or, from before there were settings, the
// labels choice).
const keptSettings = (kept) => kept?.settings ?? (kept?.labels ? { "turtle.inlayHints.labels": "all" } : {})

// The tab that owned the workspace has gone and this one owns it now: show
// what that tab kept, settings included (the ones this tab started with are
// out of date, and would be written back), and let the editors write.
async function takeOver() {
  const kept = await ws.load()
  if (kept) settings.replace(keptSettings(kept))
  if (kept?.files) restoreWorkspace(kept)
  W.readOnly = false
  ws.setReadOnly(false)
  for (const g of W.groups) g.editor.updateOptions({ readOnly: false })
  document.body.dataset.saved = "true"
  $("status-storage").textContent = "Kept in this browser"
  changed({ tree: true, tabs: true, problems: true, status: true })
  toast("The other tab was closed. You can edit here now.")
}

async function main() {
  window.MonacoEnvironment = { getWorkerUrl: () => "monaco-worker.js" }
  monaco = await new Promise((resolve, reject) => {
    window.require.config({ paths: { vs: "vendor/monaco/vs" } })
    window.require(["vs/editor/editor.main"], () => resolve(window.monaco), reject)
  })
  registerSyntax(monaco)
  quietJson(monaco)
  defineThemes()
  theme = createTheme({ monaco })

  client = new LspClient(new URL("lsp-worker.js", import.meta.url))
  let idle = []
  client.on("busy", (busy) => {
    document.body.dataset.busy = String(busy)
    $("status-busy").hidden = !busy
    if (!busy) for (const r of idle.splice(0)) r()
  })
  client.on("diagnostics", (uri, list) => {
    W.diags.set(uri, list)
    W.tally.set(uri, { e: list.filter((d) => d.severity === 1).length, w: list.filter((d) => d.severity === 2).length })
    const path = pathByUri.get(uri)
    const model = path && W.files.get(path)?.model
    if (model) monaco.editor.setModelMarkers(model, "turtle-lsp", markers(monaco, list))
    changed({ tree: true, tabs: true, problems: true, status: true })
  })
  client.on("serverRequest", (method) => hooks?.refresh(method))
  client.on("crash", (message, { state, dropped = [] } = {}) => {
    const names = dropped.map((u) => pathByUri.get(u) ?? u).join(", ")
    if (state === "off") toast(`The engine keeps crashing, so language features are off. Reload the page to try again. (${message})`)
    else if (state === "dropped") toast(`The engine crashed and restarted without ${names}, which crashes it. Reload the page to try again. (${message})`)
    else toast(`The engine crashed and restarted with your files. (${message})`)
  })

  const init = await client.initialize({
    processId: null,
    rootUri: null,
    capabilities: {
      textDocument: {
        hover: { contentFormat: ["markdown", "plaintext"] },
        rename: { prepareSupport: true },
        publishDiagnostics: { relatedInformation: false },
      },
      workspace: { inlayHint: { refreshSupport: true }, semanticTokens: { refreshSupport: true } },
    },
  })
  hooks = connect(monaco, client, init.capabilities, (uri) => W.diags.get(uri) ?? [])
  $("status-version").textContent = `turtle-lsp ${init.serverInfo?.version ?? ""}`
  // One tab owns the workspace; another one shows it and writes nothing.
  const ownership = await claim()
  W.readOnly = !ownership.owner
  ws.setReadOnly(W.readOnly)
  document.body.dataset.saved = W.readOnly ? "readonly" : "true"
  // Settings go to the server before the first file opens.
  const kept = await ws.load()
  vim = createVim({
    monaco,
    status: $("status-vim"),
    commands: { save: () => save(), saveAll: () => saveAll(), close: () => W.active && closeTab(W.active), open: (path) => W.files.has(path) && (openTab(path), true) },
  })
  settings = createSettings({ monaco, client, mount: document.querySelector(".center"), opener: $("act-settings"), saved: keptSettings(kept), onChange: () => changed({ status: true, layout: true }), onClose: () => editor?.focus(), effects: { "editor.vimMode": (v) => vim.set(v), "workbench.colorTheme": (v) => theme.set(v) } })
  diff = createDiff({ monaco, client, mount: document.querySelector(".center"), toast, options: editorOptions, setup: (ed) => settings.applyTo(ed), onClose: () => editor?.focus() })

  // Cross-file navigation (definition, references) opens the file in a tab.
  monaco.editor.registerEditorOpener({
    openCodeEditor(source, resource, selection) {
      const path = pathByUri.get(resource.toString())
      if (!path) return false
      const i = W.groups.findIndex((g) => g.editor === source)
      if (i >= 0) focusGroup(i)
      openTab(path)
      if (selection) {
        const r = monaco.Range.isIRange(selection) ? selection : new monaco.Range(selection.lineNumber, selection.column, selection.lineNumber, selection.column)
        editor.setSelection(r)
        editor.revealRangeInCenter(r)
      }
      return true
    },
  })
  monaco.editor.registerCommand("turtle.fetchImport", (_accessor, iri) => fetchImport(iri))
  queries = createQueryPane({
    files: () => [...W.files].map(([path, f]) => ({ name: path, text: f.model.getValue() })),
    reveal: (path, line, column) => {
      if (!W.files.has(path)) return
      openTab(path)
      editor.setPosition({ lineNumber: line, column })
      editor.revealPositionInCenter({ lineNumber: line, column })
      editor.focus()
    },
  })
  addGroup()
  focusGroup(0)
  monaco.editor.addKeybindingRule({ keybinding: monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyP, command: "editor.action.quickCommand" })

  wire()
  // Leaving with changes not saved to disk asks first, as closing VS Code does.
  // Monaco cancels work it no longer needs (word highlighting when the file
  // shown changes) by rejecting with "Canceled", which it treats as expected.
  addEventListener("unhandledrejection", (e) => e.reason?.name === "Canceled" && e.preventDefault())
  addEventListener("beforeunload", (e) => {
    if ([...W.files.keys()].some(dirty)) e.preventDefault()
  })
  if (W.readOnly) {
    $("status-storage").textContent = "Read-only: open in another tab"
    toast("This workspace is open in another tab, so this tab shows it read-only. Close the other tab to edit here.")
    ownership.whenFree.then(takeOver)
  }
  if (kept?.files) restoreWorkspace(kept)
  else resetWorkspace(WORKSPACE)
  changed({ tree: true, tabs: true, problems: true, status: true })
  // Hand over what is waiting to be written when the page goes.
  addEventListener("pagehide", () => ws.flush())
  document.addEventListener("visibilitychange", () => document.visibilityState === "hidden" && ws.flush())
  try {
    if (!localStorage.getItem(INTRO_SEEN)) (localStorage.setItem(INTRO_SEEN, "1"), intro())
  } catch {}
  await new Promise((r) => (client.busy ? idle.push(r) : r()))
  document.body.dataset.ready = "true"
}

main().catch((e) => {
  document.body.dataset.ready = "error"
  toast(`The editor could not start: ${e.message}`)
  console.error(e)
})
