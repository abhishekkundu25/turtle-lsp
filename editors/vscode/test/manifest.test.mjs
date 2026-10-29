// Security-relevant parts of the extension manifest, and the settings the
// server reads.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import { settingsFrom } from "../src/server.ts"

const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"))
const settings = pkg.contributes.configuration.properties
const setting = settings["turtle.server.path"]
const restricted = pkg.capabilities.untrustedWorkspaces.restrictedConfigurations

// The keys src/settings.rs applies: the match arms of Settings::apply_at that
// name a whole setting (`["shacl", "maxTriples"] =>`), as dotted names.
const rustSource = fs.readFileSync(new URL("../../../src/settings.rs", import.meta.url), "utf8")
const serverKeys = [...rustSource.matchAll(/^\s*\[((?:"\w+"(?:,\s*)?)+)\]\s*=>/gm)]
  .map((m) => [...m[1].matchAll(/"(\w+)"/g)].map((k) => k[1]).join("."))
  .sort()

test("a workspace cannot choose which binary the extension runs", () => {
  // "machine" settings are ignored in workspace and folder settings files.
  assert.equal(setting.scope, "machine")
})

test("turtle.server.path is restricted in untrusted workspaces", () => {
  const cap = pkg.capabilities?.untrustedWorkspaces
  assert.equal(cap?.supported, "limited")
  assert.ok(cap.restrictedConfigurations.includes("turtle.server.path"))
})

test("network imports are off by default and restricted in untrusted workspaces", () => {
  const s = settings["turtle.imports.network"]
  assert.equal(s.type, "boolean")
  assert.equal(s.default, false)
  assert.equal(s.scope, "machine-overridable")
  assert.ok(pkg.capabilities.untrustedWorkspaces.restrictedConfigurations.includes("turtle.imports.network"))
})

test("server settings are declared with the server's defaults", () => {
  const expected = {
    "turtle.inlayHints.labels": ["string", "opaque"],
    "turtle.shacl.enabled": ["boolean", true],
    "turtle.shacl.maxTriples": ["integer", 2000000],
    "turtle.shacl.onType": ["boolean", true],
    "turtle.lint.undescribedTerms": ["boolean", true],
    "turtle.imports.network": ["boolean", false],
    "turtle.imports.unresolved": ["string", "soften"],
    "turtle.format.lineWidth": ["integer", 100],
    "turtle.codeLens.references": ["boolean", true],
    "turtle.diagnostics.workspace": ["boolean", false],
    "turtle.parse.incremental": ["boolean", true],
    "turtle.query.timeoutSeconds": ["number", 30],
    "turtle.query.maxRows": ["integer", 1000],
    "turtle.query.maxBufferedRows": ["integer", 10000000],
  }
  for (const [key, [type, def]] of Object.entries(expected)) {
    assert.ok(settings[key], `${key} missing`)
    assert.equal(settings[key].type, type, key)
    assert.equal(settings[key].default, def, key)
    assert.ok(settings[key].markdownDescription || settings[key].description, `${key} has no description`)
  }
  assert.deepEqual(settings["turtle.inlayHints.labels"].enum, ["opaque", "all", "off"])
  assert.deepEqual(settings["turtle.imports.unresolved"].enum, ["soften", "ignore"])
})

test("lint.ignore and lint.enable are lists of codes, empty by default, sent as lists", () => {
  for (const key of ["turtle.lint.ignore", "turtle.lint.enable"]) {
    assert.equal(settings[key].type, "array", key)
    assert.equal(settings[key].items.type, "string", key)
    assert.deepEqual(settings[key].default, [], key)
    assert.equal(settings[key].scope, "resource", key)
  }
  // Every off-by-default code (features::curation::OPT_IN) is named in lint.enable's description.
  const curation = fs.readFileSync(new URL("../../../src/features/curation.rs", import.meta.url), "utf8")
  const names = Object.fromEntries([...curation.matchAll(/const (\w+): &str = "([\w-]+)"/g)].map((m) => [m[1], m[2]]))
  const optIn = curation.match(/OPT_IN: &\[&str\] = &\[([^\]]*)\]/)[1].match(/\w+/g).map((n) => names[n])
  assert.equal(optIn.length, 8, optIn.join(", "))
  for (const code of optIn) assert.match(settings["turtle.lint.enable"].markdownDescription, new RegExp(`\`${code}\``), code)
  // A list is sent as a list; like VS Code itself, the narrowest scope's list wins.
  const inspect = (key) => (key === "lint.ignore" ? { globalValue: ["unused-prefix"], workspaceValue: ["unknown-term"] } : {})
  assert.deepEqual(settingsFrom({ properties: settings, restricted, inspect, trusted: true }), { lint: { ignore: ["unknown-term"] } })
})

test("JSON-LD files are served without taking over VS Code's JSON language", () => {
  // The JSON language (and its highlighting) stays VS Code's own: no language
  // here claims .jsonld, and the client attaches to the files by name.
  const claimed = pkg.contributes.languages.flatMap((l) => l.extensions ?? [])
  assert.ok(!claimed.includes(".jsonld"), claimed.join(", "))
  assert.ok(!pkg.contributes.languages.some((l) => l.id === "json" || l.id === "jsonld"))
  assert.ok(pkg.activationEvents.some((e) => e.startsWith("workspaceContains:") && e.includes("jsonld")), pkg.activationEvents.join(", "))
  assert.ok(pkg.activationEvents.includes("onLanguage:json"))
  const source = fs.readFileSync(new URL("../src/extension.ts", import.meta.url), "utf8")
  assert.match(source, /\{ scheme: "file", pattern: JSONLD_PATTERN \}/)
  assert.match(source, /const JSONLD_PATTERN = "\*\*\/\*\.jsonld"/)
})

test("suggestions pop up in Turtle and TriG strings, where SHACL queries are typed", () => {
  const defaults = pkg.contributes.configurationDefaults
  for (const lang of ["[turtle]", "[trig]"]) {
    assert.deepEqual(defaults?.[lang]?.["editor.quickSuggestions"], { other: "on", comments: "off", strings: "on" }, lang)
  }
  // Other languages keep VS Code's defaults.
  assert.ok(!("editor.quickSuggestions" in defaults))
  assert.deepEqual(
    Object.keys(defaults).filter((k) => defaults[k]["editor.quickSuggestions"]),
    ["[turtle]", "[trig]"]
  )
})

test("the manifest declares exactly the settings the server applies", () => {
  assert.ok(serverKeys.length >= 14, `found ${serverKeys.join(", ")} in src/settings.rs`)
  const declared = Object.keys(settings)
    .map((k) => k.replace(/^turtle\./, ""))
    // Editor-only settings, not sent to the server.
    .filter((k) => !k.startsWith("server.") && !k.startsWith("trace."))
    .sort()
  assert.deepEqual(declared, serverKeys)
})

test("settings the server limits or fetches with are restricted in untrusted workspaces", () => {
  for (const key of [
    "turtle.server.path",
    "turtle.imports.network",
    "turtle.query.timeoutSeconds",
    "turtle.query.maxRows",
    "turtle.query.maxBufferedRows",
    "turtle.shacl.maxTriples",
    "turtle.diagnostics.workspace",
  ]) {
    assert.ok(restricted.includes(key), `${key} is not in restrictedConfigurations`)
    assert.ok(settings[key], `${key} is not a setting`)
  }
  assert.match(pkg.capabilities.untrustedWorkspaces.description, /turtle\.query/)
})

// Every setting set at the user level, as VS Code's inspect() reports it.
const allSet = (key) => ({ globalValue: settings[`turtle.${key}`].default })

test("every server setting reaches the server, and only the user's values do", () => {
  const flat = (o, prefix = "") =>
    Object.entries(o).flatMap(([k, v]) => (v && typeof v === "object" && !Array.isArray(v) && k !== "prefixes" ? flat(v, `${prefix}${k}.`) : [`${prefix}${k}`]))
  const sent = settingsFrom({ properties: settings, restricted, inspect: allSet, trusted: true })
  assert.deepEqual(flat(sent).sort(), serverKeys)
  // Unset settings: nothing is sent, so turtle-lsp.json decides.
  assert.deepEqual(settingsFrom({ properties: settings, restricted, inspect: () => ({}), trusted: true }), {})
})

test("an untrusted workspace turns network imports off and ignores workspace values of restricted settings", () => {
  const inspect = (key) => {
    if (key === "imports.network") return { globalValue: true, workspaceValue: true }
    if (key === "query.maxRows") return { globalValue: 50, workspaceValue: 99999999 }
    if (key === "format.lineWidth") return { workspaceValue: 60 }
    return {}
  }
  const untrusted = settingsFrom({ properties: settings, restricted, inspect, trusted: false })
  assert.deepEqual(untrusted, { imports: { network: false }, query: { maxRows: 50 }, format: { lineWidth: 60 } })
  const trusted = settingsFrom({ properties: settings, restricted, inspect, trusted: true })
  assert.deepEqual(trusted, { imports: { network: true }, query: { maxRows: 99999999 }, format: { lineWidth: 60 } })
})

test("a setting left at its default is not sent, so turtle-lsp.json can decide (format.lineWidth)", () => {
  const sent = settingsFrom({ properties: settings, restricted, inspect: () => ({ defaultValue: 100 }), trusted: true })
  assert.equal(sent.format, undefined)
})
