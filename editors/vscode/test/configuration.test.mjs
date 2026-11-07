// What the extension sends the server when the settings change. The server
// reads the `turtle` section of `settings` (as a client that sends the whole
// configuration does), so the user's settings go inside that section.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import { configurationChange, layered, settingsFile, settingsFrom } from "../src/client.ts"

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

// A turtle-lsp.json comes from whatever repository is open: in the browser,
// any repository on github.dev, read in the shared web extension host.
// JSON.parse keeps "__proto__" as an own key; none of these may reach
// Object.prototype or any result's prototype.
const prototypeKeys = () => Object.getOwnPropertyNames(Object.prototype).sort()

for (const [name, text, legit] of [
  ["a top-level __proto__", '{"__proto__": {"polluted": true}, "format": {"lineWidth": 80}}', { format: { lineWidth: 80 } }],
  ["a nested __proto__", '{"lint": {"__proto__": {"x": 1}, "ignore": ["a"]}}', { lint: { ignore: ["a"] } }],
  ["a dotted __proto__", '{"__proto__.polluted": true, "turtle.__proto__.x": 1, "format.lineWidth": 80}', { format: { lineWidth: 80 } }],
  ["constructor.prototype", '{"constructor": {"prototype": {"y": 1}}, "shacl": {"constructor": {"prototype": {"y": 1}}, "enabled": false}}', { shacl: { enabled: false } }],
  ["a __proto__ prefix", '{"prefixes": {"__proto__": {"polluted": true}, "ex": "http://example.org/"}, "prefixes.__proto__": "http://x/"}', { prefixes: { ex: "http://example.org/" } }],
]) {
  test(`a settings file with ${name} pollutes no prototype and keeps the keys beside it`, () => {
    const before = prototypeKeys()
    const { settings, ignored } = settingsFile(JSON.parse(text), defaultOf)
    const out = layered(settings, JSON.parse(text))
    const back = layered(JSON.parse(text), settings)
    for (const o of [settings, out, back, ...Object.values(settings)]) assert.equal(Object.getPrototypeOf(o), Object.prototype)
    assert.equal(({}).polluted, undefined)
    assert.equal(({}).x, undefined)
    assert.equal(({}).y, undefined)
    assert.deepEqual(prototypeKeys(), before)
    assert.deepEqual(settings, legit)
    assert.deepEqual(JSON.parse(JSON.stringify(settings)), legit)
    assert.deepEqual(ignored, [])
  })
}

test("layering the user's settings over a file skips __proto__ on either side", () => {
  const before = prototypeKeys()
  const under = JSON.parse('{"__proto__": {"polluted": true}, "format": {"__proto__": {"x": 1}, "lineWidth": 80}}')
  const over = JSON.parse('{"__proto__": {"y": 1}, "format": {"__proto__": {"x": 1}}, "lint": {"ignore": ["b"]}}')
  const out = layered(under, over)
  assert.deepEqual(out, { format: { lineWidth: 80 }, lint: { ignore: ["b"] } })
  assert.equal(Object.getPrototypeOf(out), Object.prototype)
  assert.equal(Object.getPrototypeOf(out.format), Object.prototype)
  assert.equal(({}).polluted, undefined)
  assert.equal(({}).x, undefined)
  assert.equal(({}).y, undefined)
  assert.deepEqual(prototypeKeys(), before)
})

test("prefixes merged across scopes from a workspace's settings skip __proto__", () => {
  // .vscode/settings.json comes with the repository too.
  const before = prototypeKeys()
  const settings = settingsFrom({
    properties: { "turtle.prefixes": {} },
    restricted: [],
    trusted: true,
    inspect: () => ({ globalValue: { ex: "http://example.org/" }, workspaceValue: JSON.parse('{"__proto__": {"polluted": true}, "my": "http://my/"}') }),
  })
  assert.deepEqual(settings, { prefixes: { ex: "http://example.org/", my: "http://my/" } })
  assert.equal(Object.getPrototypeOf(settings.prefixes), Object.prototype)
  assert.equal(settings.prefixes.polluted, undefined)
  assert.equal(({}).polluted, undefined)
  assert.deepEqual(prototypeKeys(), before)
})
