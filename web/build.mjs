// Builds the toolbox's engine (web/engine) for the browser and copies it
// next to the page as web/app/engine.wasm, with the Monaco editor (from the
// npm package) as web/app/vendor/monaco, and monaco-vim beside it. The
// project's LICENSE and THIRD_PARTY_NOTICES.md (the crates and data in the
// engine) go to web/app/vendor/ too, so the site ships its notices.
//
//   node web/build.mjs                 build everything
//   node web/build.mjs --skip-engine   copy the vendor files and check the
//                                      size of the engine.wasm already there
//
// The engine's size is checked against a budget (gzip, as a server sends
// it): the build fails over it, so growth is a decision.
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import zlib from "node:zlib"
import { fileURLToPath, pathToFileURL } from "node:url"

/** The most the engine may weigh gzipped, in bytes. It weighs about 3.71 MB with TurbOWL, queries over inferred triples, RDF/XML rename, and completion in OWL Functional syntax, OWL/XML and RDF/XML (3.61 MB before rename and these completions); the budget leaves about 2.4%. */
export const WASM_GZIP_BUDGET = 3_800_000

/** Monaco's language services the editor never starts: it opens Turtle, TriG, N-Triples, N-Quads, SPARQL and JSON-LD (JSON). Each with the prefix of its chunk files. */
const UNUSED_LANGUAGES = { typescript: "ts", css: "css", html: "html" }

/**
 * Whether the Monaco file `src` is copied. Not copied: the message catalogues
 * in other languages (`vs/nls/`), and an unused language service: its folder in
 * `vs/language/`, its mode (`vs/<prefix>Mode-<hash>.js`) and its worker
 * (`vs/assets/<prefix>.worker-<hash>.js`). The small `vs/<prefix>.worker-<hash>.js`
 * that `editor.main.js` imports for each service stays.
 */
export function wanted(src) {
  const p = src.split(path.sep).join("/")
  if (p.endsWith("/vs/nls") || p.includes("/vs/nls/")) return false
  return !Object.entries(UNUSED_LANGUAGES).some(([l, prefix]) =>
    p.includes(`/vs/language/${l}/`) || p.endsWith(`/vs/language/${l}`) || new RegExp(`/vs/(?:${prefix}Mode|assets/${prefix}\\.worker)-[\\w-]+\\.js$`).test(p))
}

/** Throws when `gzip` bytes are over `budget`. */
export function checkBudget(gzip, budget = WASM_GZIP_BUDGET) {
  if (gzip > budget) throw new Error(`engine.wasm is ${(gzip / 1e6).toFixed(2)} MB gzipped, over the budget of ${(budget / 1e6).toFixed(2)} MB. Find what grew, or raise WASM_GZIP_BUDGET in web/build.mjs on purpose.`)
}

const mb = (n) => (n / 1e6).toFixed(1)

/**
 * Copy Monaco from its npm package `from` into `to` (replacing what is there):
 * the minified editor without the unused services, with its licence and the
 * notices for what it bundles. Returns Monaco's version.
 */
export function copyMonaco(from, to) {
  fs.rmSync(to, { recursive: true, force: true })
  fs.cpSync(path.join(from, "min", "vs"), path.join(to, "vs"), { recursive: true, filter: wanted })
  for (const f of ["LICENSE", "ThirdPartyNotices.txt"]) fs.copyFileSync(path.join(from, f), path.join(to, f))
  return JSON.parse(fs.readFileSync(path.join(from, "package.json"), "utf8")).version
}

function build({ skipEngine }) {
  const web = path.dirname(fileURLToPath(import.meta.url))
  const target = path.join(web, "app", "engine.wasm")
  if (!skipEngine) {
    const engine = path.join(web, "engine")
    execFileSync("cargo", ["build", "--release", "--locked"], { cwd: engine, stdio: "inherit" })
    fs.copyFileSync(path.join(engine, "target", "wasm32-unknown-unknown", "release", "turtle_lsp_web.wasm"), target)
  }
  const bytes = fs.readFileSync(target)
  const gzip = zlib.gzipSync(bytes, { level: 9 }).length
  console.log(`web/app/engine.wasm: ${mb(bytes.length)} MB, ${mb(gzip)} MB gzipped (budget ${mb(WASM_GZIP_BUDGET)} MB)`)
  checkBudget(gzip)

  const version = copyMonaco(path.join(web, "..", "node_modules", "monaco-editor"), path.join(web, "app", "vendor", "monaco"))
  console.log(`web/app/vendor/monaco: Monaco ${version}`)

  // Vim mode's keymap (monaco-vim), loaded by the page only when turned on.
  const vim = path.join(web, "..", "node_modules", "monaco-vim")
  fs.mkdirSync(path.join(web, "app", "vendor", "monaco-vim"), { recursive: true })
  fs.copyFileSync(path.join(vim, "dist", "monaco-vim.umd.js"), path.join(web, "app", "vendor", "monaco-vim", "monaco-vim.js"))
  for (const f of ["LICENSE", "LICENSE.codemirror.txt"]) fs.copyFileSync(path.join(vim, f), path.join(web, "app", "vendor", "monaco-vim", f))
  console.log("web/app/vendor/monaco-vim: monaco-vim " + JSON.parse(fs.readFileSync(path.join(vim, "package.json"), "utf8")).version)

  for (const f of ["LICENSE", "THIRD_PARTY_NOTICES.md"]) fs.copyFileSync(path.join(web, "..", f), path.join(web, "app", "vendor", f))
  console.log("web/app/vendor: LICENSE, THIRD_PARTY_NOTICES.md")
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    build({ skipEngine: process.argv.includes("--skip-engine") })
  } catch (e) {
    console.error(e.message)
    process.exit(1)
  }
}
