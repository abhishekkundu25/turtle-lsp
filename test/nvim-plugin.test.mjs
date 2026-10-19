// The repository as a Neovim plugin (build.lua, lsp/turtle_lsp.lua,
// plugin/turtle_lsp.lua), in a headless, isolated Neovim: build.lua installs
// the server from a release (served here from a local copy of the built
// server), refuses a download whose checksum is wrong, and the installed
// server attaches with vim.lsp.enable("turtle_lsp").
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawn, spawnSync, execFileSync } from "node:child_process"
import crypto from "node:crypto"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { serverPath } from "./lsp-client.mjs"

const repo = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const hasNvim = spawnSync("nvim", ["--version"]).status === 0
// The release asset build.lua asks for on this machine.
const ASSET = {
  "darwin-arm64": "turtle-lsp-darwin-arm64.tar.gz",
  "darwin-x64": "turtle-lsp-darwin-x64.tar.gz",
  "linux-arm64": "turtle-lsp-linux-arm64.tar.gz",
  "linux-x64": "turtle-lsp-linux-x64.tar.gz",
}[`${process.platform}-${process.arch}`]
const skip = !hasNvim ? "nvim not found on PATH" : !ASSET ? "no release asset for this platform" : false

let home, release, server, base
before(async () => {
  if (skip) return
  home = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-nvim-plugin-"))
  // A release: the built server as this machine's archive, and SHA256SUMS.
  release = path.join(home, "release")
  fs.mkdirSync(path.join(release, "stage"), { recursive: true })
  fs.copyFileSync(serverPath, path.join(release, "stage", "turtle-lsp"))
  execFileSync("tar", ["-czf", path.join(release, ASSET), "-C", path.join(release, "stage"), "turtle-lsp"])
  const sum = crypto.createHash("sha256").update(fs.readFileSync(path.join(release, ASSET))).digest("hex")
  fs.writeFileSync(path.join(release, "SHA256SUMS"), `${sum}  ${ASSET}\n`)
  fs.writeFileSync(path.join(release, "BAD"), `${"0".repeat(64)}  ${ASSET}\n`)
  server = http.createServer((req, res) => {
    // /good/<file> serves the release; /bad/<file> the same with a wrong checksum.
    const [, kind, name] = req.url.split("/")
    const file = path.join(release, kind === "bad" && name === "SHA256SUMS" ? "BAD" : name)
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

// A copy of the plugin's files, as a plugin manager would check them out.
function checkout(name) {
  const dir = path.join(home, name)
  fs.mkdirSync(dir)
  for (const f of ["build.lua", "Cargo.toml", "lsp", "plugin"]) fs.cpSync(path.join(repo, f), path.join(dir, f), { recursive: true })
  return dir
}

// Neovim, isolated from the user's config and data, asynchronously (the
// release is served by this process).
function nvim(args, env = {}) {
  return new Promise((resolve) => {
    const p = spawn("nvim", ["--headless", "--clean", ...args], {
      env: { ...process.env, XDG_CONFIG_HOME: home, XDG_DATA_HOME: home, XDG_STATE_HOME: home, ...env },
    })
    let out = ""
    p.stdout.on("data", (d) => (out += d))
    p.stderr.on("data", (d) => (out += d))
    const timer = setTimeout(() => p.kill(), 60_000)
    p.on("close", (code) => (clearTimeout(timer), resolve({ code, out })))
  })
}

test("build.lua installs the release's server in bin/, and vim.lsp.enable attaches it", { skip }, async () => {
  const dir = checkout("good")
  const build = await nvim(["-l", path.join(dir, "build.lua")], { TURTLE_LSP_RELEASE_URL: `${base}/good` })
  assert.equal(build.code, 0, build.out)
  assert.match(build.out, /installed turtle-lsp \d+\.\d+\.\d+/)
  const bin = path.join(dir, "bin", "turtle-lsp")
  assert.ok(fs.existsSync(bin), "bin/turtle-lsp")
  assert.deepEqual(fs.readdirSync(path.join(dir, "bin")), ["turtle-lsp"], "the archive is removed")

  const ttl = path.join(home, "a.ttl")
  fs.writeFileSync(ttl, "@prefix ex: <https://e.org/> .\nex:a ex:b ex:c .\n")
  const probe = `
    vim.lsp.enable("turtle_lsp")
    vim.cmd.edit(${JSON.stringify(ttl)})
    vim.wait(10000, function() return #vim.lsp.get_clients({ bufnr = 0 }) > 0 end)
    local c = vim.lsp.get_clients({ bufnr = 0 })[1]
    io.stdout:write("RESULT " .. vim.json.encode({
      ft = vim.bo.filetype, modeline = vim.bo.modeline,
      name = c and c.name, cmd = c and c.config.cmd[1],
      nq = vim.filetype.match({ filename = "x.nq" }),
    }) .. "\\n")
    vim.cmd("qa!")`
  const r = await nvim(["--cmd", `set rtp^=${dir}`, "-c", `lua ${probe.replace(/\n\s*/g, " ")}`])
  const line = r.out.split("\n").find((l) => l.startsWith("RESULT "))
  assert.ok(line, r.out)
  assert.deepEqual(JSON.parse(line.slice(7)), { ft: "turtle", modeline: false, name: "turtle_lsp", cmd: bin, nq: "nquads" })
})

test("build.lua refuses a download whose checksum does not match SHA256SUMS", { skip }, async () => {
  const dir = checkout("bad")
  const build = await nvim(["-l", path.join(dir, "build.lua")], { TURTLE_LSP_RELEASE_URL: `${base}/bad` })
  assert.notEqual(build.code, 0)
  assert.match(build.out, /checksum of turtle-lsp-.+ does not match SHA256SUMS/)
  assert.ok(!fs.existsSync(path.join(dir, "bin", "turtle-lsp")), "nothing installed")
})
