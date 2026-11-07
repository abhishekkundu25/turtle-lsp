// The server worker's disk and editor layers (src/web/layers.ts): what the
// server is sent for each event, so that it sees the union of the files on
// disk and the editor's documents, the editor's text winning.
import { test } from "node:test"
import assert from "node:assert/strict"
import { Layers } from "../src/web/layers.ts"

const A = "vscode-test-web://mount/a.ttl"
const DISK = "@prefix ex: <http://example.org/> .\nex:a ex:p ex:b .\n"
const EDITED = "@prefix ex: <http://example.org/> .\nex:a ex:p ex:c .\n"

const opened = (uri, version, text, languageId = "") => ({ method: "textDocument/didOpen", params: { textDocument: { uri, languageId, version, text } } })
const whole = (uri, version, text) => ({ method: "textDocument/didChange", params: { textDocument: { uri, version }, contentChanges: [{ text }] } })
const closed = (uri) => ({ method: "textDocument/didClose", params: { textDocument: { uri } } })
const editorOpen = (l, text = EDITED, version = 1) => l.didOpen({ uri: A, languageId: "turtle", version, text })

test("a disk file not in the editor is opened, by its file name's language", () => {
  const l = new Layers()
  assert.deepEqual(l.diskChanged(A, DISK), [opened(A, 1, DISK)])
})

test("a document only the editor has is opened with its language", () => {
  const l = new Layers()
  assert.deepEqual(editorOpen(l), [opened(A, 1, EDITED, "turtle")])
})

test("opening a file known from disk is a full-text change, not a second open", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  assert.deepEqual(editorOpen(l), [whole(A, 2, EDITED)])
})

test("an edit goes through as the client sent it, under the server's next version", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  editorOpen(l, DISK, 7)
  const changes = [{ range: { start: { line: 1, character: 13 }, end: { line: 1, character: 14 } }, text: "c" }]
  assert.deepEqual(l.didChange(A, 8, changes), [{ method: "textDocument/didChange", params: { textDocument: { uri: A, version: 3 }, contentChanges: changes } }])
  // The edit is kept: closing puts the disk text back, so the server's text was EDITED.
  assert.deepEqual(l.didClose(A), [whole(A, 4, DISK)])
})

test("a change to a document the editor does not have is ignored", () => {
  assert.deepEqual(new Layers().didChange(A, 2, [{ text: "x" }]), [])
})

test("closing a file with disk text changes it back to the disk text instead of closing it", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  editorOpen(l)
  assert.deepEqual(l.didClose(A), [whole(A, 3, DISK)])
})

test("closing a file whose text is the disk text sends nothing", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  editorOpen(l, DISK)
  assert.deepEqual(l.didClose(A), [])
})

test("closing a document with no disk text closes it", () => {
  const l = new Layers()
  editorOpen(l)
  assert.deepEqual(l.didClose(A), [closed(A)])
  assert.deepEqual(l.didClose(A), [], "a second close sends nothing")
})

test("a disk change while the editor has the file changes nothing until it closes", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  editorOpen(l)
  assert.deepEqual(l.diskChanged(A, "# saved elsewhere\n"), [])
  assert.deepEqual(l.didClose(A), [whole(A, 3, "# saved elsewhere\n")])
})

test("a disk change to a closed file is a full-text change; the same text again is nothing", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  assert.deepEqual(l.diskChanged(A, EDITED), [whole(A, 2, EDITED)])
  assert.deepEqual(l.diskChanged(A, EDITED), [])
})

test("a file deleted while the editor has it stays; closing it later closes it", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  editorOpen(l)
  assert.deepEqual(l.diskDeleted(A), [])
  assert.deepEqual(l.didClose(A), [closed(A)])
})

test("a file deleted while closed is closed; deleting it again sends nothing", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  assert.deepEqual(l.diskDeleted(A), [closed(A)])
  assert.deepEqual(l.diskDeleted(A), [])
})

test("versions rise per URI across closes and reopens", () => {
  const l = new Layers()
  assert.deepEqual(editorOpen(l), [opened(A, 1, EDITED, "turtle")])
  l.didClose(A)
  assert.deepEqual(editorOpen(l), [opened(A, 2, EDITED, "turtle")])
  assert.deepEqual(l.diskChanged("vscode-test-web://mount/b.ttl", DISK), [opened("vscode-test-web://mount/b.ttl", 1, DISK)])
})

test("a restart replays the union, the editor's text winning", () => {
  const l = new Layers()
  const B = "vscode-test-web://mount/b.ttl"
  const U = "untitled:Untitled-1"
  l.diskChanged(A, DISK)
  l.diskChanged(B, DISK)
  editorOpen(l)
  l.didOpen({ uri: U, languageId: "turtle", version: 1, text: "ex:u ex:p ex:o ." })
  assert.deepEqual(l.replay(), [opened(A, 3, EDITED, "turtle"), opened(B, 2, DISK), opened(U, 2, "ex:u ex:p ex:o .", "turtle")])
  // Replaying again converges on the same documents.
  assert.deepEqual(
    l.replay().map((m) => [m.params.textDocument.uri, m.params.textDocument.text]),
    [
      [A, EDITED],
      [B, DISK],
      [U, "ex:u ex:p ex:o ."],
    ],
  )
})

test("a new client session forgets the editor's documents: the replay is the disk", () => {
  const l = new Layers()
  l.diskChanged(A, DISK)
  editorOpen(l)
  l.forgetEditor()
  assert.deepEqual(l.replay(), [opened(A, 3, DISK)])
  // The client opens it again: over disk, a change.
  assert.deepEqual(editorOpen(l), [whole(A, 4, EDITED)])
})

test("an edit computed by the server carries the client's version, or null when the client does not have the document", () => {
  const l = new Layers()
  const B = "vscode-test-web://mount/b.ttl"
  l.diskChanged(A, DISK)
  l.diskChanged(B, DISK)
  editorOpen(l, EDITED, 5)
  l.didChange(A, 6, [{ text: DISK }])
  const edit = (a, b) => ({
    documentChanges: [
      { textDocument: { uri: A, version: a }, edits: [] },
      { textDocument: { uri: B, version: b }, edits: [] },
      { kind: "create", uri: "vscode-test-web://mount/c.ttl" },
    ],
  })
  assert.deepEqual(l.forClient({ edit: edit(4, 1) }), { edit: edit(6, null) })
  // `changes` has no versions and is left as it is.
  const changes = { changes: { [A]: [] } }
  assert.deepEqual(l.forClient(changes), changes)
})
