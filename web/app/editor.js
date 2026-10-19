// The RDF Editor: Monaco with turtle-lsp (in a worker) over a workspace of
// files and folders kept in this browser. VS Code's arrangement: explorer,
// tabs, editor, Problems, status bar, command palette (F1).
import { WORKSPACE } from "./samples.js"
import { LspClient, connect, markers, registerSyntax, languageOf } from "./editor-lsp.js"
import { zip } from "./zip.js"
import * as store from "./store.js"
import { createSearch } from "./search.js"

const ROOT = "file:///work/"
// The files a folder contributes, by extension; anything else is skipped.
const KNOWN = /\.(ttl|turtle|trig|nt|nq|rq|sparql|ru)$/i
const MAX_FILES = 2000
const MAX_FILE_BYTES = 10 * 1024 * 1024
const SAVE_DELAY_MS = 300
const ICON = { turtle: "TTL", trig: "TRIG", ntriples: "NT", nquads: "NQ", sparql: "RQ" }

const $ = (id) => document.getElementById(id)
const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c])
const dirOf = (p) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "")
const nameOf = (p) => p.slice(p.lastIndexOf("/") + 1)
const join = (dir, name) => (dir ? `${dir}/${name}` : name)
const under = (p, dir) => p === dir || p.startsWith(dir + "/")
const uriOf = (path) => ROOT + path.split("/").map(encodeURIComponent).join("/")

// ---- state --------------------------------------------------------------------------

const W = {
  files: new Map(), // path -> { model, handle?, disk? }
  folders: new Set(), // folders kept even when empty
  expanded: new Set(),
  selected: null, // the tree's selection (a file or folder path)
  tabs: [],
  active: null,
  views: new Map(), // path -> Monaco view state
  diags: new Map(), // uri -> LSP diagnostics
  filter: "all",
  labels: false,
  panel: true,
  editing: null, // { mode: "newFile" | "newFolder" | "rename", dir, path }
  folded: new Set(), // files whose problems are folded in the Problems panel
}
let monaco, editor, client, hooks, search
const pathByUri = new Map()

// ---- the workspace ---------------------------------------------------------------------

function folderList() {
  const all = new Set(W.folders)
  for (const p of W.files.keys()) for (let d = dirOf(p); d; d = dirOf(d)) all.add(d)
  return all
}

function addFile(path, text, { handle } = {}) {
  const existing = W.files.get(path)
  if (existing) {
    // Importing a file again replaces its text (undoable).
    existing.model.pushEditOperations([], [{ range: existing.model.getFullModelRange(), text }], () => null)
    if (handle) Object.assign(existing, { handle, disk: text })
    return
  }
  const uri = monaco.Uri.parse(uriOf(path))
  const model = monaco.editor.createModel(text, languageOf(path), uri)
  const key = model.uri.toString()
  pathByUri.set(key, path)
  W.files.set(path, { model, handle, disk: handle ? text : undefined })
  // Each change goes as its range and text (Monaco orders them from the end
  // of the document, so they apply in sequence).
  model.onDidChangeContent((e) => {
    const contentChanges = e.changes.map((c) => ({
      range: {
        start: { line: c.range.startLineNumber - 1, character: c.range.startColumn - 1 },
        end: { line: c.range.endLineNumber - 1, character: c.range.endColumn - 1 },
      },
      text: c.text,
    }))
    client.notify("textDocument/didChange", { textDocument: { uri: key, version: model.getVersionId() }, contentChanges })
    changed({ tabs: true })
  })
  client.notify("textDocument/didOpen", { textDocument: { uri: key, languageId: languageOf(path), version: model.getVersionId(), text } })
}

function removeFile(path) {
  const f = W.files.get(path)
  if (!f) return
  const key = f.model.uri.toString()
  client.notify("textDocument/didClose", { textDocument: { uri: key } })
  W.diags.delete(key)
  pathByUri.delete(key)
  f.model.dispose()
  W.files.delete(path)
  W.views.delete(path)
  closeTab(path, { quiet: true })
}

// Move every file (and kept folder) at or under `from` to `to`.
function movePath(from, to) {
  let reopen = null
  for (const [p, f] of [...W.files]) {
    if (!under(p, from)) continue
    const dest = to + p.slice(from.length)
    const text = f.model.getValue()
    const wasTab = W.tabs.indexOf(p)
    const wasActive = W.active === p
    removeFile(p)
    addFile(dest, text, { handle: f.handle })
    if (f.handle) W.files.get(dest).disk = f.disk
    if (wasTab >= 0) W.tabs.splice(Math.min(wasTab, W.tabs.length), 0, dest)
    if (wasActive) reopen = dest
  }
  if (reopen) openTab(reopen, { focus: false })
  for (const d of [...W.folders]) if (under(d, from)) (W.folders.delete(d), W.folders.add(to + d.slice(from.length)))
  for (const d of [...W.expanded]) if (under(d, from)) (W.expanded.delete(d), W.expanded.add(to + d.slice(from.length)))
  if (W.selected && under(W.selected, from)) W.selected = to + W.selected.slice(from.length)
}

// Drag and drop within the explorer: `path` into folder `dir`.
function moveInto(path, dir) {
  const dest = join(dir, nameOf(path))
  if (dest === path || under(dir, path)) return
  if (W.files.has(dest) || folderList().has(dest)) return toast(`${dest} already exists.`)
  const folder = !W.files.has(path)
  if (folder) W.folders.add(dest)
  movePath(path, dest)
  if (folder) W.folders.delete(path)
  if (dir) W.expanded.add(dir)
  changed({ tree: true, tabs: true })
}

function deletePath(path) {
  const removed = [...W.files].filter(([p]) => under(p, path)).map(([p, f]) => ({ path: p, text: f.model.getValue() }))
  const folders = [...W.folders].filter((d) => under(d, path))
  for (const { path: p } of removed) removeFile(p)
  for (const d of folders) W.folders.delete(d)
  if (W.selected && under(W.selected, path)) W.selected = null
  changed({ tree: true, tabs: true })
  toast(`Deleted ${nameOf(path)}`, "Undo", () => {
    for (const d of folders) W.folders.add(d)
    for (const f of removed) addFile(f.path, f.text)
    changed({ tree: true })
  })
}

// ---- persistence -------------------------------------------------------------------------

let saveTimer
function persist() {
  clearTimeout(saveTimer)
  saveTimer = setTimeout(async () => {
    const ok = await store.save({
      files: [...W.files].map(([path, f]) => ({ path, text: f.model.getValue() })),
      folders: [...W.folders],
      expanded: [...W.expanded],
      tabs: W.tabs,
      active: W.active,
      labels: W.labels,
    })
    $("status-storage").textContent = ok ? "Kept in this browser" : "Not kept: this browser refuses storage"
  }, SAVE_DELAY_MS)
}

// ---- rendering ------------------------------------------------------------------------------

let frame = null
const due = { tree: false, tabs: false, problems: false, status: false }
// Something changed: redraw what it affects on the next frame, and keep it.
function changed(what = {}) {
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
  persist()
}

function counts(path) {
  const f = W.files.get(path)
  const list = f ? W.diags.get(f.model.uri.toString()) ?? [] : []
  return { e: list.filter((d) => d.severity === 1).length, w: list.filter((d) => d.severity === 2).length }
}

function renderTree() {
  const folders = folderList()
  const children = (dir) => {
    const subs = [...folders].filter((d) => dirOf(d) === dir).sort((a, b) => a.localeCompare(b))
    const files = [...W.files.keys()].filter((p) => dirOf(p) === dir).sort((a, b) => a.localeCompare(b))
    return { subs, files }
  }
  const input = (depth, value) =>
    `<div class="row" style="padding-left:${8 + depth * 12}px"><span class="tw"></span><input class="tree-input" value="${esc(value)}" spellcheck="false" aria-label="Name"></div>`
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
        let errs = 0
        for (const p of W.files.keys()) if (under(p, d)) errs += counts(p).e
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
    box.setSelectionRange(0, dot > 0 ? dot : box.value.length)
  }
}

function renderTabs() {
  $("tabs").innerHTML = W.tabs
    .map((p) => {
      const f = W.files.get(p)
      const dirty = f?.handle && f.model.getValue() !== f.disk
      const c = counts(p)
      return `<div class="tab${dirty ? " dirty" : ""}" role="tab" aria-label="${esc(nameOf(p))}" aria-selected="${W.active === p}" data-path="${esc(p)}" title="${esc(p)}"><span class="${c.e ? "e" : ""}">${esc(nameOf(p))}</span><span class="x" data-close="${esc(p)}" aria-hidden="true"><span class="codicon codicon-close"></span></span></div>`
    })
    .join("")
  $("welcome").hidden = Boolean(W.active)
  $("crumbs").innerHTML = W.active ? W.active.split("/").map(esc).join(` <span class="codicon codicon-chevron-right"></span> `) : ""
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
  $("status-labels").textContent = `Labels: ${W.labels ? "On" : "Off"}`
  $("status-lang").textContent = W.active ? { turtle: "Turtle", trig: "TriG", ntriples: "N-Triples", nquads: "N-Quads", sparql: "SPARQL" }[languageOf(W.active)] : ""
  const pos = W.active && editor.getPosition()
  $("status-pos").textContent = pos ? `Ln ${pos.lineNumber}, Col ${pos.column}` : ""
}

// ---- tabs and the editor ----------------------------------------------------------------------

function openTab(path, { focus = true } = {}) {
  const f = W.files.get(path)
  if (!f) return
  if (W.active && W.files.has(W.active)) W.views.set(W.active, editor.saveViewState())
  if (!W.tabs.includes(path)) W.tabs.push(path)
  W.active = path
  W.selected = path
  for (let d = dirOf(path); d; d = dirOf(d)) W.expanded.add(d)
  editor.setModel(f.model)
  const view = W.views.get(path)
  if (view) editor.restoreViewState(view)
  if (focus) editor.focus()
  changed({ tree: true, tabs: true, status: true })
  if (!$("view-outline").hidden) renderOutline()
}

function closeTab(path, { quiet = false } = {}) {
  const i = W.tabs.indexOf(path)
  if (i < 0) return
  W.tabs.splice(i, 1)
  if (W.active === path) {
    W.active = null
    const next = W.tabs[Math.min(i, W.tabs.length - 1)]
    if (next) openTab(next, { focus: !quiet })
    else editor.setModel(null)
  }
  changed({ tabs: true, status: true })
}

function reveal(path, range) {
  openTab(path)
  const r = new monaco.Range(range.start.line + 1, range.start.character + 1, range.end.line + 1, range.end.character + 1)
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
async function importEntries(entries, dir = "") {
  let added = 0, replaced = 0, skipped = 0
  let first = null
  for (const { file, path } of entries) {
    if (!KNOWN.test(file.name) || file.size > MAX_FILE_BYTES || W.files.size >= MAX_FILES) {
      skipped++
      continue
    }
    const dest = join(dir, path)
    if (W.files.has(dest)) replaced++
    else added++
    addFile(dest, await file.text(), { handle: file.handle })
    first ??= dest
    for (let d = dirOf(dest); d; d = dirOf(d)) W.expanded.add(d)
  }
  if (first) openTab(first, { focus: false })
  changed({ tree: true })
  const parts = [`${added} file${added === 1 ? "" : "s"} imported`]
  if (replaced) parts.push(`${replaced} replaced`)
  if (skipped) parts.push(`${skipped} skipped (only Turtle, TriG, N-Triples, N-Quads and SPARQL files up to 10 MB)`)
  toast(parts.join(", "))
}
const picked = (list) => [...list].map((file) => ({ file, path: file.webkitRelativePath || file.name }))

// The files under dropped items, folders walked, paths kept.
async function dropped(items) {
  const out = []
  const walk = async (entry) => {
    if (entry.isFile) {
      const file = await new Promise((res, rej) => entry.file(res, rej))
      out.push({ file, path: entry.fullPath.replace(/^\//, "") })
    } else if (entry.isDirectory) {
      const reader = entry.createReader()
      for (;;) {
        const batch = await new Promise((res, rej) => reader.readEntries(res, rej))
        if (!batch.length) break
        for (const e of batch) await walk(e)
      }
    }
  }
  for (const entry of [...items].map((i) => i.webkitGetAsEntry?.()).filter(Boolean)) await walk(entry)
  return out
}

// A folder on the disk, opened for editing in place: Ctrl+S writes back.
async function openInPlace() {
  let dir
  try {
    dir = await window.showDirectoryPicker({ mode: "readwrite" })
  } catch {
    return
  }
  const entries = []
  const walk = async (handle, prefix) => {
    for await (const [name, h] of handle.entries()) {
      if (h.kind === "directory") {
        if (!name.startsWith(".") && name !== "node_modules") await walk(h, join(prefix, name))
      } else if (KNOWN.test(name)) {
        const file = await h.getFile()
        file.handle = h
        entries.push({ file, path: join(prefix, name) })
      }
    }
  }
  await walk(dir, dir.name)
  await importEntries(entries)
  $("status-storage").textContent = `${dir.name}: Ctrl+S saves to disk`
}

async function save(path = W.active) {
  const f = W.files.get(path)
  if (!f) return
  if (!f.handle) return toast("Kept in this browser: files are saved here as you type. Download or export them to keep a copy.")
  try {
    const w = await f.handle.createWritable()
    const text = f.model.getValue()
    await w.write(text)
    await w.close()
    f.disk = text
    changed({ tabs: true })
    toast(`Saved ${path} to disk`)
  } catch (e) {
    toast(`Could not save ${path}: ${e.message}`)
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

async function fixAll() {
  const files = [...W.files].map(([path, f]) => ({ name: path, text: f.model.getValue() }))
  try {
    const { files: fixed, applied } = await client.fixAll(files)
    for (const { name, text } of fixed) {
      const m = W.files.get(name)?.model
      if (m && m.getValue() !== text) m.pushEditOperations([], [{ range: m.getFullModelRange(), text }], () => null)
    }
    toast(applied ? `Applied ${applied} fix${applied === 1 ? "" : "es"}` : "Nothing to fix")
  } catch (e) {
    toast(`Fixing failed: ${e.message}`)
  }
}

async function formatAll() {
  let n = 0
  for (const [, f] of W.files) {
    const r = await client.request("textDocument/formatting", { textDocument: { uri: f.model.uri.toString() }, options: { tabSize: 4, insertSpaces: true } })
    if (!r?.length) continue
    f.model.pushEditOperations([], r.map((e) => ({ range: new monaco.Range(e.range.start.line + 1, e.range.start.character + 1, e.range.end.line + 1, e.range.end.character + 1), text: e.newText })), () => null)
    n++
  }
  toast(n ? `Formatted ${n} file${n === 1 ? "" : "s"}` : "Every file is already formatted (or has syntax errors)")
}

function toggleLabels() {
  W.labels = !W.labels
  client.notify("workspace/didChangeConfiguration", { settings: { turtle: { inlayHints: { labels: W.labels ? "all" : "opaque" } } } })
  changed({ status: true })
}

// The folder new files go to: the selected folder, or the selected file's.
const targetDir = () => (W.selected == null ? "" : W.files.has(W.selected) ? dirOf(W.selected) : W.selected)

function startNew(mode, dir = targetDir()) {
  if (dir) W.expanded.add(dir)
  W.editing = { mode, dir }
  renderTree()
}
function startRename(path = W.selected) {
  if (!path) return
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
  changed({ tree: true, tabs: true })
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

function resetWorkspace(files) {
  for (const p of [...W.files.keys()]) removeFile(p)
  W.folders.clear()
  W.expanded = new Set(files.map((f) => dirOf(f.path)).filter(Boolean))
  W.selected = null
  for (const f of files) addFile(f.path, f.text)
  const first = files.find((f) => f.path.startsWith("data/")) ?? files[0]
  if (first) openTab(first.path)
  changed({ tree: true, tabs: true, problems: true, status: true })
}

const COMMANDS = [
  { id: "rdf.fixAll", label: "RDF: Fix All Problems", run: fixAll },
  { id: "rdf.formatAll", label: "RDF: Format All Files", run: formatAll },
  { id: "rdf.labels", label: "View: Toggle Labels", key: () => monaco.KeyMod.Alt | monaco.KeyCode.KeyL, run: toggleLabels },
  { id: "file.goto", label: "Go to File…", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyP, run: () => quickOpen() },
  { id: "file.gotoSymbol", label: "Go to Symbol in Workspace…", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyT, run: () => quickOpen("#") },
  { id: "file.save", label: "File: Save", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyS, run: () => save() },
  { id: "file.new", label: "File: New File", run: () => startNew("newFile") },
  { id: "file.newFolder", label: "File: New Folder", run: () => startNew("newFolder") },
  { id: "file.importFiles", label: "File: Import Files…", run: () => $("files-input").click() },
  { id: "file.importFolder", label: "File: Import Folder…", run: () => $("folder-input").click() },
  { id: "file.download", label: "File: Download File", run: () => downloadFile() },
  { id: "file.exportZip", label: "File: Export Workspace as Zip", run: exportZip },
  { id: "view.search", label: "View: Search", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyF, run: () => showView("search") },
  { id: "view.panel", label: "View: Toggle Panel", key: () => monaco.KeyMod.CtrlCmd | monaco.KeyCode.KeyJ, run: () => togglePanel() },
  { id: "workspace.sample", label: "Workspace: Reset to the Sample", run: () => resetWorkspace(WORKSPACE) },
  { id: "workspace.clear", label: "Workspace: Remove All Files", run: () => resetWorkspace([]) },
]

function togglePanel(open = !W.panel) {
  W.panel = open
  document.querySelector(".wb").classList.toggle("panel-closed", !open)
  $("panel-toggle").firstElementChild.className = `codicon codicon-chevron-${open ? "down" : "up"}`
}

// ---- menus and toasts ------------------------------------------------------------------------------------

function menu(x, y, items) {
  const m = $("menu")
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
    changed({ tree: true })
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
      if (e.key === "Enter") finishEdit(e.target.value)
      if (e.key === "Escape") (W.editing = null), renderTree()
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
    } else if (e.key === "Enter" && W.selected) W.files.has(W.selected) ? openTab(W.selected) : (W.expanded.has(W.selected) ? W.expanded.delete(W.selected) : W.expanded.add(W.selected), changed({ tree: true }))
  })
  tree.addEventListener("focusout", (e) => {
    if (e.target.classList?.contains("tree-input")) setTimeout(() => W.editing && finishEdit(e.target.value), 0)
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
    if (items?.length && items[0].webkitGetAsEntry) importEntries(await dropped(items), dir)
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
    "Collapse Folders": () => (W.expanded.clear(), changed({ tree: true })),
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

  $("tabs").addEventListener("click", (e) => {
    const close = e.target.closest("[data-close]")
    if (close) return closeTab(close.dataset.close)
    const tab = e.target.closest("[role=tab]")
    if (tab) openTab(tab.dataset.path)
  })
  $("tabs").addEventListener("auxclick", (e) => {
    const tab = e.target.closest("[role=tab]")
    if (tab && e.button === 1) closeTab(tab.dataset.path)
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

  $("act-explorer").addEventListener("click", () => showView("explorer"))
  $("act-outline").addEventListener("click", () => showView("outline"))
  $("act-search").addEventListener("click", () => showView("search"))
  search = createSearch({ monaco, container: $("view-search"), files: () => [...W.files].map(([path, f]) => ({ path, model: f.model })), reveal, toast })
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
    else if (mod && e.shiftKey && e.key.toLowerCase() === "f") (e.preventDefault(), showView("search"))
    else if (e.key === "F1" || (mod && e.shiftKey && e.key.toLowerCase() === "p")) {
      e.preventDefault()
      if (W.active) (editor.focus(), editor.trigger("keyboard", "editor.action.quickCommand", {}))
    } else if (e.key === "Escape") $("menu").hidden = true
  })
}

function showView(which) {
  $("view-explorer").hidden = which !== "explorer"
  $("view-outline").hidden = which !== "outline"
  $("view-search").hidden = which !== "search"
  $("act-explorer").classList.toggle("on", which === "explorer")
  $("act-outline").classList.toggle("on", which === "outline")
  $("act-search").classList.toggle("on", which === "search")
  if (which === "outline") renderOutline()
  if (which === "search") search.focus()
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

async function main() {
  window.MonacoEnvironment = { getWorkerUrl: () => "monaco-worker.js" }
  monaco = await new Promise((resolve, reject) => {
    window.require.config({ paths: { vs: "vendor/monaco/vs" } })
    window.require(["vs/editor/editor.main"], () => resolve(window.monaco), reject)
  })
  registerSyntax(monaco)
  defineThemes()
  const dark = matchMedia("(prefers-color-scheme: dark)")
  editor = monaco.editor.create($("editor"), {
    model: null,
    automaticLayout: true,
    theme: dark.matches ? "rdf-dark" : "rdf-light",
    fontSize: 13,
    "semanticHighlighting.enabled": true,
    inlayHints: { enabled: "on" },
    renameOnType: false,
    fixedOverflowWidgets: true,
  })
  dark.addEventListener("change", () => monaco.editor.setTheme(dark.matches ? "rdf-dark" : "rdf-light"))
  editor.onDidChangeCursorPosition(() => changed({ status: true }))

  client = new LspClient(new URL("lsp-worker.js", import.meta.url))
  let idle = []
  client.on("busy", (busy) => {
    document.body.dataset.busy = String(busy)
    $("status-busy").hidden = !busy
    if (!busy) for (const r of idle.splice(0)) r()
  })
  client.on("diagnostics", (uri, list) => {
    W.diags.set(uri, list)
    const path = pathByUri.get(uri)
    const model = path && W.files.get(path)?.model
    if (model) monaco.editor.setModelMarkers(model, "turtle-lsp", markers(monaco, list))
    changed({ tree: true, tabs: true, problems: true, status: true })
  })
  client.on("serverRequest", (method) => hooks?.refresh(method))
  client.on("crash", (message) => toast(`The engine crashed and restarted with your files: ${message}`))

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

  // Cross-file navigation (definition, references) opens the file in a tab.
  monaco.editor.registerEditorOpener({
    openCodeEditor(_source, resource, selection) {
      const path = pathByUri.get(resource.toString())
      if (!path) return false
      openTab(path)
      if (selection) {
        const r = monaco.Range.isIRange(selection) ? selection : new monaco.Range(selection.lineNumber, selection.column, selection.lineNumber, selection.column)
        editor.setSelection(r)
        editor.revealRangeInCenter(r)
      }
      return true
    },
  })
  for (const c of COMMANDS) {
    editor.addAction({ id: c.id, label: c.label, keybindings: c.key ? [c.key()] : [], run: () => c.run() })
  }
  monaco.editor.addKeybindingRule({ keybinding: monaco.KeyMod.CtrlCmd | monaco.KeyMod.Shift | monaco.KeyCode.KeyP, command: "editor.action.quickCommand" })

  wire()
  const kept = await store.load()
  if (kept?.files) {
    for (const d of kept.folders ?? []) W.folders.add(d)
    for (const f of kept.files) addFile(f.path, f.text)
    W.expanded = new Set(kept.expanded ?? [])
    W.tabs = (kept.tabs ?? []).filter((p) => W.files.has(p))
    const active = kept.active && W.files.has(kept.active) ? kept.active : W.tabs[0]
    if (active) openTab(active, { focus: false })
    if (kept.labels) toggleLabels()
  } else {
    resetWorkspace(WORKSPACE)
  }
  changed({ tree: true, tabs: true, problems: true, status: true })
  await new Promise((r) => (client.busy ? idle.push(r) : r()))
  document.body.dataset.ready = "true"
}

main().catch((e) => {
  document.body.dataset.ready = "error"
  toast(`The editor could not start: ${e.message}`)
  console.error(e)
})
