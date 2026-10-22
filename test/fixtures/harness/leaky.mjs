// Run by test/harness.test.mjs, never by the suite glob: starts a server and
// never stops it, after a file-level hook that stops another one properly.
import { test, after } from "node:test"
import { startServer } from "../../lsp-client.mjs"

let tidy
after(() => tidy?.stop())
test("starts two servers and stops one", async () => {
  tidy = await startServer()
  await startServer() // never stopped
})
