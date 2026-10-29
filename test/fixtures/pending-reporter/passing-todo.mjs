// Run by test/pending-reporter.test.mjs, never by the suite glob: one pending
// test that passes and one that still fails.
import { test } from "node:test"
import assert from "node:assert/strict"

test("ready to promote", { todo: "fixture:ready" }, () => {})
test("still pending", { todo: "fixture:pending" }, () => assert.fail("not yet"))
