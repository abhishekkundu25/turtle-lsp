// Renaming a prefix that a shared prefixes.ttl declares renames it in every
// file that takes it from there, wherever the rename starts; files that
// declare the prefix themselves are left alone.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { startServer, tempDir, uriOf, positionOf } from "./lsp-client.mjs"

const files = {
  "prefixes.ttl": "@prefix ex: <http://example.org/> .\n@prefix taken: <http://taken.org/> .\n",
  "data.ttl": "ex:a ex:p ex:b .\n",
  "sub/deep.ttl": "ex:c ex:p ex:d .\n",
  "own.ttl": "@prefix ex: <http://other.org/> .\nex:e ex:p ex:f .\n",
  "nested/prefixes.ttl": "@prefix ex: <http://nested.org/> .\n",
  "nested/inner.ttl": "ex:g ex:p ex:h .\n",
  "clash/uses.ttl": "@prefix zz: <http://zz.org/> .\nex:i ex:p zz:j .\n",
}

let ws, s
const file = (name) => path.join(ws, name)
const uri = (name) => uriOf(file(name))
const text = (name) => fs.readFileSync(file(name), "utf8")
// A URI from the server as a workspace-relative name with forward slashes.
const nameOf = (u) => path.relative(ws, fileURLToPath(u)).split(path.sep).join("/")

before(async () => {
  ws = tempDir("turtle-prefix-rename-")
  for (const [name, body] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(file(name)), { recursive: true })
    fs.writeFileSync(file(name), body)
  }
  s = await startServer({ cwd: ws, initializationOptions: { prefixes: { fromsettings: "http://settings.org/" } } })
})
after(async () => {
  try {
    await s?.stop()
  } finally {
    fs.rmSync(ws, { recursive: true, force: true })
  }
})

async function rename(name, needle, newName, offset = 1) {
  const t = text(name)
  s.open(uri(name), t)
  try {
    return await s.request("textDocument/rename", {
      textDocument: { uri: uri(name) },
      position: positionOf(t, needle, offset),
      newName,
    })
  } finally {
    s.close(uri(name))
  }
}

// Edits per file, relative to the workspace, as "line:char text".
function edited(edit) {
  const out = {}
  for (const [u, edits] of Object.entries(edit.changes ?? {})) {
    const name = nameOf(u)
    out[name] = [...edits]
      .sort((a, b) => a.range.start.line - b.range.start.line || a.range.start.character - b.range.start.character)
      .map((e) => `${e.range.start.line}:${e.range.start.character} ${e.newText}`)
  }
  return out
}

function apply(t, edits) {
  const offset = (p) => t.split("\n").slice(0, p.line).join("\n").length + (p.line ? 1 : 0) + p.character
  for (const e of [...edits].sort((a, b) => offset(b.range.start) - offset(a.range.start))) {
    t = t.slice(0, offset(e.range.start)) + e.newText + t.slice(offset(e.range.end))
  }
  return t
}

const shared = {
  "prefixes.ttl": ["0:8 eg"],
  "data.ttl": ["0:0 eg", "0:5 eg", "0:10 eg"],
  "sub/deep.ttl": ["0:0 eg", "0:5 eg", "0:10 eg"],
  "clash/uses.ttl": ["1:0 eg", "1:5 eg"],
}

test("renaming a prefix in prefixes.ttl renames it in the files that take it from there", async () => {
  // own.ttl declares ex: itself, and nested/ has its own prefixes.ttl.
  const edit = await rename("prefixes.ttl", "ex:", "eg")
  assert.deepEqual(edited(edit), shared)
})

test("renaming a shared prefix from a file that uses it renames it everywhere too", async () => {
  assert.deepEqual(edited(await rename("data.ttl", "ex:a", "eg")), shared)
  assert.deepEqual(edited(await rename("sub/deep.ttl", "ex:c", "eg:")), shared)
})

test("after a shared rename no file has an undeclared prefix", async () => {
  const edit = await rename("prefixes.ttl", "ex:", "eg")
  const after = {}
  for (const [u, edits] of Object.entries(edit.changes)) {
    const name = nameOf(u)
    after[name] = apply(text(name), edits)
  }
  assert.equal(after["prefixes.ttl"], "@prefix eg: <http://example.org/> .\n@prefix taken: <http://taken.org/> .\n")
  assert.equal(after["data.ttl"], "eg:a eg:p eg:b .\n")
  // The edited buffers, open and unsaved, as the editor has them after the rename.
  s.open(uri("prefixes.ttl"), after["prefixes.ttl"])
  try {
    for (const name of ["data.ttl", "sub/deep.ttl", "clash/uses.ttl"]) {
      s.open(uri(name), after[name])
      const undeclared = (await s.diagnosticsFor(uri(name))).filter((d) => d.code === "undeclared-prefix")
      s.close(uri(name))
      assert.deepEqual(undeclared, [], name)
    }
  } finally {
    s.close(uri("prefixes.ttl"))
  }
})

test("a prefix the file declares itself is renamed only in that file", async () => {
  assert.deepEqual(edited(await rename("own.ttl", "ex:e", "eg")), {
    "own.ttl": ["0:8 eg", "1:0 eg", "1:5 eg", "1:10 eg"],
  })
})

test("a nested prefixes.ttl is its own scope", async () => {
  assert.deepEqual(edited(await rename("nested/inner.ttl", "ex:g", "eg")), {
    "nested/prefixes.ttl": ["0:8 eg"],
    "nested/inner.ttl": ["0:0 eg", "0:5 eg", "0:10 eg"],
  })
})

test("a shared rename to a name already declared is refused", async () => {
  // prefixes.ttl declares taken:, and clash/uses.ttl declares zz: itself.
  await assert.rejects(rename("prefixes.ttl", "ex:", "taken"), /taken:.*already declared/)
  await assert.rejects(rename("data.ttl", "ex:a", "zz"), /zz:.*already declared.*clash/)
})

test("a prefix from the turtle.prefixes setting cannot be renamed in a file", async () => {
  fs.writeFileSync(file("settings-use.ttl"), "fromsettings:a fromsettings:p fromsettings:b .\n")
  await assert.rejects(rename("settings-use.ttl", "fromsettings:a", "fs"), /turtle\.prefixes/)
})

test("a shared prefix can be renamed and then renamed back", async () => {
  // own.ttl declares ex: itself but never uses eg:, so renaming eg: back to
  // ex: does not touch it and must not be refused because of it.
  const saved = {}
  const save = async (edit) => {
    for (const [u, edits] of Object.entries(edit.changes)) {
      const name = nameOf(u)
      saved[name] ??= text(name)
      fs.writeFileSync(file(name), apply(text(name), edits))
    }
    s.notify("workspace/didChangeWatchedFiles", {
      changes: Object.keys(edit.changes).map((uri) => ({ uri, type: 2 })),
    })
  }
  try {
    await save(await rename("prefixes.ttl", "ex:", "eg"))
    const back = await rename("prefixes.ttl", "eg:", "ex")
    const expected = Object.fromEntries(
      Object.entries(shared).map(([name, edits]) => [name, edits.map((e) => e.replace(/ eg$/, " ex"))]),
    )
    assert.deepEqual(edited(back), expected)
    await save(back)
    for (const [name, original] of Object.entries(saved)) assert.equal(text(name), original, name)
  } finally {
    for (const [name, original] of Object.entries(saved)) fs.writeFileSync(file(name), original)
    s.notify("workspace/didChangeWatchedFiles", {
      changes: Object.keys(saved).map((name) => ({ uri: uri(name), type: 2 })),
    })
  }
})

test("a prefix is renamed only to a valid prefix name", async () => {
  // PN_PREFIX: starts with a letter, no trailing dot, no colon or space.
  for (const bad of ["1x", "a.", "-a", "_a", "a b", "a:b", "é!"]) {
    await assert.rejects(rename("data.ttl", "ex:a", bad), /not a valid prefix name/, bad)
  }
  for (const ok of ["a.b", "a-1", "é"]) {
    const edit = await rename("data.ttl", "ex:a", ok)
    assert.ok(edited(edit)["data.ttl"].every((e) => e.endsWith(` ${ok}`)), ok)
  }
})
