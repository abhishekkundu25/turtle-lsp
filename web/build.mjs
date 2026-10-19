// Builds the toolbox's engine (web/engine) for the browser and copies it
// next to the page as web/app/engine.wasm, with the Monaco editor (from the
// npm package) as web/app/vendor/monaco.
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const web = path.dirname(fileURLToPath(import.meta.url))
const engine = path.join(web, "engine")
execFileSync("cargo", ["build", "--release", "--locked"], { cwd: engine, stdio: "inherit" })
const built = path.join(engine, "target", "wasm32-unknown-unknown", "release", "turtle_lsp_web.wasm")
fs.copyFileSync(built, path.join(web, "app", "engine.wasm"))
console.log(`web/app/engine.wasm: ${(fs.statSync(built).size / 1e6).toFixed(1)} MB`)

const monaco = path.join(web, "..", "node_modules", "monaco-editor", "min", "vs")
const vendor = path.join(web, "app", "vendor", "monaco", "vs")
fs.rmSync(path.dirname(vendor), { recursive: true, force: true })
fs.cpSync(monaco, vendor, { recursive: true, filter: (src) => !/nls\.messages\.[\w-]+\.js$/.test(src) })
console.log("web/app/vendor/monaco: Monaco " + JSON.parse(fs.readFileSync(path.join(monaco, "..", "..", "package.json"), "utf8")).version)
