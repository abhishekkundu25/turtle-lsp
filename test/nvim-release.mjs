// What the Neovim test files share: whether nvim is installed, and the
// release archive build.lua downloads on this machine, made from the built
// server as the release workflow makes it.
import { spawnSync, execFileSync } from "node:child_process"
import path from "node:path"

export const hasNvim = spawnSync("nvim", ["--version"]).status === 0
// In CI a missing nvim is a broken install step, not a reason to skip.
if (!hasNvim && process.env.CI === "true") {
  throw new Error(
    'nvim not found on PATH with CI=true: the workflow step "Install Neovim (for the editor checks)" did not install it. The Neovim tests fail in CI instead of skipping.',
  )
}

const windows = process.platform === "win32"
// The server's file name in the archive and in the plugin's bin/.
export const EXE = windows ? "turtle-lsp.exe" : "turtle-lsp"

// The release asset build.lua asks for on this machine (release.yml's names).
export const ASSET = {
  "darwin-arm64": "turtle-lsp-darwin-arm64.tar.gz",
  "darwin-x64": "turtle-lsp-darwin-x64.tar.gz",
  "linux-arm64": "turtle-lsp-linux-arm64.tar.gz",
  "linux-x64": "turtle-lsp-linux-x64.tar.gz",
  "win32-x64": "turtle-lsp-win32-x64.zip",
}[`${process.platform}-${process.arch}`]

// Windows' own tar (bsdtar), which build.lua extracts the zip with. It writes
// a zip for a .zip name (-a); the GNU tar of Git for Windows, which can come
// first on PATH, cannot.
export const windowsTar = path.join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe")

/** Packs `stage`/EXE into `archive`: a .tar.gz, or on Windows a .zip. */
export function pack(stage, archive) {
  if (windows) execFileSync(windowsTar, ["-a", "-cf", archive, "-C", stage, EXE])
  else execFileSync("tar", ["-czf", archive, "-C", stage, EXE])
}
