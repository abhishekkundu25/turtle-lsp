// Serves web/app on http://127.0.0.1:4174 (or PORT) for local use. Every
// response says no-cache, so a rebuilt engine.wasm reaches the page's worker
// on the next reload instead of the browser's heuristic cache.
import { listen } from "./test/static-server.mjs"

const { origin } = await listen({ port: Number(process.env.PORT ?? 4174), headers: { "cache-control": "no-cache" } })
console.log(`${origin}/`)
