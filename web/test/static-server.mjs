// The one static-file server: web/serve.mjs serves the app with it, and the
// browser tests serve it from a free port.
import fs from "node:fs"
import http from "node:http"
import path from "node:path"
import { fileURLToPath } from "node:url"

export const APP = path.join(path.dirname(fileURLToPath(import.meta.url)), "..", "app")

export const TYPES = {
  ".html": "text/html",
  ".js": "text/javascript",
  ".mjs": "text/javascript",
  ".css": "text/css",
  ".wasm": "application/wasm",
  ".ttf": "font/ttf",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".txt": "text/plain",
}

/** The file under `root` that the request path `rel` names, or null. */
export function resolveIn(root, rel) {
  if (rel.endsWith("/")) rel += "index.html"
  const file = path.join(root, path.normalize(rel))
  const inside = path.relative(root, file)
  if (inside.startsWith("..") || path.isAbsolute(inside)) return null
  try {
    return fs.statSync(file).isFile() ? file : null
  } catch {
    return null
  }
}

/**
 * An HTTP server for the files under `root` (default: web/app).
 * @param {{ root?: string, headers?: Record<string, string> }} [options] `headers`: sent with every file
 */
export function createStaticServer({ root = APP, headers = {} } = {}) {
  return http.createServer((req, res) => {
    let rel
    try {
      rel = decodeURIComponent(new URL(req.url, "http://x").pathname)
    } catch {
      return res.writeHead(400).end()
    }
    const file = resolveIn(root, rel)
    if (!file) return res.writeHead(404).end()
    const stream = fs.createReadStream(file)
    // A file that cannot be read after the headers went out cannot get a
    // status any more: end the connection instead of crashing the server.
    stream.on("error", () => (res.headersSent ? res.destroy() : res.writeHead(500).end()))
    stream.on("open", () => {
      res.writeHead(200, { "content-type": TYPES[path.extname(file)] ?? "application/octet-stream", ...headers })
      stream.pipe(res)
    })
    res.on("close", () => stream.destroy())
  })
}

/**
 * Serve `root` on a free local port (or `port`).
 * @returns {Promise<{ server: http.Server, origin: string, close(): Promise<void> }>}
 */
export async function listen({ port = 0, ...options } = {}) {
  const server = createStaticServer(options)
  await new Promise((resolve, reject) => {
    server.once("error", reject)
    server.listen(port, "127.0.0.1", resolve)
  })
  return {
    server,
    origin: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((r) => (server.closeAllConnections?.(), server.close(r))),
  }
}
