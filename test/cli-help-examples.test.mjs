// The examples in `turtle-lsp --help` run as written: each is read from the
// help text and run in a small workspace, so an example that stops working
// fails here instead of misleading the agent that copies it.
import { test } from "node:test"
import assert from "node:assert/strict"
import { spawnSync } from "node:child_process"
import fs from "node:fs"
import path from "node:path"
import { serverPath, tempDir } from "./lsp-client.mjs"

const PREFIXES = "@prefix ex: <http://example.org/> .\n@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n"
const FILES = {
  ".git/HEAD": "",
  "prefixes.ttl": PREFIXES,
  "data/people.ttl": 'ex:Person a rdfs:Class ; rdfs:label "Person" .\nex:ann a ex:Person ; rdfs:label "Ann" .\n',
  "new.ttl": 'ex:bob   a ex:Person;rdfs:label "Bob".\n',
}

function examples() {
  const help = spawnSync(serverPath, ["--help"], { encoding: "utf8" }).stdout
  const at = help.indexOf("\nExamples:\n")
  assert.ok(at >= 0, "the help has an Examples section")
  // A command is indented two spaces; its description four more.
  return help
    .slice(at)
    .split("\n")
    .filter((l) => /^  \S/.test(l))
    .map((l) => l.trim())
}

test("the help has examples, and each runs as written", (t) => {
  const ws = tempDir("turtle-cli-examples-")
  t.after(() => fs.rmSync(ws, { recursive: true, force: true }))
  for (const [name, text] of Object.entries(FILES)) {
    fs.mkdirSync(path.dirname(path.join(ws, name)), { recursive: true })
    fs.writeFileSync(path.join(ws, name), text)
  }
  fs.mkdirSync(path.join(ws, ".cache"))
  const list = examples()
  assert.ok(list.length >= 5, list.join("\n"))
  for (const line of list) {
    // `turtle-lsp` is the binary under test, wherever it is.
    const command = line.replaceAll(/\bturtle-lsp\b/g, `'${serverPath}'`)
    const r = spawnSync("sh", ["-c", command], {
      cwd: ws,
      encoding: "utf8",
      timeout: 60000,
      env: { ...process.env, TURTLE_LSP_CACHE_DIR: path.join(ws, ".cache") },
    })
    // 0 or 1 (problems found): never 2, the tool failing.
    assert.ok(r.status === 0 || r.status === 1, `${line}\nexit ${r.status}\n${r.stdout}${r.stderr}`)
    if (line.includes("--format json")) assert.doesNotThrow(() => JSON.parse(r.stdout), line)
    if (/format -|explain|query/.test(line)) assert.ok(r.stdout.trim().length > 0, `${line}: no output`)
  }
  assert.equal(fs.existsSync(path.join(ws, "data", "new.ttl")), false, "checking standard input wrote no file")
})
