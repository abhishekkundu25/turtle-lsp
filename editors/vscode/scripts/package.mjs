// Builds the Rust server, copies it into server/, bundles the extension and
// packages a platform-specific .vsix (e.g. turtle-lsp-darwin-arm64-0.1.0.vsix).
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const here = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const repo = path.resolve(here, "../..")
const run = (cmd, args, cwd = here) => execFileSync(cmd, args, { cwd, stdio: "inherit" })

const target = process.env.VSCE_TARGET ?? `${process.platform === "win32" ? "win32" : process.platform}-${process.arch}`
const exe = process.platform === "win32" ? "turtle-lsp.exe" : "turtle-lsp"

run("cargo", ["build", "--release"], repo)
fs.mkdirSync(path.join(here, "server"), { recursive: true })
fs.copyFileSync(path.join(repo, "target", "release", exe), path.join(here, "server", exe))
fs.chmodSync(path.join(here, "server", exe), 0o755)
run("npm", ["run", "build"])
const version = JSON.parse(fs.readFileSync(path.join(here, "package.json"), "utf8")).version
const out = `turtle-lsp-${target}-${version}.vsix`
run("npx", ["vsce", "package", "--target", target, "--no-dependencies", "--skip-license", "--out", out])
console.log(`\nInstall with: code --install-extension ${path.join(here, out)}`)
