// What the extension sends the server when the settings change. The server
// reads the `turtle` section of `settings` (as a client that sends the whole
// configuration does), so the user's settings go inside that section.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import { configurationChange, layered, settingsFile } from "../src/client.ts"

test("a settings change goes to the server as the turtle section of settings", () => {
  const settings = { format: { lineWidth: 80 }, imports: { network: false } }
  assert.deepEqual(configurationChange(settings), { settings: { turtle: settings } })
})

test("no settings set is an empty turtle section, not a bare object", () => {
  // An empty section still replaces the settings the server started with.
  assert.deepEqual(configurationChange({}), { settings: { turtle: {} } })
})

// The browser reads turtle-lsp.json itself (the worker has no disk) and
// layers it under the user's settings, as the native server does.
const properties = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8")).contributes.configuration.properties
const defaultOf = (key) => properties[`turtle.${key}`]?.default

test("a settings file's keys, nested, dotted or under turtle, come out nested", () => {
  const { settings, ignored } = settingsFile(
    { turtle: { "format.lineWidth": 80 }, lint: { "ignore": ["unused-prefix"] }, "prefixes.ex": "http://example.org/", prefixes: { "a.b": "http://a/" } },
    defaultOf,
  )
  assert.deepEqual(settings, { format: { lineWidth: 80 }, lint: { ignore: ["unused-prefix"] }, prefixes: { ex: "http://example.org/", "a.b": "http://a/" } })
  assert.deepEqual(ignored, [])
})

test("a settings file cannot turn on network imports or raise a limit past its default", () => {
  const { settings, ignored } = settingsFile(
    { imports: { network: true, unresolved: "ignore" }, "shacl.maxTriples": 1e9, query: { maxRows: 0, timeoutSeconds: 5 } },
    defaultOf,
  )
  assert.deepEqual(settings, { imports: { unresolved: "ignore" }, shacl: { maxTriples: 2000000 }, query: { maxRows: 1000, timeoutSeconds: 5 } })
  assert.deepEqual(ignored.sort(), ["imports.network", "query.maxRows", "shacl.maxTriples"])
})

test("the user's settings go over the file's: objects merge, lint lists add up, prefixes replace", () => {
  const file = { format: { lineWidth: 80 }, shacl: { enabled: false }, lint: { ignore: ["a"] }, prefixes: { ex: "http://example.org/" } }
  const user = { format: { lineWidth: 120 }, lint: { ignore: ["b"] }, prefixes: { my: "http://my/" } }
  assert.deepEqual(layered(file, user), {
    format: { lineWidth: 120 },
    shacl: { enabled: false },
    lint: { ignore: ["a", "b"] },
    prefixes: { my: "http://my/" },
  })
  // Without a file the user's settings go as they are.
  assert.deepEqual(layered({}, user), user)
})
