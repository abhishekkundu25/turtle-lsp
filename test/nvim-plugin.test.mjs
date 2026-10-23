// The repository as a Neovim plugin (build.lua, lsp/turtle_lsp.lua,
// plugin/turtle_lsp.lua), in a headless, isolated Neovim: build.lua installs
// the server from a release (served here from a local copy of the built
// server), refuses a download whose checksum is wrong, keeps the installed
// server when an update fails, falls back to cargo when the release has no
// asset, and the installed server attaches with vim.lsp.enable("turtle_lsp").
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
import { processesOf, leftRunning } from "./procs.mjs"
import { hasNvim, ASSET, EXE, pack, windowsTar } from "./nvim-release.mjs"

const repo = path.join(path.dirname(fileURLToPath(import.meta.url)), "..")
const skip = !hasNvim ? "nvim not found on PATH" : !ASSET ? "no release asset for this platform" : false

let home, release, server, base
// Every server binary these tests installed, for the leak check at the end.
const installed = []
const requests = []
before(async () => {
  if (skip) return
  home = fs.mkdtempSync(path.join(os.tmpdir(), "turtle-nvim-plugin-"))
  // A release: the built server as this machine's archive, and SHA256SUMS.
  release = path.join(home, "release")
  fs.mkdirSync(path.join(release, "stage"), { recursive: true })
  fs.copyFileSync(serverPath, path.join(release, "stage", EXE))
  pack(path.join(release, "stage"), path.join(release, ASSET))
  const sum = crypto.createHash("sha256").update(fs.readFileSync(path.join(release, ASSET))).digest("hex")
  fs.writeFileSync(path.join(release, "SHA256SUMS"), `${sum}  ${ASSET}\n`)
  fs.writeFileSync(path.join(release, "BAD"), `${"0".repeat(64)}  ${ASSET}\n`)
  // An archive that matches its checksum but cannot be extracted.
  const junk = Buffer.from("this is not an archive\n".repeat(40))
  fs.writeFileSync(path.join(release, "CORRUPT"), junk)
  fs.writeFileSync(path.join(release, "CORRUPT_SUMS"), `${crypto.createHash("sha256").update(junk).digest("hex")}  ${ASSET}\n`)
  server = http.createServer((req, res) => {
    // /good/<file> serves the release; /bad/<file> the same with a wrong checksum;
    // /gone/<file> has nothing (HTTP 404); /corrupt/<file> has an archive that does not unpack.
    const [, kind, name] = req.url.split("/")
    requests.push(req.url)
    if (kind === "gone") return res.writeHead(404).end()
    const stand = kind === "bad" && name === "SHA256SUMS" ? "BAD" : kind === "corrupt" ? (name === "SHA256SUMS" ? "CORRUPT_SUMS" : "CORRUPT") : name
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

// A copy of the plugin's files, as a plugin manager would check them out.
function checkout(name) {
  const dir = path.join(home, name)
  fs.mkdirSync(dir)
  for (const f of ["build.lua", "Cargo.toml", "lsp", "plugin"]) fs.cpSync(path.join(repo, f), path.join(dir, f), { recursive: true })
  installed.push(path.join(dir, "bin", EXE))
  return dir
}

// Neovim, isolated from the user's config and data, asynchronously (the
// release is served by this process).
function nvim(args, env = {}) {
  const inherited = { ...process.env }
  // Windows spells it Path: a second key, PATH, would leave which one counts to chance.
  if (env.PATH) for (const key of Object.keys(inherited)) if (key.toUpperCase() === "PATH") delete inherited[key]
  return new Promise((resolve) => {
    const p = spawn("nvim", ["--headless", "--clean", ...args], {
      env: { ...inherited, XDG_CONFIG_HOME: home, XDG_DATA_HOME: home, XDG_STATE_HOME: home, ...env },
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
  const bin = path.join(dir, "bin", EXE)
  assert.ok(fs.existsSync(bin), "bin/turtle-lsp")
  assert.deepEqual(fs.readdirSync(path.join(dir, "bin")), [EXE], "the archive is removed")

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
      jsonld = vim.filetype.match({ filename = "x.jsonld" }),
      json = vim.filetype.match({ filename = "x.json" }),
    }) .. "\\n")
    vim.cmd("qa!")`
  const r = await nvim(["--cmd", `set rtp^=${dir}`, "-c", `lua ${probe.replace(/\n\s*/g, " ")}`])
  const line = r.out.split("\n").find((l) => l.startsWith("RESULT "))
  assert.ok(line, r.out)
  const { cmd, ...got } = JSON.parse(line.slice(7))
  // Compared as files: on Windows Neovim joins with "/" and the temp folder may be a short (8.3) name.
  assert.ok(cmd, r.out)
  assert.equal(fs.realpathSync.native(cmd), fs.realpathSync.native(bin))
  assert.deepEqual(got, {
    ft: "turtle",
    modeline: false,
    name: "turtle_lsp",
    nq: "nquads",
    jsonld: "jsonld",
    json: "json",
  })
})

test("build.lua refuses a download whose checksum does not match SHA256SUMS", { skip }, async () => {
  const dir = checkout("bad")
  const build = await nvim(["-l", path.join(dir, "build.lua")], { TURTLE_LSP_RELEASE_URL: `${base}/bad` })
  assert.notEqual(build.code, 0)
  assert.match(build.out, /checksum of turtle-lsp-.+ does not match SHA256SUMS/)
  assert.ok(!fs.existsSync(path.join(dir, "bin", EXE)), "nothing installed")
})

// ---------------------------------------------------------------------------
// A PATH of our own: only the tools build.lua needs (so no cargo, no gh, unless
// a test adds a fake), in front of nothing else.

function onPath(tool) {
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    const file = path.join(dir, tool)
    if (dir && fs.existsSync(file)) return file
  }
}

const posix = process.platform !== "win32"
const fakeSkip = skip || (!posix && "needs a POSIX shell")

/**
 * A directory with the tools build.lua runs, plus `scripts` ({ name: shell
 * script }); returned as a PATH. On Windows, where a shell script is no tool,
 * the PATH is nvim's folder and the system folder (curl, tar, certutil), and
 * neither holds cargo.
 */
function toolbox(name, scripts = {}) {
  if (!posix) {
    assert.deepEqual(Object.keys(scripts), [], "fake tools need a POSIX shell: give the test fakeSkip")
    return [path.dirname(onPath("nvim.exe")), path.dirname(windowsTar)].join(path.delimiter)
  }
  const dir = path.join(home, name)
  fs.mkdirSync(dir)
  // (cp, basename, mkdir and sleep are for the fake scripts below; gzip for GNU
  // tar, which runs it to unpack a .tar.gz where bsdtar does not.)
  for (const tool of ["nvim", "curl", "tar", "gzip", "shasum", "sha256sum", "cp", "basename", "mkdir", "sleep"]) {
    const file = onPath(tool)
    if (file && !scripts[tool]) fs.symlinkSync(fs.realpathSync(file), path.join(dir, tool))
  }
  for (const [tool, script] of Object.entries(scripts)) {
    fs.writeFileSync(path.join(dir, tool), `#!/bin/sh\n${script}\n`, { mode: 0o755 })
  }
  return dir
}

// A curl that serves the release directory for any URL and logs its arguments.
const FAKE_CURL = `echo "$@" >> "$FAKE_LOG"
out=""; url=""
while [ $# -gt 0 ]; do
  case "$1" in
    -o) out="$2"; shift ;;
    -w|--connect-timeout|--max-time|--proto|--proto-redir) shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
cp "$FAKE_RELEASE/$(basename "$url")" "$out" 2>/dev/null && printf 200 || printf 404`

const log = () => path.join(home, "fake.log")
const fakeEnv = (tools, extra = {}) => ({ PATH: tools, FAKE_LOG: log(), FAKE_RELEASE: release, ...extra })

test("build.lua asks for the win32-x64 zip on Windows", { skip }, async () => {
  const dir = checkout("win")
  fs.writeFileSync(
    path.join(home, "as-windows.lua"),
    `vim.uv.os_uname = function() return { sysname = "Windows_NT", machine = "AMD64" } end
local has = vim.fn.has
vim.fn.has = function(f) if f == "win32" then return 1 end return has(f) end
dofile(os.getenv("BUILD_LUA"))`,
  )
  requests.length = 0
  // No cargo on this PATH: a 404 would end in an error, not in a build.
  const r = await nvim(["-l", path.join(home, "as-windows.lua")], {
    PATH: toolbox("tools-win"),
    BUILD_LUA: path.join(dir, "build.lua"),
    TURTLE_LSP_RELEASE_URL: `${base}/good`,
  })
  assert.ok(requests.includes("/good/turtle-lsp-win32-x64.zip"), `${requests.join(" ")}\n${r.out}`)
  assert.ok(!requests.some((u) => /\.tar\.gz$/.test(u)), requests.join(" "))
})

test("a release without the asset (404) and no cargo: a clear message, nothing installed", { skip }, async () => {
  const dir = checkout("gone-nocargo")
  const r = await nvim(["-l", path.join(dir, "build.lua")], { PATH: toolbox("tools-nocargo"), TURTLE_LSP_RELEASE_URL: `${base}/gone` })
  assert.notEqual(r.code, 0, r.out)
  assert.match(r.out, /there is no release asset turtle-lsp-.+ at .+\(HTTP 404\)/)
  assert.match(r.out, /cargo is not installed/)
  assert.match(r.out, /rustup\.rs/)
  assert.ok(!fs.existsSync(path.join(dir, "bin", EXE)), "nothing installed")
  assert.deepEqual(fs.readdirSync(path.join(dir, "bin")), [], "no download left behind")
})

test("a release without the asset (404) falls back to building with cargo", { skip: fakeSkip }, async () => {
  const dir = checkout("gone-cargo")
  const tools = toolbox("tools-cargo", {
    cargo: `echo "cargo $@ in $PWD" >> "$FAKE_LOG"
mkdir -p target/release && cp "$FAKE_SERVER" target/release/turtle-lsp`,
  })
  fs.rmSync(log(), { force: true })
  const r = await nvim(["-l", path.join(dir, "build.lua")], {
    ...fakeEnv(tools, { FAKE_SERVER: serverPath }),
    TURTLE_LSP_RELEASE_URL: `${base}/gone`,
  })
  assert.equal(r.code, 0, r.out)
  assert.match(r.out, /no release asset .*HTTP 404.*; building with cargo/)
  assert.match(fs.readFileSync(log(), "utf8"), /cargo build --release --locked in .*gone-cargo/)
  assert.match(r.out, /installed turtle-lsp \d+\.\d+\.\d+/)
  assert.deepEqual(fs.readdirSync(path.join(dir, "bin")), [EXE], "only the binary is left")
})

test("a cargo build that produces no binary is an error, not a silent success", { skip: fakeSkip }, async () => {
  const dir = checkout("cargo-empty")
  const tools = toolbox("tools-cargo-empty", { cargo: "exit 0" })
  const r = await nvim(["-l", path.join(dir, "build.lua")], { PATH: tools, TURTLE_LSP_RELEASE_URL: `${base}/gone` })
  assert.notEqual(r.code, 0, r.out)
  assert.match(r.out, /could not copy .*target\/release\/turtle-lsp to bin\//)
  assert.ok(!fs.existsSync(path.join(dir, "bin", EXE)))
})

test("a failed update leaves the installed server in place", { skip }, async () => {
  const dir = checkout("keep")
  const bin = path.join(dir, "bin", EXE)
  const first = await nvim(["-l", path.join(dir, "build.lua")], { TURTLE_LSP_RELEASE_URL: `${base}/good` })
  assert.equal(first.code, 0, first.out)
  const before = fs.readFileSync(bin)
  const tools = toolbox("tools-keep")
  for (const kind of ["gone", "bad", "corrupt"]) {
    const r = await nvim(["-l", path.join(dir, "build.lua")], { PATH: tools, TURTLE_LSP_RELEASE_URL: `${base}/${kind}` })
    assert.notEqual(r.code, 0, `${kind}: ${r.out}`)
    assert.ok(fs.existsSync(bin), `${kind}: the installed server was removed`)
    assert.ok(before.equals(fs.readFileSync(bin)), `${kind}: the installed server changed`)
    assert.deepEqual(fs.readdirSync(path.join(dir, "bin")), [EXE], `${kind}: leftovers`)
  }
})

test("a download that hangs is stopped, not waited for", { skip: fakeSkip }, async () => {
  const dir = checkout("hang")
  // A curl that never ends, and a build.lua whose limit is short.
  const tools = toolbox("tools-hang", { curl: "exec sleep 600" })
  const lua = fs.readFileSync(path.join(dir, "build.lua"), "utf8").replace(/download = \d+/, "download = 1500")
  assert.match(lua, /download = 1500/, "the limit table changed: update this test")
  fs.writeFileSync(path.join(dir, "build.lua"), lua)
  const started = Date.now()
  const r = await nvim(["-l", path.join(dir, "build.lua")], { PATH: tools, TURTLE_LSP_RELEASE_URL: `${base}/good` })
  assert.notEqual(r.code, 0, r.out)
  assert.match(r.out, /did not finish within \d+ s/)
  assert.ok(Date.now() - started < 20_000, "waited too long")
})

test("the real download goes over https with TLS 1.2 and is attested when gh is there", { skip: fakeSkip }, async () => {
  const dir = checkout("https")
  const bin = path.join(dir, "bin", EXE)
  const withGh = (code) => toolbox(`tools-gh${code}`, { curl: FAKE_CURL, gh: `echo "gh $@" >> "$FAKE_LOG"\nexit ${code}` })
  const version = fs.readFileSync(path.join(repo, "Cargo.toml"), "utf8").match(/^version\s*=\s*"([^"]+)"/m)[1]

  // gh rejects the archive: nothing is installed.
  fs.rmSync(log(), { force: true })
  let r = await nvim(["-l", path.join(dir, "build.lua")], fakeEnv(withGh(1)))
  assert.notEqual(r.code, 0, r.out)
  assert.match(r.out, /gh attestation verify rejected turtle-lsp-/)
  assert.ok(!fs.existsSync(bin), "installed a download gh rejected")

  // gh accepts it.
  fs.rmSync(log(), { force: true })
  r = await nvim(["-l", path.join(dir, "build.lua")], fakeEnv(withGh(0)))
  assert.equal(r.code, 0, r.out)
  const calls = fs.readFileSync(log(), "utf8")
  assert.match(calls, new RegExp(`--proto =https --proto-redir =https --tlsv1\\.2 -o .*${ASSET.replace(/\./g, "\\.")} -w %\\{http_code\\} https://github\\.com/abhishekkundu25/turtle-lsp/releases/download/v${version.replace(/\./g, "\\.")}/${ASSET.replace(/\./g, "\\.")}`))
  assert.match(calls, /^gh attestation verify .*turtle-lsp-.* --repo abhishekkundu25\/turtle-lsp$/m)
  assert.match(r.out, /build attestation verified/)
  assert.ok(fs.existsSync(bin))

  // Without gh: a note, and the install goes on.
  const dir2 = checkout("https-nogh")
  r = await nvim(["-l", path.join(dir2, "build.lua")], fakeEnv(toolbox("tools-nogh", { curl: FAKE_CURL })))
  assert.equal(r.code, 0, r.out)
  assert.match(r.out, /gh not found; skipping the build attestation check/)
})

test("TURTLE_LSP_RELEASE_URL downloads without the https flags, with a warning", { skip: fakeSkip }, async () => {
  const dir = checkout("override")
  fs.rmSync(log(), { force: true })
  const r = await nvim(["-l", path.join(dir, "build.lua")], fakeEnv(toolbox("tools-override", { curl: FAKE_CURL }), { TURTLE_LSP_RELEASE_URL: "http://mirror.invalid/rel" }))
  assert.equal(r.code, 0, r.out)
  assert.match(r.out, /warning: downloading from TURTLE_LSP_RELEASE_URL/)
  const calls = fs.readFileSync(log(), "utf8")
  assert.ok(!/--proto|--tlsv1/.test(calls), calls)
  assert.match(calls, /http:\/\/mirror\.invalid\/rel\/SHA256SUMS/)
})

test("no turtle-lsp installed by these tests is left running", { skip }, async () => {
  for (const bin of installed) {
    if (!fs.existsSync(bin)) continue
    const alive = await leftRunning(bin, processesOf(bin))
    assert.deepEqual(alive, [], `${bin} still running`)
  }
})
