// SPARQL queries in the editor: a Run button in an editor group's breadcrumbs
// bar when its file is a query (.rq, .sparql), and the results under the
// editor: a table, true or false, or the constructed graph as Turtle. The
// query runs in its own engine worker (worker.js), so typing is never slowed.

import { currentSettings } from "./editor-lsp.js"

export const isQuery = (path) => /\.(rq|sparql)$/i.test(path ?? "")

const esc = (s) => String(s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c])
const plural = (n, what) => `${n.toLocaleString("en")} ${what}${n === 1 ? "" : "s"}`

// `files()`: the workspace as [{ name, text }]; `reveal(path, line, column)`
// puts the cursor at a 1-based place in `path`'s editor; `settings()`: the
// user's settings (the object Settings' `values()` gives, or the server's
// configuration), by default what the page last sent the server. A query
// runs under them: their prefixes, and `query.timeoutSeconds` and
// `query.maxRows` (30 seconds and 1000 rows unless set).
export function createQueryPane({ files, reveal, settings = currentSettings }) {
  let worker = null
  let seq = 0
  const pending = new Map()

  // A worker that failed to load, died or whose engine crashed answers nothing
  // more: fail what waits on it, and start a new one for the next run.
  function drop(w, message) {
    if (worker !== w) return
    worker = null
    w.terminate()
    for (const p of pending.values()) p.reject(new Error(message))
    pending.clear()
  }

  function call(payload) {
    if (!worker) {
      const w = (worker = new Worker(new URL("worker.js", import.meta.url), { type: "module" }))
      w.onmessage = ({ data }) => {
        const p = pending.get(data.id)
        pending.delete(data.id)
        if ("error" in data) p?.reject(new Error(data.error))
        else p?.resolve(data.ok)
        if (data.crashed) drop(w, "the query engine crashed")
      }
      w.onerror = w.onmessageerror = (e) => {
        e.preventDefault()
        drop(w, "the query engine stopped" + (e.message ? `: ${e.message}` : ""))
      }
    }
    const id = ++seq
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject })
      worker.postMessage({ id, ...payload })
    })
  }

  // Adds the button and the pane to group `g` ({ el, editor }); `run` runs
  // the group's file.
  function attach(g, run) {
    const crumbs = g.el.querySelector(".crumbs")
    const button = document.createElement("button")
    button.className = "qrun"
    button.title = "Run Query (Ctrl+Enter)"
    button.setAttribute("aria-label", "Run Query")
    button.innerHTML = `<span class="codicon codicon-play" aria-hidden="true"></span><span class="qrun-label">Run</span>`
    button.hidden = true
    button.addEventListener("click", run)
    crumbs.append(button)
    const pane = document.createElement("section")
    pane.className = "qpane"
    pane.hidden = true
    pane.setAttribute("aria-label", "Query results")
    pane.innerHTML = `<div class="qpane-hd"><span class="view-title">Results</span><span class="qpane-file"></span><span class="qpane-status" role="status"></span><span class="sp"></span><button class="ic" aria-label="Close Results" title="Close Results"><span class="codicon codicon-close"></span></button></div><div class="qpane-body" tabindex="0"></div>`
    pane.querySelector(".ic").addEventListener("click", () => {
      g.query = null
      pane.hidden = true
      g.editor.focus()
    })
    pane.querySelector(".qpane-body").addEventListener("click", (e) => {
      const at = e.target.closest("[data-line]")
      if (at && g.query) reveal(g.query.path, Number(at.dataset.line), Number(at.dataset.column))
    })
    g.el.querySelector(".editor-host").after(pane)
    g.run = { button, pane }
  }

  // Shows the button when the group's file is a query, and the results when
  // they are its file's.
  function update(g) {
    if (!g.run) return
    g.run.button.hidden = !isQuery(g.active)
    g.run.pane.hidden = !g.query || g.query.path !== g.active
  }

  async function run(g) {
    const path = g.active
    if (!isQuery(path) || g.query?.running) return
    const { button, pane } = g.run
    const ticket = { path, running: true }
    g.query = ticket
    const body = pane.querySelector(".qpane-body")
    pane.querySelector(".qpane-file").textContent = path
    pane.querySelector(".qpane-status").textContent = "Running…"
    body.setAttribute("aria-busy", "true")
    button.disabled = true
    update(g)
    let answer
    try {
      answer = await call({ op: "query", files: files(), query: path, settings: settings() })
    } catch (e) {
      answer = { kind: "error", message: e.message, line: null, column: null }
    }
    ticket.running = false
    // Unless the pane was closed, or another run of the group took over.
    if (g.query === ticket) {
      show(pane, answer)
      body.removeAttribute("aria-busy")
    }
    button.disabled = Boolean(g.query?.running)
  }

  return { attach, update, run }
}

function show(pane, a) {
  const body = pane.querySelector(".qpane-body")
  const ms = a.elapsedMs == null ? "" : ` · ${Math.round(a.elapsedMs).toLocaleString("en")} ms`
  let status = ""
  let html = ""
  if (a.kind === "select") {
    status = a.rows.length < a.total ? `showing ${a.rows.length.toLocaleString("en")} of ${plural(a.total, "row")}` : plural(a.total, "row")
    const head = a.variables.map((v) => `<th scope="col">?${esc(v)}</th>`).join("")
    const rows = a.rows.map((r) => `<tr>${r.map((c) => (c == null ? `<td class="unbound"></td>` : `<td>${esc(c)}</td>`)).join("")}</tr>`).join("")
    html = a.total ? `<table class="qtable"><thead><tr>${head}</tr></thead><tbody>${rows}</tbody></table>` : `<p class="qempty">No results.</p>`
  } else if (a.kind === "ask") {
    status = "ask"
    html = `<p class="qbool">${a.boolean}</p>`
  } else if (a.kind === "construct") {
    status = a.shown < a.total ? `showing ${a.shown.toLocaleString("en")} of ${plural(a.total, "triple")}` : plural(a.total, "triple")
    html = a.total ? `<pre class="qturtle" aria-label="Constructed graph (Turtle)">${esc(a.turtle)}</pre>` : `<p class="qempty">No triples.</p>`
  } else {
    status = "error"
    const where = a.line != null ? `<button class="qloc" data-line="${a.line}" data-column="${a.column}" title="Go to ${a.line}:${a.column} in the query">${a.line}:${a.column}</button> ` : ""
    html = `<p class="qerror"><span class="codicon codicon-error" aria-hidden="true"></span> ${where}<span class="msg">${esc(a.message)}</span></p>`
  }
  pane.querySelector(".qpane-status").textContent = status + ms
  body.innerHTML = html
  body.scrollTop = 0
}
