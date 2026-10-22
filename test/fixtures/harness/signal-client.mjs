// Run by test/harness.test.mjs, never by the suite glob: starts a server, says
// where its cache root and process are, and idles until a signal arrives.
import path from "node:path"
import { startServer } from "../../lsp-client.mjs"

const s = await startServer()
console.log(JSON.stringify({ cacheRoot: path.dirname(s.cacheDir), pid: s.proc.pid }))
setInterval(() => {}, 1 << 30)
