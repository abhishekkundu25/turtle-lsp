// The engine runs here, off the page's thread. Messages in: { id, op, ...payload }
// (payload: { files } for most operations); out: { id, ok, ms } or
// { id, error, crashed }.
import { createEngine } from "./engine.js"

const engine = fetch(new URL("engine.wasm", import.meta.url))
  .then((r) => (r.ok ? r.arrayBuffer() : Promise.reject(new Error(`engine.wasm: HTTP ${r.status}`))))
  .then(createEngine)

self.onmessage = async ({ data: { id, op, ...payload } }) => {
  try {
    const e = await engine
    const t0 = performance.now()
    const ok = e.call(op, payload)
    self.postMessage({ id, ok, ms: performance.now() - t0 })
  } catch (err) {
    self.postMessage({ id, error: String(err?.message ?? err), crashed: Boolean(err?.crashed) })
  }
}
