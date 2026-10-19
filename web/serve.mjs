// Serves web/app on http://127.0.0.1:4174 (or PORT) for local use. Every
// response says no-cache, so a rebuilt engine.wasm reaches the page's worker
// on the next reload instead of the browser's heuristic cache.
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { fileURLToPath } from "node:url"

const app = path.join(path.dirname(fileURLToPath(import.meta.url)), "app")
const port = Number(process.env.PORT ?? 4174)
const TYPES = { ".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".wasm": "application/wasm", ".ttf": "font/ttf", ".json": "application/json", ".svg": "image/svg+xml" }

http
  .createServer((req, res) => {
    let rel
    try {
      rel = decodeURIComponent(new URL(req.url, "http://x").pathname)
    } catch {
      return res.writeHead(400).end()
    }
    if (rel.endsWith("/")) rel += "index.html"
    const file = path.join(app, path.normalize(rel))
    if (!file.startsWith(app) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) return res.writeHead(404).end()
    res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream", "cache-control": "no-cache" })
    fs.createReadStream(file).pipe(res)
  })
  .listen(port, "127.0.0.1", () => console.log(`http://127.0.0.1:${port}/`))
