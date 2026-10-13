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
    "turtle.format.lineWidth": ["integer", 100],
  }
  for (const [key, [type, def]] of Object.entries(expected)) {
    assert.ok(settings[key], `${key} missing`)
    assert.equal(settings[key].type, type, key)
    assert.equal(settings[key].default, def, key)
    assert.ok(settings[key].markdownDescription || settings[key].description, `${key} has no description`)
  }
  assert.deepEqual(settings["turtle.inlayHints.labels"].enum, ["opaque", "all", "off"])
})
