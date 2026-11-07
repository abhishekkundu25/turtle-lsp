// The JSON Schema VS Code checks turtle-lsp.json against: the keys the server
// reads from the file, with the editor settings' defaults and choices.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"

const read = (path) => fs.readFileSync(new URL(path, import.meta.url), "utf8")
const pkg = JSON.parse(read("../package.json"))
const settings = pkg.contributes.configuration.properties
const schema = JSON.parse(read("../schemas/turtle-lsp.schema.json"))
const rustSource = read("../../../src/settings.rs")

// The keys src/settings.rs applies, as in manifest.test.mjs: the match arms of
// Settings::apply_at that name a whole setting (`["shacl", "maxTriples"] =>`).
const serverKeys = [...rustSource.matchAll(/^\s*\[((?:"\w+"(?:,\s*)?)+)\]\s*=>/gm)]
  .map((m) => [...m[1].matchAll(/"(\w+)"/g)].map((k) => k[1]).join("."))
  .sort()
// The keys the file cannot turn on or raise: `ig.untrusted("shacl.maxTriples")`.
const untrustedKeys = [...new Set([...rustSource.matchAll(/ig\.untrusted\("([\w.]+)"\)/g)].map((m) => m[1]))].sort()

const resolve = (s) => (s.$ref ? resolve(s.$ref.slice(1).split("/").slice(1).reduce((o, k) => o[k], schema)) : s)

// Leaf schemas by dotted key, read from the nested form.
const leaves = {}
for (const [group, s] of Object.entries(schema.properties)) {
  if (group === "$schema" || group === "turtle") continue
  const g = resolve(s)
  if (g.properties) for (const [key, leaf] of Object.entries(g.properties)) leaves[`${group}.${key}`] = resolve(leaf)
  else leaves[group] = g
}

// What the schema accepts, for the draft-07 keywords it uses.
function errors(value, s = schema, at = "") {
  s = resolve(s)
  const out = []
  const kind = Array.isArray(value) ? "array" : value === null ? "null" : Number.isInteger(value) ? "integer" : typeof value
  if (s.type && !(s.type === kind || (s.type === "number" && kind === "integer"))) return [`${at}: ${kind}, not ${s.type}`]
  if (s.enum && !s.enum.some((e) => e === value)) out.push(`${at}: ${JSON.stringify(value)} is not one of ${s.enum.join(", ")}`)
  if (s.minimum !== undefined && value < s.minimum) out.push(`${at}: below ${s.minimum}`)
  if (s.exclusiveMinimum !== undefined && value <= s.exclusiveMinimum) out.push(`${at}: not above ${s.exclusiveMinimum}`)
  if (s.maximum !== undefined && value > s.maximum) out.push(`${at}: above ${s.maximum}`)
  if (kind === "array" && s.items) value.forEach((v, i) => out.push(...errors(v, s.items, `${at}[${i}]`)))
  if (kind === "object") {
    for (const [k, v] of Object.entries(value)) {
      const matched = [
        ...(s.properties && k in s.properties ? [s.properties[k]] : []),
        ...Object.entries(s.patternProperties ?? {})
          .filter(([p]) => new RegExp(p, "u").test(k))
          .map(([, p]) => p),
      ]
      if (matched.length) for (const m of matched) out.push(...errors(v, m, `${at}/${k}`))
      else if (s.additionalProperties === false) out.push(`${at}/${k}: not a setting`)
      else if (typeof s.additionalProperties === "object") out.push(...errors(v, s.additionalProperties, `${at}/${k}`))
    }
  }
  return out
}

test("VS Code checks turtle-lsp.json against the schema, and the .vsix ships it", () => {
  assert.equal(schema.$schema, "http://json-schema.org/draft-07/schema#")
  const [entry] = pkg.contributes.jsonValidation
  assert.deepEqual(entry.fileMatch, ["turtle-lsp.json", "**/turtle-lsp.json"])
  assert.equal(entry.url, "./schemas/turtle-lsp.schema.json")
  assert.ok(fs.existsSync(new URL(`../${entry.url}`, import.meta.url)))
  assert.equal(pkg.files, undefined, "a files list would need schemas/")
  const ignored = read("../.vscodeignore").split("\n").map((l) => l.trim())
  assert.ok(!ignored.some((l) => l.startsWith("schemas") || l === "**/*.json"), ignored.join(", "))
})

test("the schema has exactly the keys the server reads from the file", () => {
  assert.ok(serverKeys.length >= 16, serverKeys.join(", "))
  assert.deepEqual(Object.keys(leaves).sort(), serverKeys)
  // Each also dotted, and inside a "turtle" object.
  for (const key of serverKeys.filter((k) => k !== "prefixes")) {
    const v = leaves[key].default
    assert.deepEqual(errors({ [key]: v }), [], key)
    assert.deepEqual(errors({ turtle: { [key]: v } }), [], key)
    assert.deepEqual(errors({ [`turtle.${key}`]: v }), [], key)
    const [group, name] = key.split(".")
    assert.deepEqual(errors({ [`turtle.${group}`]: { [name]: v } }), [], key)
    assert.ok(leaves[key].description && leaves[key].markdownDescription, `${key} has no description`)
  }
})

test("defaults, types and choices agree with the VS Code settings", () => {
  for (const key of serverKeys) {
    const leaf = leaves[key]
    const setting = settings[`turtle.${key}`]
    assert.equal(leaf.type, setting.type, key)
    assert.deepEqual(leaf.default, setting.default, key)
    if (setting.enum) assert.deepEqual(leaf.enum, setting.enum, key)
    if (setting.enumDescriptions) assert.deepEqual(leaf.enumDescriptions, setting.enumDescriptions, key)
    // The file may allow less than the editor, never more.
    if (setting.minimum !== undefined) assert.ok(leaf.minimum >= setting.minimum || leaf.exclusiveMinimum >= setting.minimum, key)
    assert.deepEqual(errors({ [key]: setting.default }), [], key)
  }
  // lint.enable offers the off-by-default codes (features::curation::OPT_IN).
  const curation = read("../../../src/features/curation.rs")
  const names = Object.fromEntries([...curation.matchAll(/const (\w+): &str = "([\w-]+)"/g)].map((m) => [m[1], m[2]]))
  const optIn = curation.match(/OPT_IN: &\[&str\] = &\[([^\]]*)\]/)[1].match(/\w+/g).map((n) => names[n])
  assert.deepEqual([...leaves["lint.enable"].items.enum].sort(), [...optIn].sort())
})

test("what the file cannot turn on or raise is flagged, and described", () => {
  assert.deepEqual(untrustedKeys, [
    "imports.network",
    "query.maxBufferedRows",
    "query.maxRows",
    "query.timeoutSeconds",
    "reasoning.maxTriples",
    "shacl.maxTriples",
  ])
  for (const key of untrustedKeys) {
    const leaf = leaves[key]
    const def = settings[`turtle.${key}`].default
    assert.match(leaf.markdownDescription, /This file can(not| lower it but not raise it)/, key)
    assert.deepEqual(errors({ [key]: def }), [], key)
    if (leaf.type === "boolean") assert.notDeepEqual(errors({ [key]: true }), [], key)
    else assert.notDeepEqual(errors({ [key]: def + 1 }), [], key)
  }
})

test("a project's file passes; a typo or a wrong value does not", () => {
  const readme = read("../../../README.md")
  const example = readme.split("## Settings file (turtle-lsp.json)")[1].match(/```json\n([\s\S]*?)```/)[1]
  for (const ok of [
    JSON.parse(example),
    { $schema: "./node_modules/x/turtle-lsp.schema.json", "prefixes.ex": "http://example.org/", "lint.undescribedTerms": false },
    { turtle: { "lint.ignore": ["unused-prefix"], lint: { enable: ["missing-label"] } } },
    { query: { timeoutSeconds: 2.5, maxRows: 50 }, reasoning: { rules: "complete" }, imports: { network: false } },
    { query: { reasoning: "default" }, "query.reasoning": "complete" },
  ]) {
    assert.deepEqual(errors(ok), [], JSON.stringify(ok))
  }
  for (const bad of [
    { lint: { ignroe: ["unused-prefix"] } },
    { "lint.ignroe": ["unused-prefix"] },
    { shacl: { enabled: "yes" } },
    { reasoning: { rules: "all" } },
    { query: { reasoning: "on" } },
    { format: { lineWidth: 10 } },
    { prefixes: { ex: 42 } },
    { lint: { enable: ["missing-lable"] } },
    { query: { timeoutSeconds: 0 } },
  ]) {
    assert.notDeepEqual(errors(bad), [], JSON.stringify(bad))
  }
})
