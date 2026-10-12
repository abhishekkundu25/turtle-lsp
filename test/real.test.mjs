// Real-world Turtle from the ontology-design repo. See fixtures/README.md.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { startServer, uriOf, fixtures } from "./lsp-client.mjs"

const dir = path.join(fixtures, "real")
// test/fixtures/real is local-only (gitignored); skip when it is not there.
const present = fs.existsSync(path.join(dir, "prefixes.ttl"))
const header = present ? fs.readFileSync(path.join(dir, "prefixes.ttl"), "utf8") : ""
const files = present ? fs.readdirSync(dir).filter((f) => f.endsWith(".ttl") && f !== "prefixes.ttl") : []
if (!present) test("real-world fixtures", { skip: "test/fixtures/real not present (local-only)" }, () => {})
const fmt = (diags) => diags.map((d) => `${d.range.start.line}:${d.range.start.character} ${d.message}`).join("\n")

let s
before(async () => {
  if (present) s = await startServer({ cwd: dir })
})
after(async () => {
  await s?.stop()
})

for (const f of files) {
  test(`${f} with prefix header: no errors or warnings`, async () => {
    const uri = uriOf(path.join(dir, `__full_${f}`))
    s.open(uri, header + fs.readFileSync(path.join(dir, f), "utf8"))
    const diags = (await s.diagnosticsFor(uri)).filter((d) => d.severity <= 2)
    assert.deepEqual(diags, [], fmt(diags))
  })
}

// Proposed behaviour: a workspace-level prefixes.ttl supplies prefixes to headerless fragments.
for (const f of files) {
  test(`${f} as a fragment: prefixes come from the workspace prefixes.ttl`, async () => {
    const uri = uriOf(path.join(dir, f))
    s.open(uri, fs.readFileSync(path.join(dir, f), "utf8"))
    const diags = (await s.diagnosticsFor(uri)).filter((d) => /not declared/.test(d.message))
    assert.deepEqual(diags, [], fmt(diags))
  })
}
