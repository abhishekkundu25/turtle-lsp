// Downloads VS Code (cached in .vscode-test/) and runs test/suite.cjs inside it
// with this extension loaded and test/fixtures as the workspace.
import fs from "node:fs"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { runTests } from "@vscode/test-electron"

const here = path.dirname(fileURLToPath(import.meta.url))
const extensionDevelopmentPath = path.resolve(here, "..")
const fixtures = path.resolve(here, "../../../test/fixtures")

// VS Code puts its IPC socket under the user data dir; a deep checkout path
// makes it longer than macOS allows (listen EINVAL). Use a short temp dir.
const userData = fs.mkdtempSync(path.join(os.tmpdir(), "tvsc-"))

try {
  await runTests({
    version: process.env.VSCODE_VERSION ?? "stable",
    extensionDevelopmentPath,
    extensionTestsPath: path.join(here, "suite.cjs"),
    launchArgs: [fixtures, "--disable-extensions", "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes", `--user-data-dir=${userData}`],
    extensionTestsEnv: { TURTLE_FIXTURES: fixtures },
  })
} catch (err) {
  console.error(err)
  process.exitCode = 1
} finally {
  fs.rmSync(userData, { recursive: true, force: true })
}
