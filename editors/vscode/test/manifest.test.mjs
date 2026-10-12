// Security-relevant parts of the extension manifest.
import { test } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"

const pkg = JSON.parse(fs.readFileSync(new URL("../package.json", import.meta.url), "utf8"))
const setting = pkg.contributes.configuration.properties["turtle.server.path"]

test("a workspace cannot choose which binary the extension runs", () => {
  // "machine" settings are ignored in workspace and folder settings files.
  assert.equal(setting.scope, "machine")
})

test("turtle.server.path is restricted in untrusted workspaces", () => {
  const cap = pkg.capabilities?.untrustedWorkspaces
  assert.equal(cap?.supported, "limited")
  assert.ok(cap.restrictedConfigurations.includes("turtle.server.path"))
})
