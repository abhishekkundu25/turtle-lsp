// Security-relevant parts of the extension manifest, and the settings the
// server reads.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"

const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"))
const settings = pkg.contributes.configuration.properties
const setting = settings["turtle.server.path"]

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
    "turtle.lint.undescribedTerms": ["boolean", true],
    "turtle.imports.network": ["boolean", false],
    "turtle.imports.unresolved": ["string", "soften"],
    "turtle.format.lineWidth": ["integer", 100],
    "turtle.codeLens.references": ["boolean", true],
    "turtle.diagnostics.workspace": ["boolean", false],
    "turtle.query.timeoutSeconds": ["number", 30],
    "turtle.query.maxRows": ["integer", 1000],
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
