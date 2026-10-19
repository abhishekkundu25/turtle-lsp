// The Search view: find (and replace) text in every workspace file, as VS
// Code's. It searches the editor's models with Monaco's own find, so case,
// whole word and regex behave as in the editor's find widget, and replaces
// with undoable model edits, which the language server hears as typing.

const WORD_SEPARATORS = "`~!@#$%^&*()-=+[{]}\\|;:'\",.<>/?"
const MAX_PER_FILE = 2000
const MAX_TOTAL = 10000
const DELAY_MS = 150
const BEFORE = 28 // characters of the line shown before a match

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c])
const plural = (n, one) => `${n} ${one}${n === 1 ? "" : "s"}`

// Expand $&, $0..$99, $$ and \n, \t in a regex replace string, as VS Code does.
function expand(pattern, groups) {
  return pattern.replace(/\$(\$|&|\d{1,2})|\\([nt\\])/g, (all, d, e) => {
    if (e) return e === "n" ? "\n" : e === "t" ? "\t" : "\\"
    if (d === "$") return "$"
    const g = d === "&" ? 0 : Number(d)
    return g < groups.length ? (groups[g] ?? "") : all
  })
}

/**
 * Build the Search view inside `container`.
 * @param {object} o
 * @param {object} o.monaco
 * @param {HTMLElement} o.container the side-bar section to fill
 * @param {() => {path: string, model: object}[]} o.files the workspace's files
 * @param {(path: string, range: object) => void} o.reveal open `path` and select an LSP range
 * @param {(message: string) => void} o.toast
 * @returns {{ focus(): void, refresh(): void }}
 */
export function createSearch({ monaco, container, files, reveal, toast }) {
  container.innerHTML = `
    <div class="view-hd"><span class="view-title">Search</span><span class="sp"></span>
      <button class="ic" data-act="collapse" aria-label="Collapse All" title="Collapse All"><span class="codicon codicon-collapse-all"></span></button></div>
    <div class="sx-form">
      <button class="sx-twist" aria-label="Toggle Replace" title="Toggle Replace" aria-expanded="false"><span class="codicon codicon-chevron-right"></span></button>
      <div class="sx-fields">
        <div class="sx-box">
          <input class="sx-query" aria-label="Search" placeholder="Search" autocomplete="off" spellcheck="false">
          <button class="sx-opt" data-opt="matchCase" aria-label="Match Case" title="Match Case" aria-pressed="false"><span class="codicon codicon-case-sensitive"></span></button>
          <button class="sx-opt" data-opt="wholeWord" aria-label="Match Whole Word" title="Match Whole Word" aria-pressed="false"><span class="codicon codicon-whole-word"></span></button>
          <button class="sx-opt" data-opt="isRegex" aria-label="Use Regular Expression" title="Use Regular Expression" aria-pressed="false"><span class="codicon codicon-regex"></span></button>
        </div>
        <div class="sx-error" role="alert" hidden></div>
        <div class="sx-replace" hidden>
          <div class="sx-box"><input class="sx-with" aria-label="Replace" placeholder="Replace" autocomplete="off" spellcheck="false"></div>
          <button class="ic sx-all" aria-label="Replace All" title="Replace All" disabled><span class="codicon codicon-replace-all"></span></button>
        </div>
      </div>
    </div>
    <div class="sx-summary" aria-live="polite"></div>
    <div class="sx-results" role="tree" aria-label="Search results"></div>`
  const q = (sel) => container.querySelector(sel)
  const query = q(".sx-query")
  const withInput = q(".sx-with")
  const error = q(".sx-error")
  const summary = q(".sx-summary")
  const list = q(".sx-results")
  const replaceAll = q(".sx-all")

  const opts = { matchCase: false, wholeWord: false, isRegex: false }
  const collapsed = new Set()
  let results = [] // [{ path, model, version, matches: FindMatch[] }]
  let stale = true
  let timer

  const visible = () => !container.hidden
  function schedule() {
    stale = true
    clearTimeout(timer)
    if (visible()) timer = setTimeout(refresh, DELAY_MS)
  }

  // Follow every model: content edits, files added, removed or renamed.
  const watch = (m) => m.onDidChangeContent(schedule)
  for (const m of monaco.editor.getModels()) watch(m)
  monaco.editor.onDidCreateModel((m) => (watch(m), schedule()))
  monaco.editor.onWillDisposeModel(() => schedule())

  // The query's matches in one model (Monaco's find, as the find widget).
  function find(model) {
    return model.findMatches(query.value, false, opts.isRegex, opts.matchCase, opts.wholeWord ? WORD_SEPARATORS : null, opts.isRegex, MAX_PER_FILE)
  }

  function refresh() {
    clearTimeout(timer)
    stale = false
    results = []
    error.hidden = true
    query.parentElement.classList.remove("bad")
    if (opts.isRegex && query.value) {
      try {
        new RegExp(query.value, "m")
      } catch (e) {
        error.textContent = e.message.replace(/^Invalid regular expression: /, "")
        error.hidden = false
        query.parentElement.classList.add("bad")
        return render()
      }
    }
    if (query.value) {
      let total = 0
      for (const { path, model } of [...files()].sort((a, b) => a.path.localeCompare(b.path))) {
        if (model.isDisposed()) continue
        let matches
        try {
          matches = find(model)
        } catch (e) {
          error.textContent = e.message
          error.hidden = false
          return render()
        }
        if (!matches.length) continue
        results.push({ path, model, version: model.getVersionId(), matches })
        total += matches.length
        if (total >= MAX_TOTAL) break
      }
    }
    render()
  }

  function replacement(match) {
    return opts.isRegex ? expand(withInput.value, match.matches ?? []) : withInput.value
  }

  function preview(model, m) {
    const line = model.getLineContent(m.range.startLineNumber)
    const s = m.range.startColumn - 1
    const e = m.range.endLineNumber === m.range.startLineNumber ? m.range.endColumn - 1 : line.length
    // Start up to BEFORE characters back, at a word's start where one is near.
    let from = Math.max(0, s - BEFORE)
    if (from > 0) {
      const gap = line.slice(from, s).search(/\s\S/)
      if (gap >= 0) from += gap + 1
    }
    const head = (from > 0 ? "…" : "") + line.slice(from, s).trimStart()
    const hit = line.slice(s, e)
    const tail = line.slice(e, e + 200)
    const showReplace = !q(".sx-replace").hidden && (withInput.value || hit)
    const mid = showReplace && withInput.value !== "" ? `<del>${esc(hit)}</del><ins>${esc(replacement(m))}</ins>` : `<mark>${esc(hit)}</mark>`
    return `${esc(head)}${mid}${esc(tail)}`
  }

  function render() {
    const total = results.reduce((n, r) => n + r.matches.length, 0)
    summary.textContent = !query.value || !error.hidden ? "" : total ? `${plural(total, "result")} in ${plural(results.length, "file")}` : "No results found."
    replaceAll.disabled = !total
    const replacing = !q(".sx-replace").hidden
    list.innerHTML = results
      .map((r, fi) => {
        const name = r.path.slice(r.path.lastIndexOf("/") + 1)
        const dir = r.path.includes("/") ? r.path.slice(0, r.path.lastIndexOf("/")) : ""
        const open = !collapsed.has(r.path)
        const rows = open
          ? r.matches
              .map(
                (m, mi) =>
                  `<div class="sx-match" role="treeitem" aria-level="2" data-f="${fi}" data-m="${mi}" title="${esc(r.model.getLineContent(m.range.startLineNumber).trim().slice(0, 300))}"><span class="sx-line">${preview(r.model, m)}</span>${
                    replacing ? `<button class="ic sx-one" aria-label="Replace" title="Replace"><span class="codicon codicon-replace"></span></button>` : ""
                  }</div>`
              )
              .join("")
          : ""
        return `<div class="sx-file" role="treeitem" aria-level="1" aria-expanded="${open}" data-f="${fi}" data-file="${esc(r.path)}"><span class="codicon codicon-chevron-${open ? "down" : "right"}"></span><span class="sx-name">${esc(name)}</span><span class="sx-dir">${esc(dir)}</span><span class="sp"></span>${
          replacing ? `<button class="ic sx-one" aria-label="Replace All in File" title="Replace All"><span class="codicon codicon-replace-all"></span></button>` : ""
        }<span class="sx-count">${r.matches.length}</span></div>${rows}`
      })
      .join("")
  }

  // Replace `matches` of one result in one undoable edit.
  function apply(r, matches) {
    const edits = matches.map((m) => ({ range: m.range, text: replacement(m) }))
    r.model.pushStackElement()
    r.model.pushEditOperations([], edits, () => null)
    r.model.pushStackElement()
    return edits.length
  }

  function replaceEverything() {
    if (stale) refresh()
    let n = 0
    let files = 0
    for (const r of results) {
      if (r.model.isDisposed()) continue
      n += apply(r, r.matches)
      files++
    }
    refresh()
    if (n) toast(`Replaced ${plural(n, "occurrence")} in ${plural(files, "file")}`)
  }

  query.addEventListener("input", schedule)
  query.addEventListener("keydown", (e) => e.key === "Enter" && (e.preventDefault(), refresh()))
  withInput.addEventListener("input", () => render())
  withInput.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && e.altKey) (e.preventDefault(), replaceEverything())
  })
  replaceAll.addEventListener("click", replaceEverything)
  for (const b of container.querySelectorAll(".sx-opt")) {
    b.addEventListener("click", () => {
      opts[b.dataset.opt] = !opts[b.dataset.opt]
      b.setAttribute("aria-pressed", String(opts[b.dataset.opt]))
      b.classList.toggle("on", opts[b.dataset.opt])
      refresh()
    })
  }
  q(".sx-twist").addEventListener("click", (e) => {
    const box = q(".sx-replace")
    box.hidden = !box.hidden
    const b = e.currentTarget
    b.setAttribute("aria-expanded", String(!box.hidden))
    b.firstElementChild.className = `codicon codicon-chevron-${box.hidden ? "right" : "down"}`
    render()
    if (!box.hidden) withInput.focus()
  })
  q("[data-act=collapse]").addEventListener("click", () => {
    for (const r of results) collapsed.add(r.path)
    render()
  })
  list.addEventListener("click", (e) => {
    if (stale) return refresh()
    const row = e.target.closest("[data-f]")
    if (!row) return
    const r = results[row.dataset.f]
    if (!r) return
    if (r.model.isDisposed() || r.model.getVersionId() !== r.version) return refresh()
    const one = e.target.closest(".sx-one")
    if (row.classList.contains("sx-file")) {
      if (one) return (apply(r, r.matches), refresh())
      collapsed.has(r.path) ? collapsed.delete(r.path) : collapsed.add(r.path)
      return render()
    }
    const m = r.matches[row.dataset.m]
    if (one) return (apply(r, [m]), refresh())
    const x = m.range
    reveal(r.path, { start: { line: x.startLineNumber - 1, character: x.startColumn - 1 }, end: { line: x.endLineNumber - 1, character: x.endColumn - 1 } })
  })

  return {
    // Show the view's input, selected, with fresh results.
    focus() {
      if (stale) refresh()
      query.focus()
      query.select()
    },
    refresh,
  }
}
