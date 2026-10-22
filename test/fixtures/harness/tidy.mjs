// Run by test/harness.test.mjs, never by the suite glob: stops its server in a
// file-level hook, as the suite's files do.
import { test, after } from "node:test"
import { startServer } from "../../lsp-client.mjs"

let s
after(() => s?.stop())
test("starts a server", async () => {
  s = await startServer()
})
