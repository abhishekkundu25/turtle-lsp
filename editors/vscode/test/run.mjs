// Downloads VS Code (cached in .vscode-test/) and runs test/suite.cjs inside it
// with this extension loaded and test/fixtures as the workspace.
import path from "node:path"
import { fileURLToPath } from "node:url"
import { runTests } from "@vscode/test-electron"

const here = path.dirname(fileURLToPath(import.meta.url))
const extensionDevelopmentPath = path.resolve(here, "..")
const fixtures = path.resolve(here, "../../../test/fixtures")

try {
  await runTests({
    version: process.env.VSCODE_VERSION ?? "stable",
    extensionDevelopmentPath,
    extensionTestsPath: path.join(here, "suite.cjs"),
    launchArgs: [fixtures, "--disable-extensions", "--disable-workspace-trust", "--skip-welcome", "--skip-release-notes"],
    extensionTestsEnv: { TURTLE_FIXTURES: fixtures },
  })
} catch (err) {
  console.error(err)
  process.exit(1)
}
