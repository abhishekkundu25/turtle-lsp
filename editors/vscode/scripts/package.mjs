// Builds the Rust server, copies it into server/, bundles the extension and
// packages a platform-specific .vsix (e.g. turtle-lsp-darwin-arm64-0.1.0.vsix).
//
// Env:
//   VSCE_TARGET   VS Code platform (darwin-arm64, linux-x64, win32-x64, ...);
//                 defaults to this machine.
//   CARGO_TARGET  Rust target triple to cross-compile for (CI sets it).
import { execFileSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"

const here = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..")
const repo = path.resolve(here, "../..")
const run = (cmd, args, cwd = here) => execFileSync(cmd, args, { cwd, stdio: "inherit", shell: process.platform === "win32" })

const target = process.env.VSCE_TARGET ?? `${process.platform}-${process.arch}`
const triple = process.env.CARGO_TARGET
const exe = target.startsWith("win32") ? "turtle-lsp.exe" : "turtle-lsp"

run("cargo", ["build", "--release", "--locked", ...(triple ? ["--target", triple] : [])], repo)
const built = path.join(repo, "target", ...(triple ? [triple] : []), "release", exe)
fs.rmSync(path.join(here, "server"), { recursive: true, force: true })
fs.mkdirSync(path.join(here, "server"))
fs.copyFileSync(built, path.join(here, "server", exe))
fs.chmodSync(path.join(here, "server", exe), 0o755)
for (const f of ["LICENSE", "CHANGELOG.md"]) fs.copyFileSync(path.join(repo, f), path.join(here, f))
// The notices for what the server and the bundled npm packages include.
const notices = "THIRD_PARTY_NOTICES.md"

run("npm", ["run", "build"])
const version = JSON.parse(fs.readFileSync(path.join(here, "package.json"), "utf8")).version
const out = `turtle-lsp-${target}-${version}.vsix`
try {
  fs.copyFileSync(path.join(repo, notices), path.join(here, notices))
  run("npx", ["vsce", "package", "--target", target, "--no-dependencies", "--out", out])
} finally {
  // The copied binary is only for the .vsix; left behind, the extension would
  // prefer it over a fresh `cargo build` when run from this checkout.
  fs.rmSync(path.join(here, "server"), { recursive: true, force: true })
  // Only for the .vsix too (LICENSE and CHANGELOG.md are gitignored here).
  fs.rmSync(path.join(here, notices), { force: true })
}
console.log(`\nInstall with: code --install-extension ${path.join(here, out)}`)
