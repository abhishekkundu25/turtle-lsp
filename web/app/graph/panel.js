// The filter panel: search, colouring, depth and cap, what to show, then the
// kinds, files, namespaces and predicates from the server's facets. Every
// control is a labelled native input, so the keyboard reaches all of them.
// It changes `state` (filters.js) in place and says what changed. The
// controls only the graph and focus modes use carry `data-for="graph"`;
// graph.css hides them in Schema, which keeps the search and the files.
// Also the class inspector: what the info panel shows for a class in Schema.

import { h } from "./el.js"
import { KINDS, commonRoot, fileLabel, middleCut } from "./model.js"
import { CAPS, COLOR_BY, DEPTHS, ruleOf, setRule } from "./filters.js"
import { STATE_TEXT, classText, declaredForText, rowBadge } from "./schema.js"

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
  const section = (title, body, open, only) => h(doc, "details", { class: "graph-sec", open, "data-for": only }, h(doc, "summary", { text: title }), body)
  // All / None for the files: a list of fifty is one click from one file.
  const allNone = h(doc, "div", { class: "graph-allnone", role: "group", "aria-label": "Select files" },
    h(doc, "button", { type: "button", class: "graph-btn", "data-files": "all", "data-key": "files:all", text: "All" }),
    h(doc, "button", { type: "button", class: "graph-btn", "data-files": "none", "data-key": "files:none", text: "None" }))

  el.replaceChildren(
    h(doc, "div", { class: "graph-sec graph-find" }, search, results),
    h(doc, "div", { class: "graph-sec graph-fields", "data-for": "graph" },
      select("colorBy", "Colour by", COLOR_BY.map((c) => [c, c]), state.colorBy),
      select("depth", "Depth around selection", DEPTHS.map((d) => [d, d ? `${d} hop${d > 1 ? "s" : ""}` : "whole graph"]), F.depth),
      select("maxNodes", "Node cap", caps.map((c) => [c, c.toLocaleString("en")]), F.maxNodes),
      check("literals", "on", F.literals, "Literals"),
      check("types", "on", F.types, "rdf:type edges")),
    section("Kinds", kinds, true, "graph"),
    section("Files", [allNone, lists.files], true),
    section("Namespaces", lists.namespaces, false, "graph"),
    section("Predicates", lists.predicates, false, "graph"))

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
    /** Show search results: `[{ index, name, kind, title? }]`; choosing one calls `on.pick(index)`. */
    results(list) {
      results.replaceChildren(...list.map((r) => h(doc, "button", { type: "button", class: "graph-result", "data-i": r.index, title: r.title }, h(doc, "span", { class: "graph-check-l", text: r.name }), h(doc, "span", { class: "graph-count", text: r.kind }))))
    },
    get search() {
      return search.value
    },
  }
}

const SEV = { error: "graph-sev1", warning: "graph-sev2", "": "graph-sev0" }
const KIND_TEXT = { class: "class", imported: "imported class", undefined: "never defined" }
// A source in the inspector, in characters: the panel is about 320 px.
const SOURCE_CHARS = 30

/**
 * The info panel's content for class `i` of a Schema answer (schema.js
 * schemaModel): its rows as declared vs. used, each row's sources, and its
 * problems, each with a button that opens the place to fix it. Buttons carry
 * `data-act`: "close", "reveal" (the class), "src" (`data-src`: an index into
 * the returned `sources`), "row" (`data-row`), "class" (`data-c`).
 * @param {Document} doc
 * @param {ReturnType<import("./schema.js").schemaModel>} model
 * @param {number} i
 * @param {object} [o]
 * @param {number} [o.lit] the row to light
 * @param {boolean} [o.canReveal] the host can open a location
 * @param {boolean} [o.loading] the class's places are on their way: a
 *   problem that has places says so, and its Open waits for them
 * @returns {{ nodes: HTMLElement[], sources: { uri: string, range: object }[] }}
 */
export function classInspector(doc, model, i, { lit = -1, canReveal = false, loading = false } = {}) {
  const c = model.classes[i]
  const sources = []
  const root = commonRoot([c.uri, ...c.rows.flatMap((r) => [...r.declared, ...r.examples].map((d) => d.uri))].filter(Boolean))
  const where = (d) => middleCut(fileLabel(d.uri, root), SOURCE_CHARS) + (d.range?.start ? `:${d.range.start.line + 1}` : "")
  const source = (d) => {
    sources.push({ uri: d.uri, range: d.range })
    return sources.length - 1
  }
  const f = (v) => v.toLocaleString("en")
  const rowItem = (r, k) => {
    const badge = rowBadge(r)
    return h(doc, "li", { class: "schema-i-row" + (k === lit ? " is-lit" : ""), "data-row": k },
      h(doc, "div", { class: "schema-i-line" },
        h(doc, "button", { type: "button", class: "schema-i-name", "data-act": "row", "data-row": k, title: r.property, text: r.name }),
        h(doc, "span", { class: "schema-i-use", title: `${f(r.used)} of ${f(r.of)} instances use it`, text: `${f(r.used)}/${f(r.of)}` }),
        h(doc, "span", { class: `schema-i-state ${SEV[badge?.sev ?? ""]}`, text: badge ? badge.text : r.state === "ok" ? "ok" : STATE_TEXT[r.state] })),
      r.link && r.target >= 0 ? h(doc, "button", { type: "button", class: "schema-i-link", "data-act": "class", "data-c": r.target, title: r.range || model.classes[r.target].id, text: `→ ${r.rangeName || model.classes[r.target].name}` })
        : r.rangeName ? h(doc, "div", { class: "schema-i-range", title: r.range, text: r.rangeName }) : null,
      h(doc, "div", { class: "schema-i-src" },
        r.declared.length
          ? r.declared.map((d) => h(doc, "button", { type: "button", class: "schema-src", "data-act": "src", "data-src": source(d), disabled: !canReveal, title: `${d.how}: ${d.uri}`, "aria-label": `Open ${d.how} declaration at ${where(d)}`, text: `${where(d)} · ${d.how}` }))
          : h(doc, "span", { class: "graph-none", text: declaredForText(r, classText(c)) })))
  }
  // What a problem's places are: the instances missing a property, the
  // bad values, the uses of an undeclared one, or the values outside a
  // conflicting shape (never a declaration).
  const PLACE = { missing: "instance", badValues: "value", undeclared: "use", conflict: "value" }
  const places = (p, n) => `${f(n)} ${PLACE[p.kind] ?? "place"}${n === 1 ? "" : "s"}`
  // The places a problem lists, behind "Show N …": each opens its place.
  const examples = (p) => {
    const key = `${p.row}:${p.kind}`
    const more = p.count - p.examples.length
    return h(doc, "details", { class: "schema-ex", "data-key": key },
      h(doc, "summary", { "aria-label": `Show ${places(p, p.examples.length)}: ${p.text}`, text: `Show ${places(p, p.examples.length)}` }),
      h(doc, "ul", { class: "schema-ex-list", "aria-label": `Places: ${p.text}` },
        p.examples.map((e, j) => h(doc, "li", {},
          h(doc, "button", {
            type: "button", class: "schema-src schema-ex-item", "data-act": "src", "data-src": canReveal ? source(e) : null, "data-key": `${key}:${j}`, disabled: !canReveal,
            title: canReveal ? e.uri : "This host cannot open files", "aria-label": `Open ${e.name} at ${where(e)}`, text: `${e.name} · ${where(e)}`,
          }))),
        more > 0 ? h(doc, "li", { class: "schema-ex-more", text: `+${f(more)} more` }) : null))
  }
  const problem = (p) => {
    // Its places are on their way: Open waits for the first.
    const waits = loading && PLACE[p.kind] && p.count > 0 && !p.examples.length
    const at = p.at && canReveal && !waits ? source(p.at) : -1
    return h(doc, "li", { class: "schema-problem" },
      h(doc, "span", { class: `schema-dot ${SEV[p.sev]}`, "aria-hidden": "true", text: "●" }),
      h(doc, "div", { class: "schema-problem-tx" }, h(doc, "div", { text: p.text }), p.detail ? h(doc, "div", { class: "schema-problem-d", text: p.detail }) : null,
        p.examples.length ? examples(p) : waits ? h(doc, "div", { class: "schema-problem-d schema-ex-loading", text: "Loading places…" }) : null),
      h(doc, "button", {
        type: "button", class: "graph-btn", "data-act": "src", "data-src": at >= 0 ? at : null, disabled: at < 0,
        "aria-label": `Open: ${p.text}`,
        title: waits ? "Loading places…" : !p.at ? "Nothing declares it: no place to open" : !canReveal ? "This host cannot open files" : `Open ${where(p.at)}`,
        text: p.examples.length || waits ? `Open first ${PLACE[p.kind] ?? "place"}` : "Open declaration",
      }))
  }
  const nodes = [
    h(doc, "div", { class: "graph-info-hd" },
      h(doc, "h3", { class: "graph-info-name", title: c.id, text: c.label || c.name }),
      h(doc, "span", { class: "graph-badge", text: KIND_TEXT[c.kind] }),
      h(doc, "button", { type: "button", class: "graph-x", "data-act": "close", "aria-label": "Close", title: "Close", text: "×" })),
    h(doc, "div", { class: "graph-info-id", text: c.id }),
    c.uri ? h(doc, "div", { class: "graph-info-file", title: c.uri, text: where(c) }) : null,
    h(doc, "div", { class: "schema-i-meta", text: `${f(c.instances)} instance${c.instances === 1 ? "" : "s"} · ${c.problemCount ? `${f(c.problemCount)} problem${c.problemCount === 1 ? "" : "s"}` : "no problems"}` }),
    h(doc, "div", { class: "graph-actions" },
      h(doc, "button", { type: "button", class: "graph-btn", "data-act": "reveal", disabled: !(canReveal && c.uri && c.range), text: "Go to definition" })),
    // The work first: the problems, then the rows.
    h(doc, "h4", { class: "graph-info-sec", text: c.problems.length ? `Problems (${c.problems.length})` : "No problems" }),
    c.problems.length ? h(doc, "ul", { class: "schema-problems", "aria-label": "Problems" }, c.problems.map(problem)) : null,
    h(doc, "h4", { class: "graph-info-sec", text: `Properties: declared vs. used (${c.rows.length + c.moreRows})` }),
    c.rows.length ? h(doc, "ul", { class: "schema-i-rows", "aria-label": "Properties" }, c.rows.map(rowItem)) : h(doc, "p", { class: "graph-none", text: "No properties" }),
    c.moreRows ? h(doc, "p", { class: "graph-none", text: `${f(c.moreRows)} more not listed` }) : null,
  ].filter(Boolean)
  return { nodes, sources }
}
