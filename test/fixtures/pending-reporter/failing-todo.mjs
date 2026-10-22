// Run by test/pending-reporter.test.mjs, never by the suite glob: pending
// tests that all still fail.
import { test } from "node:test"
import assert from "node:assert/strict"

test("still pending", { todo: "fixture:pending" }, () => assert.fail("not yet"))
