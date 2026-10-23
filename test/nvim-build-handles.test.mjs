// build.lua closes every file it opens. On Windows an open handle keeps a
// file from being deleted, so a SHA256SUMS left open stops build.lua from
// removing its scratch folder. Here every file opened with io.open or
// io.lines is recorded (io.lines closes its file only at the end of the
// file, not when the loop is left early) and, once build.lua has installed
// the server from a local release, none may still be open.
import { test, before, after } from "node:test"
import assert from "node:assert/strict"
import { spawn } from "node:child_process"
import crypto from "node:crypto"
import fs from "node:fs"
import http from "node:http"
import os from "node:os"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { serverPath } from "./lsp-client.mjs"
import { hasNvim, ASSET, EXE, pack } from "./nvim-release.mjs"

const repo = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const skip = !hasNvim ? "nvim not found on PATH" : !ASSET ? "no release asset for this platform" : false

// Runs build.lua (its path is the first argument) with io.open and io.lines
// recorded.
const WRAPPER = `
local build = _G.arg[1]
local real = io.open
local opened = {}
io.open = function(name, ...)
  local f, err = real(name, ...)
  if f then opened[#opened + 1] = { f = f, name = name } end
  return f, err
end
io.lines = function(name, ...)
  local f = assert(io.open(name))
  local next_line = f:lines(...)
  return function()
    local line = next_line()
    if line == nil then f:close() end
    return line
  end
end
local ok, err = pcall(dofile, build)
for _, o in ipairs(opened) do
  if io.type(o.f) == "file" then io.stdout:write("LEFT OPEN " .. o.name .. "\\n") end
end
io.stdout:write("OPENED " .. #opened .. "\\n")
io.stdout:write("BUILD " .. tostring(ok) .. " " .. tostring(err) .. "\\n")
`

let home, server, base
before(async () => {
  if (skip) return
  home = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-nvim-handles-"))
  const release = path.join(home, "release")
  fs.mkdirSync(path.join(release, "stage"), { recursive: true })
  fs.copyFileSync(serverPath, path.join(release, "stage", EXE))
  pack(path.join(release, "stage"), path.join(release, ASSET))
  const sum = crypto.createHash("sha256").update(fs.readFileSync(path.join(release, ASSET))).digest("hex")
  fs.writeFileSync(path.join(release, "SHA256SUMS"), `${sum}  ${ASSET}\n`)
  server = http.createServer((req, res) => {
    const file = path.join(release, path.basename(req.url))
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

function nvim(args, env) {
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

test("build.lua leaves no file open after installing a release", { skip }, async () => {
  const dir = path.join(home, "plugin")
  fs.mkdirSync(dir)
  for (const f of ["build.lua", "Cargo.toml"]) fs.cpSync(path.join(repo, f), path.join(dir, f))
  const wrapper = path.join(home, "wrapper.lua")
  fs.writeFileSync(wrapper, WRAPPER)
  const r = await nvim(["-l", wrapper, path.join(dir, "build.lua")], { TURTLE_LSP_RELEASE_URL: base })
  assert.match(r.out, /BUILD true/, r.out)
  assert.doesNotMatch(r.out, /LEFT OPEN/, r.out)
  assert.ok(fs.existsSync(path.join(dir, "bin", EXE)), r.out)
  assert.deepEqual(fs.readdirSync(path.join(dir, "bin")), [EXE], "the scratch folder is removed")
})
