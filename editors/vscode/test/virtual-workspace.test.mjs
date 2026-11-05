// Whether the desktop entry runs its server in a workspace with folders on a
// virtual file system (src/server.ts `planWorkspace`), and what it tells the user.
import { test } from "node:test"
import assert from "node:assert/strict"
import { planWorkspace } from "../src/server.ts"

const disk = (name) => ({ scheme: "file", authority: "", path: `/work/${name}`, name })
const github = (repo, authority = "github") => ({ scheme: "vscode-vfs", authority, path: `/${repo}`, name: repo.split("/")[1] })

test("an empty window and a workspace on disk run the server and say nothing", () => {
  assert.deepEqual(planWorkspace([]), { runServer: true, unserved: [] })
  assert.deepEqual(planWorkspace([disk("a"), disk("b")]), { runServer: true, unserved: [] })
})

test("a remote window (SSH, WSL, containers) counts as on disk: its extension host sees file: folders", () => {
  assert.deepEqual(planWorkspace([{ scheme: "file", authority: "", path: "/home/me/project", name: "project" }]), { runServer: true, unserved: [] })
})

test("a GitHub repository opened remotely runs no server and offers to clone it or open it in the browser", () => {
  const plan = planWorkspace([github("ex/data")])
  assert.equal(plan.runServer, false)
  assert.deepEqual(plan.unserved, ["vscode-vfs"])
  assert.deepEqual(plan.notice, {
    message: "Turtle features need the folder on disk. Clone it, or open it in vscode.dev or github.dev, where they run in the browser.",
    cloneUrl: "https://github.com/ex/data.git",
    browserUrl: "https://vscode.dev/github/ex/data",
  })
})

test("a branch in the authority (github+ref) still names the repository", () => {
  const plan = planWorkspace([github("ex/data", "github+7b22726566227d")])
  assert.equal(plan.notice.cloneUrl, "https://github.com/ex/data.git")
  assert.equal(plan.notice.browserUrl, "https://vscode.dev/github/ex/data")
})

test("an Azure Repos folder opens in vscode.dev; clone asks for the URL", () => {
  const plan = planWorkspace([{ scheme: "vscode-vfs", authority: "azurerepos", path: "/org/proj/repo", name: "repo" }])
  assert.equal(plan.runServer, false)
  assert.equal(plan.notice.browserUrl, "https://vscode.dev/azurerepos/org/proj/repo")
  assert.equal(plan.notice.cloneUrl, undefined)
})

test("another virtual file system runs no server and offers vscode.dev itself", () => {
  const plan = planWorkspace([{ scheme: "memfs", authority: "", path: "/", name: "mem" }])
  assert.equal(plan.runServer, false)
  assert.deepEqual(plan.unserved, ["memfs"])
  assert.equal(plan.notice.browserUrl, "https://vscode.dev")
  assert.equal(plan.notice.cloneUrl, undefined)
})

test("a mixed workspace runs the server for its folders on disk and names the virtual ones as not checked", () => {
  const plan = planWorkspace([disk("local"), github("ex/data")])
  assert.equal(plan.runServer, true)
  assert.deepEqual(plan.unserved, ["vscode-vfs"])
  assert.equal(
    plan.notice.message,
    "Turtle features need the folder on disk, so data is not checked. Clone it, or open it in vscode.dev or github.dev, where they run in the browser.",
  )
})

test("several virtual folders are named together, each scheme once", () => {
  const plan = planWorkspace([disk("local"), github("ex/a"), github("ex/b")])
  assert.deepEqual(plan.unserved, ["vscode-vfs"])
  assert.match(plan.notice.message, /^Turtle features need the folders on disk, so a, b are not checked\. Clone them, or open them in/)
  // The first virtual folder is the one offered.
  assert.equal(plan.notice.browserUrl, "https://vscode.dev/github/ex/a")
})

test("a virtual window that gains a folder on disk runs the server, and loses it again", () => {
  const vfs = { scheme: "vscode-vfs", authority: "github", path: "/owner/repo", name: "repo" }
  const disk = { scheme: "file", authority: "", path: "/home/u/data", name: "data" }
  assert.equal(planWorkspace([vfs]).runServer, false)
  assert.equal(planWorkspace([vfs, disk]).runServer, true)
  assert.equal(planWorkspace([vfs]).runServer, false)
})
