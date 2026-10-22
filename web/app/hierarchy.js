// The Type Hierarchy view: the class or property under the cursor with its
// supertypes or subtypes (rdfs:subClassOf, rdfs:subPropertyOf), as VS Code's.
// Children are asked from the language server when a row is first expanded.

import { esc } from "./dom.js"

const ICON = { 5: "symbol-class", 7: "symbol-property", 11: "symbol-interface" }
const NOTHING = "No class or property at the cursor. Place the cursor on a class or property and run Show Type Hierarchy (Shift+Alt+H)."

/**
 * Build the Type Hierarchy view inside `container`.
 * @param {object} o
 * @param {HTMLElement} o.container the side-bar section to fill
 * @param {(method: string, params: object) => Promise<any>} o.request a request to the language server
 * @param {() => ({ uri: string, position: { line: number, character: number } } | null)} o.at the cursor
 * @param {(uri: string) => string | undefined} o.pathOf the workspace path of a document URI
 * @param {(path: string, range: object) => void} o.reveal open `path` and select an LSP range
 * @param {(message: string) => void} o.toast
 * @returns {{ show(): Promise<void>, refresh(): Promise<void> }}
 */
export function createHierarchy({ container, request, at, pathOf, reveal, toast }) {
  container.innerHTML = `
    <div class="view-hd"><span class="view-title">Type Hierarchy</span><span class="sp"></span>
      <button class="ic" data-dir="supertypes" aria-label="Show Supertypes" title="Show Supertypes" aria-pressed="false"><span class="codicon codicon-type-hierarchy-super"></span></button>
      <button class="ic" data-dir="subtypes" aria-label="Show Subtypes" title="Show Subtypes" aria-pressed="true"><span class="codicon codicon-type-hierarchy-sub"></span></button>
      <button class="ic" data-act="refresh" aria-label="Refresh" title="Refresh"><span class="codicon codicon-refresh"></span></button></div>
    <div class="th-msg" aria-live="polite"></div>
    <div class="th-tree" role="tree" aria-label="Type hierarchy"></div>`
  const msg = container.querySelector(".th-msg")
  const tree = container.querySelector(".th-tree")

  let dir = "subtypes"
  let root = null // { item, open, children: node[] | null, loading }
  const nodes = [] // rendered rows' nodes, by data-n
  let generation = 0

  const node = (item) => ({ item, open: false, children: null })

  async function load(n) {
    const g = generation
    n.loading = true
    const list = await request(`typeHierarchy/${dir}`, { item: n.item }).catch(() => null)
    if (g !== generation) return false
    n.loading = false
    n.children = (list ?? []).map(node)
    return true
  }

  // Ask again for the children of every open row, keeping open what still is.
  async function reload(n, was) {
    if (!(await load(n))) return false
    const open = new Set((was ?? []).filter((c) => c.open).map((c) => c.item.data?.iri ?? c.item.name))
    const old = new Map((was ?? []).map((c) => [c.item.data?.iri ?? c.item.name, c]))
    for (const c of n.children) {
      const key = c.item.data?.iri ?? c.item.name
      if (!open.has(key)) continue
      c.open = true
      if (!(await reload(c, old.get(key).children))) return false
    }
    return true
  }

  async function expand(n) {
    n.open = true
    if (!n.children) {
      render()
      await load(n)
    }
    render()
  }

  // Ask again at the cursor and show the item with its first level open.
  async function show() {
    const g = ++generation
    const here = at()
    const items = here ? await request("textDocument/prepareTypeHierarchy", { textDocument: { uri: here.uri }, position: here.position }).catch(() => null) : null
    if (g !== generation) return
    root = items?.length ? node(items[0]) : null
    if (root) await expand(root)
    else render()
  }

  // Ask again for the shown item's tree; with none shown, ask at the cursor.
  async function refresh() {
    if (!root) return show()
    generation++
    const was = root.open ? root.children : null
    root = { ...node(root.item), open: true }
    if (await reload(root, was)) render()
  }

  function render() {
    msg.textContent = root ? "" : NOTHING
    msg.hidden = !!root
    nodes.length = 0
    let html = ""
    const row = (n, depth) => {
      const i = nodes.push(n) - 1
      const { item } = n
      const path = pathOf(item.uri)
      const file = path ? path.slice(path.lastIndexOf("/") + 1) : item.uri.slice(item.uri.lastIndexOf("/") + 1)
      const leaf = n.children && !n.children.length
      const twisty = leaf ? "blank" : n.loading ? "loading codicon-modifier-spin" : n.open ? "chevron-down" : "chevron-right"
      const detail = [item.detail, file].filter(Boolean).join(" · ")
      html += `<div class="th-row" role="treeitem" aria-level="${depth + 1}"${leaf ? "" : ` aria-expanded="${n.open}"`} data-n="${i}" style="padding-left:${4 + depth * 12}px" title="${esc(item.data?.iri ?? item.name)}"><span class="th-twist codicon codicon-${twisty}"></span><span class="codicon codicon-${ICON[item.kind] ?? "symbol-class"}"></span><span class="th-name">${esc(item.name)}</span><span class="th-detail">${esc(detail)}</span></div>`
      if (n.open) for (const c of n.children ?? []) row(c, depth + 1)
    }
    if (root) row(root, 0)
    if (root?.open && root.children && !root.children.length) html += `<div class="th-none" style="padding-left:28px">No ${dir}.</div>`
    tree.innerHTML = html
  }

  function setDir(d) {
    dir = d
    for (const b of container.querySelectorAll("[data-dir]")) b.setAttribute("aria-pressed", String(b.dataset.dir === d)), b.classList.toggle("on", b.dataset.dir === d)
    if (!root) return
    generation++
    root = node(root.item)
    expand(root)
  }

  for (const b of container.querySelectorAll("[data-dir]")) b.addEventListener("click", () => setDir(b.dataset.dir))
  container.querySelector("[data-act=refresh]").addEventListener("click", refresh)
  tree.addEventListener("click", (e) => {
    const r = e.target.closest("[data-n]")
    const n = r && nodes[r.dataset.n]
    if (!n) return
    if (e.target.closest(".th-twist")) {
      if (n.open) return (n.open = false), render()
      return expand(n)
    }
    const path = pathOf(n.item.uri)
    if (path) reveal(path, n.item.selectionRange)
    else toast(`${n.item.name} is not defined in a workspace file`)
  })
  tree.addEventListener("dblclick", (e) => {
    const r = e.target.closest("[data-n]")
    const n = r && nodes[r.dataset.n]
    if (n && !e.target.closest(".th-twist")) n.open ? ((n.open = false), render()) : expand(n)
  })
  container.querySelector("[data-dir=subtypes]").classList.add("on")
  render()

  return { show, refresh }
}
