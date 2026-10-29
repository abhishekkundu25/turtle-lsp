// The class hierarchy as a tree of rows (role tree), children drawn when a
// row is first expanded, at most PAGE per level with a "+N more" button.
// Keyboard as in VS Code's trees: arrows move, Right/Left expand or collapse,
// Enter opens the definition.

import { h } from "./el.js"
import { KINDS, nameOf } from "./model.js"
import { pathTo } from "./rings.js"

const PAGE = 200

/**
 * @param {HTMLElement} el
 * @param {object} on
 * @param {(i: number) => void} on.select
 * @param {(i: number) => void} on.open
 */
export function createTreeView(el, on) {
  const doc = el.ownerDocument
  const list = h(doc, "ul", { class: "graph-tree", role: "tree", "aria-label": "Class hierarchy" })
  const note = h(doc, "p", { class: "graph-none" })
  el.replaceChildren(note, list)
  let model = null
  let hier = null
  let selected = -1
  const expanded = new Set()

  function item(i, level) {
    const kids = hier.children.get(i)
    const li = h(doc, "li", { role: "treeitem", "aria-level": level, "aria-selected": String(i === selected), "aria-expanded": kids ? String(expanded.has(i)) : null, tabindex: "-1", "data-i": i },
      h(doc, "div", { class: "graph-row" },
        h(doc, "span", { class: "graph-twist", "aria-hidden": "true", text: kids ? (expanded.has(i) ? "▾" : "▸") : "" }),
        h(doc, "span", { class: "graph-tree-name", text: nameOf(model.nodes[i]) }),
        kids ? h(doc, "span", { class: "graph-count", text: hier.size[i].toLocaleString("en") }) : h(doc, "span", { class: "graph-count", text: KINDS[model.kind[i]] ?? "" })))
    if (kids && expanded.has(i)) li.append(group(kids, level + 1))
    return li
  }
  function group(ids, level, from = 0) {
    const ul = h(doc, "ul", { role: "group" })
    fill(ul, ids, level, from)
    return ul
  }
  function fill(ul, ids, level, from) {
    for (const i of ids.slice(from, from + PAGE)) ul.append(item(i, level))
    if (ids.length > from + PAGE) {
      const more = h(doc, "li", { role: "none" }, h(doc, "button", { type: "button", class: "graph-more-row", text: `+${(ids.length - from - PAGE).toLocaleString("en")} more` }))
      more.firstChild.addEventListener("click", () => {
        more.remove()
        fill(ul, ids, level, from + PAGE)
      })
      ul.append(more)
    }
  }
  const rowOf = (i) => list.querySelector(`[role="treeitem"][data-i="${i}"]`)
  const items = () => [...list.querySelectorAll('[role="treeitem"]')]

  function setExpanded(li, open) {
    const i = Number(li.dataset.i)
    if (!hier.children.has(i) || expanded.has(i) === open) return
    open ? expanded.add(i) : expanded.delete(i)
    li.setAttribute("aria-expanded", String(open))
    li.querySelector(".graph-twist").textContent = open ? "▾" : "▸"
    if (open) li.append(group(hier.children.get(i), Number(li.getAttribute("aria-level")) + 1))
    else li.querySelector(':scope > [role="group"]')?.remove()
  }
  function current(li, focus = true) {
    for (const r of list.querySelectorAll('[tabindex="0"]')) r.tabIndex = -1
    li.tabIndex = 0
    if (focus) li.focus()
  }
  function choose(li, focus = true) {
    for (const r of list.querySelectorAll('[aria-selected="true"]')) r.setAttribute("aria-selected", "false")
    li.setAttribute("aria-selected", "true")
    selected = Number(li.dataset.i)
    current(li, focus)
    on.select(selected)
  }

  list.addEventListener("click", (e) => {
    const li = e.target.closest('[role="treeitem"]')
    if (!li) return
    if (e.target.closest(".graph-twist")) setExpanded(li, li.getAttribute("aria-expanded") !== "true")
    choose(li)
  })
  list.addEventListener("dblclick", (e) => {
    const li = e.target.closest('[role="treeitem"]')
    if (li) on.open(Number(li.dataset.i))
  })
  list.addEventListener("keydown", (e) => {
    const li = e.target.closest('[role="treeitem"]')
    if (!li) return
    const all = items()
    const at = all.indexOf(li)
    const go = (k) => all[k] && current(all[k])
    if (e.key === "ArrowDown") go(at + 1)
    else if (e.key === "ArrowUp") go(at - 1)
    else if (e.key === "Home") go(0)
    else if (e.key === "End") go(all.length - 1)
    else if (e.key === "ArrowRight") {
      if (li.getAttribute("aria-expanded") === "false") setExpanded(li, true)
      else if (li.getAttribute("aria-expanded") === "true") go(at + 1)
    } else if (e.key === "ArrowLeft") {
      if (li.getAttribute("aria-expanded") === "true") setExpanded(li, false)
      else {
        const parent = li.parentElement.closest('[role="treeitem"]')
        if (parent) current(parent)
      }
    } else if (e.key === " ") choose(li)
    else if (e.key === "Enter") on.open(Number(li.dataset.i))
    else return
    e.preventDefault()
  })

  return {
    /** Draw the hierarchy of `m` (rings.js buildHierarchy). The first roots open when there are few. */
    show(m, hierarchy) {
      model = m
      hier = hierarchy
      expanded.clear()
      selected = -1
      if (hier.roots.length <= 10) for (const r of hier.roots) if (hier.children.has(r)) expanded.add(r)
      list.replaceChildren()
      fill(list, hier.roots, 1, 0)
      note.textContent = hier.roots.length ? "" : "No classes or rdfs:subClassOf links pass the filters."
      const first = list.querySelector('[role="treeitem"]')
      if (first) first.tabIndex = 0
    },
    /** Open the rows down to node `i` and select it; `focus`: move the keyboard focus there too. */
    reveal(i, focus = true) {
      if (!hier || !(i >= 0)) return false
      const path = pathTo(hier, i)
      for (const p of path.slice(0, -1)) {
        const li = rowOf(p)
        if (li) setExpanded(li, true)
      }
      const li = rowOf(i)
      if (!li) return false
      choose(li, focus)
      li.scrollIntoView?.({ block: "nearest" })
      return true
    },
  }
}
