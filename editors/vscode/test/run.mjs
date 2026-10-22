// Downloads VS Code (cached in .vscode-test/) and runs test/suite.cjs inside it
// with this extension loaded and a temporary copy of test/fixtures as the
// workspace: the suite writes files into its workspace, and the Node tests
// and the chaos test read the real fixtures, which must stay as they are.
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { runTests } from "@vscode/test-electron"
import { processesOf, leftRunning } from "../../../test/procs.mjs"

const here = path.dirname(fileURLToPath(import.meta.url))
const extensionDevelopmentPath = path.resolve(here, "..")
const sourceFixtures = path.resolve(here, "../../../test/fixtures")

// VS Code puts its IPC socket under the user data dir; a deep checkout path
// makes it longer than macOS allows (listen EINVAL). Use a short temp dir.
const userData = fs.mkdtempSync(path.join(os.tmpdir(), "tvsc-"))

// The copy, under its long name (a Windows temp path can be an 8.3 short
// name, which VS Code and the server spell differently). Not the private,
// gitignored fragments of test/fixtures/real, which the suite does not use.
const fixtures = fs.realpathSync.native(fs.mkdtempSync(path.join(os.tmpdir(), "tvsc-fixtures-")))
fs.cpSync(sourceFixtures, fixtures, {
  recursive: true,
  filter: (src) => path.relative(sourceFixtures, src).split(path.sep)[0] !== "real",
})

// The extension runs a private copy of the freshly built server (set through
// the user's turtle.server.path), so the test never picks up a stale bundled
// binary and the leak check below sees only the servers VS Code started.
const exe = process.platform === "win32" ? "turtle-lsp.exe" : "turtle-lsp"
const built = path.resolve(here, "../../../target/release", exe)
const server = path.join(userData, exe)
fs.copyFileSync(built, server)
fs.chmodSync(server, 0o755)
fs.mkdirSync(path.join(userData, "User"), { recursive: true })
fs.writeFileSync(path.join(userData, "User", "settings.json"), JSON.stringify({ "turtle.server.path": server }))

try {
  await runTests({
    version: process.env.VSCODE_VERSION ?? "stable",
    extensionDevelopmentPath,
    extensionTestsPath: path.join(here, "suite.cjs"),
    launchArgs: [fixtures, "--disable-extensions", "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes", `--user-data-dir=${userData}`],
    extensionTestsEnv: { TURTLE_FIXTURES: fixtures, TURTLE_LSP_CACHE_DIR: path.join(userData, "cache") },
  })
  // Quitting VS Code must not leave a server behind.
  const alive = await leftRunning(server, processesOf(server))
  if (alive.length > 0) throw new Error(`turtle-lsp still running after VS Code quit: ${alive.join(", ")}`)
  console.log("ok - no turtle-lsp left running after VS Code quit")
} catch (err) {
  console.error(err)
  process.exitCode = 1
} finally {
  fs.rmSync(userData, { recursive: true, force: true })
  fs.rmSync(fixtures, { recursive: true, force: true })
}
