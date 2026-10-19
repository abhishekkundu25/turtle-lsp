// Builds the toolbox's engine (web/engine) for the browser and copies it
// next to the page as web/app/engine.wasm.
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
