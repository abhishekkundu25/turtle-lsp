// The base for the graph's "changes since HEAD" mode (src/git-base.ts), with a
// fake of the git extension's API.
import { test } from "node:test"
import assert from "node:assert/strict"
import { gitBase } from "../src/git-base.ts"

const uri = (p) => ({ fsPath: p, toString: () => `file://${p}` })
const change = (status, p, from = p) => ({ status, uri: uri(p), originalUri: uri(from) })
const MODIFIED = 5, DELETED = 6, UNTRACKED = 7, IGNORED = 8, INDEX_ADDED = 1, INDEX_RENAMED = 3
const HEAD = { "/w/a.ttl": "HEAD a", "/w/old.ttl": "HEAD old", "/w/gone.nt": "HEAD gone", "/w/s.ttl": "HEAD s", "/w/notes.md": "x", "/w/q.rq": "q" }

function api({ working = [], index = [], untracked = [] } = {}, rootPath = "/w") {
  const shown = []
  const repo = {
    rootUri: uri(rootPath),
    state: { workingTreeChanges: working, indexChanges: index, untrackedChanges: untracked },
    async show(ref, p) {
      shown.push([ref, p])
      if (!(p in HEAD)) throw new Error(`not in ${ref}: ${p}`)
      return HEAD[p]
    },
  }
  return { api: { repositories: [repo] }, shown }
}
const sorted = (r) => [...r].sort((a, b) => a.uri.localeCompare(b.uri))

test("changed, staged, untracked, renamed and deleted files, once each", async () => {
  const { api: a } = api({
    working: [change(MODIFIED, "/w/a.ttl"), change(DELETED, "/w/gone.nt")],
    index: [change(MODIFIED, "/w/s.ttl"), change(INDEX_RENAMED, "/w/new.ttl", "/w/old.ttl"), change(INDEX_ADDED, "/w/added.trig")],
    untracked: [change(UNTRACKED, "/w/fresh.ttl")],
  })
  assert.deepEqual(sorted(await gitBase(a, ["/w"])), [
    { uri: "file:///w/a.ttl", text: "HEAD a" },
    { uri: "file:///w/added.trig", text: null },
    { uri: "file:///w/fresh.ttl", text: null },
    { uri: "file:///w/gone.nt", text: "HEAD gone" },
    { uri: "file:///w/new.ttl", text: null },
    { uri: "file:///w/old.ttl", text: "HEAD old" },
    { uri: "file:///w/s.ttl", text: "HEAD s" },
  ])
})

test("a file staged and edited again is listed once", async () => {
  const { api: a, shown } = api({ working: [change(MODIFIED, "/w/a.ttl")], index: [change(MODIFIED, "/w/a.ttl")] })
  assert.deepEqual(await gitBase(a, ["/w"]), [{ uri: "file:///w/a.ttl", text: "HEAD a" }])
  assert.deepEqual(shown, [["HEAD", "/w/a.ttl"]])
})

test("files the server does not read as data, ignored files and files outside the workspace are skipped", async () => {
  const { api: a } = api({
    working: [change(MODIFIED, "/w/notes.md"), change(MODIFIED, "/w/q.rq"), change(IGNORED, "/w/i.ttl"), change(MODIFIED, "/other/a.ttl")],
    untracked: [change(UNTRACKED, "/w/Upper.TTL")],
  })
  assert.deepEqual(await gitBase(a, ["/w"]), [{ uri: "file:///w/Upper.TTL", text: null }])
})

test("no git extension, no repository for the workspace, or a failing show gives a reason", async () => {
  assert.match(await gitBase(undefined, ["/w"]), /git extension/)
  assert.match(await gitBase({ repositories: [] }, ["/w"]), /No git repository/)
  assert.match(await gitBase(api({}, "/elsewhere").api, ["/w"]), /No git repository/)
  const { api: a } = api({ working: [change(MODIFIED, "/w/lost.ttl")] })
  assert.match(await gitBase(a, ["/w"]), /could not give the HEAD version.*not in HEAD/)
})

test("a repository above the workspace folder covers it", async () => {
  const { api: a } = api({ working: [change(MODIFIED, "/w/a.ttl")] }, "/")
  assert.deepEqual(await gitBase(a, ["/w"]), [{ uri: "file:///w/a.ttl", text: "HEAD a" }])
})
