// install.sh (macOS, Linux) and install.ps1 (Windows) install the server from
// a release: here a local copy of the built server, served as this machine's
// release archive with its SHA256SUMS. A download whose checksum is wrong, or
// that is missing, installs nothing; an update replaces the installed binary;
// the last line on stdout is the installed binary's path. No test reaches the
// internet.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawn, spawnSync } from "node:child_process"
import crypto from "node:crypto"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { serverPath } from "./lsp-client.mjs"
import { ASSET, EXE, pack } from "./nvim-release.mjs"

const repo = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const windows = process.platform === "win32"
const skip = !ASSET ? "no release asset for this platform" : false

let home, release, server, base
before(async () => {
  if (skip) return
  home = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-install-"))
  release = path.join(home, "release")
  fs.mkdirSync(path.join(release, "stage"), { recursive: true })
  fs.copyFileSync(serverPath, path.join(release, "stage", EXE))
  pack(path.join(release, "stage"), path.join(release, ASSET))
  const sum = crypto.createHash("sha256").update(fs.readFileSync(path.join(release, ASSET))).digest("hex")
  // Other names around this machine's, as a real SHA256SUMS lists them.
  fs.writeFileSync(
    path.join(release, "SHA256SUMS"),
    `${"1".repeat(64)}  turtle-lsp-x-${ASSET}\n${sum}  ${ASSET}\n${"2".repeat(64)}  ${ASSET}.vsix\n`,
  )
  fs.writeFileSync(path.join(release, "BAD"), `${"0".repeat(64)}  ${ASSET}\n`)
  fs.writeFileSync(path.join(release, "OTHER"), `${sum}  turtle-lsp-other.tar.gz\n`)
  server = http.createServer((req, res) => {
    // /good/<file> serves the release; /bad/ the same with a wrong checksum;
    // /other/ a SHA256SUMS without this machine's archive; /gone/ nothing.
    const [, kind, name] = req.url.split("/")
    if (kind === "gone") return res.writeHead(404).end()
    const stand = name === "SHA256SUMS" ? { bad: "BAD", other: "OTHER" }[kind] ?? name : name
    const file = path.join(release, stand)
    if (!fs.existsSync(file)) return res.writeHead(404).end()
    res.writeHead(200)
    fs.createReadStream(file).pipe(res)
  })
  await new Promise((r) => server.listen(0, "127.0.0.1", r))
  base = `http://127.0.0.1:${server.address().port}`
})
after(async () => {
  if (skip) return
  await new Promise((r) => server.close(r))
  fs.rmSync(home, { recursive: true, force: true })
})

// Runs this platform's install script with the release at `kind` into `dir`.
// Not spawnSync: the release is served from this process.
function install(kind, dir) {
  const env = {
    ...process.env,
    HOME: home,
    TURTLE_LSP_RELEASE_URL: `${base}/${kind}`,
    TURTLE_LSP_INSTALL_DIR: dir,
  }
  delete env.TURTLE_LSP_VERSION
  delete env.TURTLE_LSP_VERIFY
  const [cmd, args] = windows
    ? ["powershell.exe", ["-NoProfile", "-ExecutionPolicy", "Bypass", "-File", path.join(repo, "install.ps1")]]
    : ["sh", [path.join(repo, "install.sh")]]
  return new Promise((resolve) => {
    const p = spawn(cmd, args, { env, stdio: ["ignore", "pipe", "pipe"] })
    let out = ""
    let err = ""
    p.stdout.on("data", (d) => (out += d))
    p.stderr.on("data", (d) => (err += d))
    p.on("close", (code) => resolve({ code, out, err, last: out.trim().split(/\r?\n/).at(-1) }))
  })
}

const version = () => spawnSync(serverPath, ["--version"], { encoding: "utf8" }).stdout.trim()

test("installs the release's binary, checked against SHA256SUMS, and prints its path last", { skip }, async () => {
  const dir = path.join(home, "good")
  const r = await install("good", dir)
  assert.equal(r.code, 0, r.out + r.err)
  const exe = path.join(dir, EXE)
  assert.equal(path.resolve(r.last), path.resolve(exe), r.out)
  assert.equal(spawnSync(exe, ["--version"], { encoding: "utf8" }).stdout.trim(), version())
  assert.match(r.err, /installed turtle-lsp \d/)
  // Only the binary: no temporary file is left beside it.
  assert.deepEqual(fs.readdirSync(dir), [EXE])
  // Run again, it replaces the installed binary.
  const again = await install("good", dir)
  assert.equal(again.code, 0, again.out + again.err)
  assert.deepEqual(fs.readdirSync(dir), [EXE])
})

test("a wrong checksum, a missing download or a SHA256SUMS without the archive installs nothing", { skip }, async () => {
  for (const [kind, why] of [
    ["bad", /does not match SHA256SUMS/],
    ["gone", /could not download/],
    ["other", /has no line for/],
  ]) {
    const dir = path.join(home, kind)
    const r = await install(kind, dir)
    assert.notEqual(r.code, 0, `${kind}: ${r.out}${r.err}`)
    assert.match(r.err, why, kind)
    assert.ok(!fs.existsSync(path.join(dir, EXE)), `${kind}: installed anyway`)
  }
})

test("a folder not on PATH is named, with the way to run the binary", { skip }, async () => {
  const dir = path.join(home, "off-path")
  const r = await install("good", dir)
  assert.equal(r.code, 0, r.err)
  assert.match(r.err, /is not on PATH/)
})
