// The webview side of the graph panel (src/graph.ts): mounts the shared view
// and carries its requests and reveals to the extension over postMessage.
// The filters and mode live in the webview's state, which outlasts hiding
// the panel. Bundled with the view and its styles into out/graph/ by "build".
import { createGraphView } from "../../../web/app/graph/view.js"
import "../../../web/app/graph/graph.css"
import "./graph-webview.css"

const vscode = acquireVsCodeApi()
const pending = new Map()
const diagnostics = new Map()
let nextId = 0

function request(method, params) {
  const id = ++nextId
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    vscode.postMessage({ type: "request", id, method, params })
  })
}

const container = document.getElementById("graph")
const view = createGraphView({
  container,
  request,
  reveal: (uri, range) => vscode.postMessage({ type: "reveal", uri, range }),
  diagnostics: (uri) => diagnostics.get(uri) ?? [],
  store: {
    get: (key) => vscode.getState()?.[key],
    set: (key, value) => vscode.setState({ ...vscode.getState(), [key]: value }),
  },
})

// VS Code changes the body's theme class and the --vscode-… variables on the
// root element; the canvas colours are read again once both have landed.
let themeFrame = 0
function themeChanged() {
  if (!themeFrame) themeFrame = requestAnimationFrame(() => ((themeFrame = 0), view.setTheme()))
}
new MutationObserver(themeChanged).observe(document.body, { attributes: true, attributeFilter: ["class"] })
new MutationObserver(themeChanged).observe(document.documentElement, { attributes: true, attributeFilter: ["style"] })

// After each draw the view stamps data-draw-ms on its root: tell the extension
// the status line when it changes (the integration tests wait for it).
let lastStatus = null
new MutationObserver(() => {
  const status = container.querySelector(".graph-status")?.textContent ?? ""
  if (status === lastStatus) return
  lastStatus = status
  vscode.postMessage({ type: "drawn", status })
}).observe(container, { attributes: true, subtree: true, attributeFilter: ["data-draw-ms"] })

window.addEventListener("message", (event) => {
  const m = event.data
  if (m?.type === "response") {
    const p = pending.get(m.id)
    if (!p) return
    pending.delete(m.id)
    for (const [uri, list] of Object.entries(m.diagnostics ?? {})) diagnostics.set(uri, list)
    if (m.error) p.reject(new Error(m.error))
    else p.resolve(m.result)
  } else if (m?.type === "refresh") {
    view.refresh()
  } else if (m?.type === "cursor" && typeof m.uri === "string" && m.position) {
    // Follow cursor: the view waits until the cursor rests, and ignores it while the toggle is off.
    view.follow?.({ uri: m.uri, position: m.position })
    vscode.postMessage({ type: "cursorSeen" })
  } else if (m?.type === "theme") {
    themeChanged()
  }
})
