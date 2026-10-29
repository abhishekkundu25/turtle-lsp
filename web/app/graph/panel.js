// The filter panel: search, colouring, depth and cap, what to show, then the
// kinds, files, namespaces and predicates from the server's facets. Every
// control is a labelled native input, so the keyboard reaches all of them.
// It changes `state` (filters.js) in place and says what changed.

import { h } from "./el.js"
import { KINDS, commonRoot, fileLabel, middleCut } from "./model.js"
import { CAPS, COLOR_BY, DEPTHS, ruleOf, setRule } from "./filters.js"

const RULES = [["", "any"], ["include", "only"], ["exclude", "hide"]]
const LIST_MAX = 60
// A file name in the list, in characters: the 240 px panel less the box and
// the count leaves about 170 px, some 26 characters of 11 px monospace. A
// wider panel (stacked on a narrow host) shows the same; CSS still ends a
// line that does not fit with an ellipsis.
const FILE_CHARS = 26

/**
 * @param {HTMLElement} el the panel's element
 * @param {ReturnType<import("./filters.js").defaultState>} state
 * @param {object} on
 * @param {(what: "filter" | "color") => void} on.change
 * @param {(query: string) => void} on.search
 * @param {(query: string) => void} on.enter Enter in the search box
 * @param {(index: number) => void} on.pick a search result chosen
 */
export function createFilterPanel(el, state, on, caps = CAPS) {
  const doc = el.ownerDocument
  const F = state.filters
  const facets = { files: new Map(), namespaces: new Map(), predicates: new Map() }
  const names = { namespaces: new Map(), predicates: new Map() }

  const select = (key, label, options, value) =>
    h(doc, "label", { class: "graph-field" }, h(doc, "span", { text: label }),
      h(doc, "select", { "data-g": key, "data-key": key }, options.map(([v, t]) => h(doc, "option", { value: String(v), selected: String(v) === String(value) }, t))))
  const check = (group, value, checked, label, count, title) =>
    h(doc, "label", { class: "graph-check", title },
      h(doc, "input", { type: "checkbox", "data-g": group, "data-key": `${group}:${value}`, value, checked }),
      h(doc, "span", { class: "graph-check-l", text: label }),
      count != null ? h(doc, "span", { class: "graph-count", text: count.toLocaleString("en") }) : null)

  const search = h(doc, "input", { type: "search", class: "graph-search", "aria-label": "Search nodes", placeholder: "Search, Enter focuses", autocomplete: "off", "data-key": "search" })
  const results = h(doc, "div", { class: "graph-results", role: "group", "aria-label": "Search results" })
  const kinds = h(doc, "div", { class: "graph-list" })
  const lists = { files: h(doc, "div", { class: "graph-list" }), namespaces: h(doc, "div", { class: "graph-list" }), predicates: h(doc, "div", { class: "graph-list" }) }
  const section = (title, body, open) => h(doc, "details", { class: "graph-sec", open }, h(doc, "summary", { text: title }), body)
  // All / None for the files: a list of fifty is one click from one file.
  const allNone = h(doc, "div", { class: "graph-allnone", role: "group", "aria-label": "Select files" },
    h(doc, "button", { type: "button", class: "graph-btn", "data-files": "all", "data-key": "files:all", text: "All" }),
    h(doc, "button", { type: "button", class: "graph-btn", "data-files": "none", "data-key": "files:none", text: "None" }))

  el.replaceChildren(
    h(doc, "div", { class: "graph-sec graph-find" }, search, results),
    h(doc, "div", { class: "graph-sec graph-fields" },
      select("colorBy", "Colour by", COLOR_BY.map((c) => [c, c]), state.colorBy),
      select("depth", "Depth around selection", DEPTHS.map((d) => [d, d ? `${d} hop${d > 1 ? "s" : ""}` : "whole graph"]), F.depth),
      select("maxNodes", "Node cap", caps.map((c) => [c, c.toLocaleString("en")]), F.maxNodes),
      check("literals", "on", F.literals, "Literals"),
      check("types", "on", F.types, "rdf:type edges")),
    section("Kinds", kinds, true),
    section("Files", [allNone, lists.files], true),
    section("Namespaces", lists.namespaces, false),
    section("Predicates", lists.predicates, false))

  function renderKinds() {
    kinds.replaceChildren(...KINDS.map((k) => check("kind", k, !F.kinds || F.kinds.includes(k), k)))
  }

  // A list from the facets (most first), plus anything the filters name that the facets no longer do.
  function renderList(key) {
    const box = lists[key]
    const map = facets[key]
    if (key === "files") {
      const uris = [...new Set([...map.keys(), ...(F.files ?? [])])]
      const root = commonRoot(uris)
      box.replaceChildren(...(uris.length ? uris.slice(0, LIST_MAX).map((u) => check("file", u, !F.files || F.files.includes(u), middleCut(fileLabel(u, root), FILE_CHARS), map.get(u), u)) : [h(doc, "p", { class: "graph-none", text: "No files" })]))
      return
    }
    const rules = F[key]
    const iris = [...new Set([...map.keys(), ...rules.include, ...rules.exclude])]
    if (!iris.length) return box.replaceChildren(h(doc, "p", { class: "graph-none", text: "None" }))
    box.replaceChildren(...iris.slice(0, LIST_MAX).map((iri) => {
      const name = names[key].get(iri) || iri
      return h(doc, "label", { class: "graph-rule", title: iri },
        h(doc, "span", { class: "graph-check-l", text: name }),
        map.has(iri) ? h(doc, "span", { class: "graph-count", text: map.get(iri).toLocaleString("en") }) : null,
        h(doc, "select", { "data-g": key, "data-key": `${key}:${iri}`, "data-v": iri, "aria-label": `${name}: any, only or hide` },
          RULES.map(([v, t]) => h(doc, "option", { value: v, selected: ruleOf(rules, iri) === v }, t))))
    }))
  }

  el.addEventListener("change", (e) => {
    const t = e.target
    const g = t.dataset?.g
    if (!g) return
    if (g === "colorBy") return (state.colorBy = t.value), on.change("color")
    if (g === "depth") F.depth = Number(t.value)
    else if (g === "maxNodes") F.maxNodes = Number(t.value)
    else if (g === "literals") F.literals = t.checked
    else if (g === "types") F.types = t.checked
    else if (g === "kind") F.kinds = toggle(F.kinds ?? KINDS, t.value, t.checked, KINDS)
    else if (g === "file") F.files = toggle(F.files ?? [...facets.files.keys()], t.value, t.checked, [...facets.files.keys()])
    else if (g === "namespaces" || g === "predicates") setRule(F[g], t.dataset.v, t.value)
    else return
    on.change("filter")
  })
  allNone.addEventListener("click", (e) => {
    const which = e.target.closest("[data-files]")?.dataset.files
    if (!which) return
    const next = which === "all" ? null : []
    if (JSON.stringify(next) === JSON.stringify(F.files)) return
    F.files = next
    renderList("files")
    on.change("filter")
  })
  // The list without (or with) `value`; null when it holds every one of `all`.
  function toggle(list, value, keep, all) {
    const next = keep ? [...new Set([...list, value])] : list.filter((v) => v !== value)
    return all.length && all.every((v) => next.includes(v)) ? null : next
  }

  search.addEventListener("input", () => on.search(search.value))
  search.addEventListener("keydown", (e) => {
    if (e.key === "Enter") e.preventDefault(), on.enter(search.value)
    else if (e.key === "ArrowDown") results.querySelector("button")?.focus()
  })
  results.addEventListener("click", (e) => {
    const b = e.target.closest("button[data-i]")
    if (b) on.pick(Number(b.dataset.i))
  })

  renderKinds()
  for (const key of Object.keys(lists)) renderList(key)

  return {
    /** New facets from an answer: counts update, earlier entries stay. */
    update(f) {
      const active = doc.activeElement?.dataset?.key
      facets.files = new Map(f.files.map((v) => [v.uri, v.nodes]))
      facets.namespaces = new Map(f.namespaces.map((v) => [v.iri, v.nodes]))
      facets.predicates = new Map(f.predicates.map((v) => [v.iri, v.edges]))
      for (const v of f.namespaces) if (v.prefix) names.namespaces.set(v.iri, `${v.prefix}:`)
      for (const v of f.predicates) if (v.name) names.predicates.set(v.iri, v.name)
      renderKinds()
      for (const key of Object.keys(lists)) renderList(key)
      if (active) el.querySelector(`[data-key="${CSS.escape(active)}"]`)?.focus()
    },
    /** Show search results: `[{ index, name, kind }]`. */
    results(list) {
      results.replaceChildren(...list.map((r) => h(doc, "button", { type: "button", class: "graph-result", "data-i": r.index }, h(doc, "span", { class: "graph-check-l", text: r.name }), h(doc, "span", { class: "graph-count", text: r.kind }))))
    },
    get search() {
      return search.value
    },
  }
}
