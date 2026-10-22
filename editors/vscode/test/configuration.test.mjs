// What the extension sends the server when the settings change. The server
// reads the `turtle` section of `settings` (as a client that sends the whole
// configuration does), so the user's settings go inside that section.
import { test } from "node:test"
import assert from "node:assert/strict"
import { configurationChange } from "../src/server.ts"

test("a settings change goes to the server as the turtle section of settings", () => {
  const settings = { format: { lineWidth: 80 }, imports: { network: false } }
  assert.deepEqual(configurationChange(settings), { settings: { turtle: settings } })
})

test("no settings set is an empty turtle section, not a bare object", () => {
  // An empty section still replaces the settings the server started with.
  assert.deepEqual(configurationChange({}), { settings: { turtle: {} } })
})
