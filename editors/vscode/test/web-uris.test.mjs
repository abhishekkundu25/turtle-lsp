// The server worker's URI mapping (src/web/uris.ts): workspace folders are
// file:///workspace/<n> to the server, on every URI a message carries.
import { test } from "node:test"
import assert from "node:assert/strict"
import { uriMap } from "../src/web/uris.ts"

const REPO = "vscode-vfs://github/owner/repo"
const map = uriMap([REPO])
const S = "file:///workspace/0"
const range = { start: { line: 1, character: 2 }, end: { line: 1, character: 5 } }

// `client` as the server sees it is `server`, and back.
const roundTrip = (m, client, server) => {
  assert.deepEqual(m.toServer(client), server)
  assert.deepEqual(m.toClient(server), client)
}

test("a hover request and its answer", () => {
  roundTrip(map, { textDocument: { uri: `${REPO}/a.ttl` }, position: range.start }, { textDocument: { uri: `${S}/a.ttl` }, position: range.start })
  const answer = { contents: { kind: "markdown", value: "**ex:a**" }, range }
  assert.deepEqual(map.toClient(answer), answer)
})

test("a server URI inside prose (a hover's link) is the client's", () => {
  const hover = (root) => ({ contents: { kind: "markdown", value: `**ex:a**\n\n_Described in [b.ttl:1](${root}/b.ttl#L1) and [c.ttl](${root}/c.ttl)_` } })
  assert.deepEqual(map.toClient(hover(S)), hover(REPO))
  const two = uriMap(["vscode-vfs://github/o/one", "vscode-vfs://github/o/two"])
  const tenth = { message: "see file:///workspace/10/x.ttl" }
  assert.deepEqual(uriMap(Array.from({ length: 2 }, (_, i) => `vscode-vfs://github/o/r${i}`)).toClient(tenth), tenth)
  assert.deepEqual(two.toClient({ message: "in file:///workspace/1/x.ttl" }), { message: "in vscode-vfs://github/o/two/x.ttl" })
  // Document content stays as written, even with a server URI in it.
  const edit = { changes: { [`${S}/a.ttl`]: [{ range, newText: `<${S}/b.ttl> a ex:C .` }] } }
  assert.deepEqual(map.toClient(edit), { changes: { [`${REPO}/a.ttl`]: [{ range, newText: `<${S}/b.ttl> a ex:C .` }] } })
})

test("a diagnostic's message, a shown message and an inlay hint's tooltip are prose", () => {
  const diag = (root) => ({ uri: `${root}/a.ttl`, diagnostics: [{ range, message: `Import <http://example.org/o> names ${root}/o.ttl: not found` }] })
  assert.deepEqual(map.toClient(diag(S)), diag(REPO))
  assert.deepEqual(map.toClient({ type: 1, message: `cannot read ${S}/b.ttl` }), { type: 1, message: `cannot read ${REPO}/b.ttl` })
  assert.deepEqual(map.toClient([{ position: range.start, label: "x", tooltip: `<${S}/b.ttl#x>` }]), [{ position: range.start, label: "x", tooltip: `<${REPO}/b.ttl#x>` }])
})

test("any other string is mapped only when it is a URI as a whole", () => {
  const item = { label: `${S}/b.ttl#x`, detail: `"see ${S}/a.ttl"`, documentation: { kind: "markdown", value: `[b](${S}/b.ttl)` } }
  assert.deepEqual(map.toClient(item), { label: `${REPO}/b.ttl#x`, detail: `"see ${S}/a.ttl"`, documentation: { kind: "markdown", value: `[b](${REPO}/b.ttl)` } })
})

test("turtle/query terms: a uri term is mapped, literals and blank nodes never", () => {
  const select = (root) => ({
    kind: "select",
    head: { vars: ["s", "o"] },
    results: {
      bindings: [
        { s: { type: "uri", value: `${root}/a.ttl#x` }, o: { type: "literal", value: `see ${S}/x` } },
        { s: { type: "bnode", value: `${S}/b` }, o: { type: "literal", value: `${S}/x`, datatype: "http://www.w3.org/2001/XMLSchema#anyURI" } },
      ],
    },
    omitted: 0,
    elapsedMs: 1,
  })
  assert.deepEqual(map.toClient(select(S)), select(REPO))
  const construct = (root) => ({ kind: "construct", triples: [{ subject: { type: "uri", value: `${root}/a.ttl#x` }, predicate: { type: "uri", value: "http://example.org/p" }, object: { type: "literal", value: `${S}/a.ttl`, "xml:lang": "en" } }] })
  assert.deepEqual(map.toClient(construct(S)), construct(REPO))
})

test("a definition answer, as locations and as location links", () => {
  roundTrip(map, [{ uri: `${REPO}/defs/b.ttl`, range }], [{ uri: `${S}/defs/b.ttl`, range }])
  roundTrip(map, [{ targetUri: `${REPO}/b.ttl`, targetRange: range, targetSelectionRange: range }], [{ targetUri: `${S}/b.ttl`, targetRange: range, targetSelectionRange: range }])
})

test("a workspace edit: the keys of `changes` and the documents of `documentChanges`", () => {
  const edit = (root) => ({
    changes: { [`${root}/a.ttl`]: [{ range, newText: "ex:b" }], [`${root}/b.ttl`]: [{ range, newText: "ex:b" }] },
    documentChanges: [{ textDocument: { uri: `${root}/a.ttl`, version: 3 }, edits: [{ range, newText: "ex:b" }] }, { kind: "rename", oldUri: `${root}/x.ttl`, newUri: `${root}/y.ttl` }],
  })
  roundTrip(map, edit(REPO), edit(S))
})

test("a diagnostics notification, related information included", () => {
  const note = (root) => ({
    jsonrpc: "2.0",
    method: "textDocument/publishDiagnostics",
    params: { uri: `${root}/a.ttl`, diagnostics: [{ range, message: "m", relatedInformation: [{ location: { uri: `${root}/b.ttl`, range }, message: "here" }] }] },
  })
  roundTrip(map, note(REPO), note(S))
})

test("the folder itself, as in the initialize params", () => {
  roundTrip(map, { rootUri: REPO, workspaceFolders: [{ uri: REPO, name: "repo" }] }, { rootUri: S, workspaceFolders: [{ uri: S, name: "repo" }] })
  // A trailing slash on the folder's URI makes no difference.
  assert.equal(uriMap([`${REPO}/`]).toServer(`${REPO}/a.ttl`), `${S}/a.ttl`)
  assert.equal(uriMap(["vscode-test-web://mount/"]).toServer("vscode-test-web://mount/a.ttl"), `${S}/a.ttl`)
})

test("untitled documents and URIs outside the folders pass through", () => {
  for (const uri of ["untitled:Untitled-1", "file:///etc/a.ttl", "vscode-vfs://github/owner/other/a.ttl", "https://example.org/a"]) {
    roundTrip(map, { uri }, { uri })
  }
})

test("a folder's URI is a prefix only up to a path boundary", () => {
  const m = uriMap([REPO, `${REPO}2`])
  roundTrip(m, [`${REPO}/a.ttl`, `${REPO}2/a.ttl`, `${REPO}2`], [`${S}/a.ttl`, "file:///workspace/1/a.ttl", "file:///workspace/1"])
  // workspace/1 is not a prefix of workspace/10.
  const many = uriMap(Array.from({ length: 11 }, (_, i) => `vscode-test-web://m${i}`))
  assert.equal(many.toClient("file:///workspace/10/a.ttl"), "vscode-test-web://m10/a.ttl")
  assert.equal(many.toClient("file:///workspace/1/a.ttl"), "vscode-test-web://m1/a.ttl")
})

test("two folders, one inside the other: the inner one wins", () => {
  const m = uriMap([REPO, `${REPO}/sub`])
  roundTrip(m, [`${REPO}/a.ttl`, `${REPO}/sub/b.ttl`], [`${S}/a.ttl`, "file:///workspace/1/b.ttl"])
})

test("percent escapes match in either letter case and are kept past the folder", () => {
  const m = uriMap(["vscode-vfs://github/owner/my%20repo"])
  assert.equal(m.toServer("vscode-vfs://github/owner/my%20repo/a%20b.ttl"), `${S}/a%20b.ttl`)
  assert.equal(uriMap(["vscode-test-web://mount/c%3Ad"]).toServer("vscode-test-web://mount/c%3ad/x.ttl"), `${S}/x.ttl`)
  assert.equal(m.toClient(`${S}/a%20b.ttl`), "vscode-vfs://github/owner/my%20repo/a%20b.ttl")
})

test("document text is not mapped, even when it starts with a folder's URI", () => {
  const open = { textDocument: { uri: `${REPO}/a.ttl`, languageId: "turtle", version: 1, text: `${REPO}/a.ttl` } }
  assert.deepEqual(map.toServer(open), { textDocument: { ...open.textDocument, uri: `${S}/a.ttl` } })
  const edits = [{ range, newText: `${S}/x` }]
  assert.deepEqual(map.toClient(edits), edits)
})

test("the message given is not changed", () => {
  const m = { uri: `${REPO}/a.ttl` }
  map.toServer(m)
  assert.deepEqual(m, { uri: `${REPO}/a.ttl` })
})
