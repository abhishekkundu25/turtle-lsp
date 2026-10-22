-- Puts the turtle-lsp server for this machine in bin/, for Neovim. lazy.nvim
-- runs this file when it installs or updates the plugin; without lazy.nvim,
-- run `nvim -l build.lua` in this directory.
--
-- The server comes from the GitHub release of this checkout's version
-- (Cargo.toml), checked against that release's SHA256SUMS and, when the `gh`
-- command is installed, against the release's build attestation. A machine
-- with no release download for it (or no such release yet) builds the server
-- with cargo instead. The old binary stays until the new one is complete.
-- TURTLE_LSP_RELEASE_URL replaces the release's download URL (for tests and
-- mirrors); downloads from it are not held to https or attested.

local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h")
local windows = vim.fn.has("win32") == 1
local exe = windows and "turtle-lsp.exe" or "turtle-lsp"

-- lazy.nvim runs this file inside Neovim, where print() goes to its log;
-- under `nvim -l` it goes to stdout.
local function say(msg)
  print("turtle-lsp: " .. msg)
end

local REPO = "abhishekkundu25/turtle-lsp"
local override = os.getenv("TURTLE_LSP_RELEASE_URL")
if override == "" then
  override = nil
end
-- Time limits in milliseconds: nothing here may hang an install for ever.
local LIMIT = { download = 300000, hash = 60000, extract = 120000, verify = 120000, build = 1800000, probe = 10000 }

-- Runs `cmd`, killing it after `limit`; returns vim.system's result.
local function exec(cmd, limit, opts)
  local r = vim.system(cmd, vim.tbl_extend("force", { text = true, timeout = limit }, opts or {})):wait()
  if r.code == 124 and r.signal ~= 0 then
    error(table.concat(cmd, " ") .. " did not finish within " .. math.floor(limit / 1000) .. " s", 0)
  end
  return r
end

local function run(cmd, limit, opts)
  local r = exec(cmd, limit, opts)
  if r.code ~= 0 then
    error(table.concat(cmd, " ") .. " failed: " .. vim.trim((r.stderr ~= "" and r.stderr) or r.stdout or ""), 0)
  end
  return r.stdout
end

local function version()
  for line in io.lines(root .. "/Cargo.toml") do
    local v = line:match('^version%s*=%s*"([^"]+)"')
    if v then
      return v
    end
  end
  error("no version in Cargo.toml", 0)
end

-- The release asset for this machine, if there is one.
local function asset()
  local u = vim.uv.os_uname()
  local os_name, arch = u.sysname, u.machine
  local arm = arch == "arm64" or arch == "aarch64"
  local x64 = arch == "x86_64" or arch == "AMD64" or arch == "x64"
  if os_name == "Darwin" then
    return arm and "turtle-lsp-darwin-arm64.tar.gz" or x64 and "turtle-lsp-darwin-x64.tar.gz" or nil
  elseif os_name == "Linux" then
    return arm and "turtle-lsp-linux-arm64.tar.gz" or x64 and "turtle-lsp-linux-x64.tar.gz" or nil
  elseif windows then
    return x64 and "turtle-lsp-win32-x64.zip" or nil
  end
end

local function sha256(file)
  if windows then
    local out = run({ "certutil", "-hashfile", file, "SHA256" }, LIMIT.hash)
    return (out:match("\n(%x[%x ]+)\r?\n") or ""):gsub("%s", ""):lower()
  end
  local cmd = vim.fn.executable("sha256sum") == 1 and { "sha256sum", file } or { "shasum", "-a", "256", file }
  return run(cmd, LIMIT.hash):match("^(%x+)")
end

-- Downloads base/name to dest. Returns true, or false when the server has no
-- such file (HTTP 404); any other failure raises.
local function fetch(base, name, dest)
  local cmd = { "curl", "-sSL", "--connect-timeout", "30", "--max-time", tostring(math.max(1, math.floor(LIMIT.download / 1000) - 20)) }
  if not override then
    -- Release downloads are https only, also across redirects.
    vim.list_extend(cmd, { "--proto", "=https", "--proto-redir", "=https", "--tlsv1.2" })
  end
  vim.list_extend(cmd, { "-o", dest, "-w", "%{http_code}", base .. "/" .. name })
  local status = tonumber(vim.trim(run(cmd, LIMIT.download)))
  if status == 200 then
    return true
  end
  os.remove(dest)
  if status == 404 then
    return false
  end
  error("downloading " .. base .. "/" .. name .. " failed: HTTP " .. tostring(status), 0)
end

-- With the `gh` command installed, checks the archive against the build
-- attestation the release workflow published: SHA256SUMS comes from the same
-- release as the archive and proves nothing on its own.
local function attest(archive)
  if override then
    say("warning: TURTLE_LSP_RELEASE_URL is set; the download is not checked for a build attestation")
    return
  end
  if vim.fn.executable("gh") ~= 1 then
    say("gh not found; skipping the build attestation check (install gh to verify downloads)")
    return
  end
  local r = exec({ "gh", "attestation", "verify", archive, "--repo", REPO }, LIMIT.verify)
  if r.code ~= 0 then
    error(
      "gh attestation verify rejected " .. vim.fs.basename(archive) .. ": " .. vim.trim((r.stderr ~= "" and r.stderr) or r.stdout or "")
        .. " (is gh signed in? `gh auth login`)",
      0
    )
  end
  say("build attestation verified")
end

-- Moves the finished binary into bin/, replacing the old one only now.
local function install(from)
  local to = root .. "/bin/" .. exe
  local ok, err = vim.uv.fs_rename(from, to)
  if not ok then
    error("could not replace " .. to .. ": " .. tostring(err) .. " (is the server running? stop it and try again)", 0)
  end
end

-- A directory under bin/ for this run's downloads and extraction.
local function scratch()
  local dir = string.format("%s/bin/.install-%d-%d", root, vim.uv.os_getpid(), math.floor(vim.uv.hrtime() / 1e6))
  vim.fn.mkdir(dir, "p")
  return dir
end

-- The release's server into bin/. Returns true, or false and the reason when
-- there is no download for this machine's release.
local function download()
  local name = asset()
  if not name then
    return false, "there is no release download for this machine"
  end
  local base = override or ("https://github.com/" .. REPO .. "/releases/download/v" .. version())
  if override then
    say("warning: downloading from TURTLE_LSP_RELEASE_URL, not from GitHub's release over https")
  end
  vim.fn.mkdir(root .. "/bin", "p")
  local tmp = scratch()
  local ok, res, why = pcall(function()
    local archive = tmp .. "/" .. name
    say("downloading " .. name .. " from " .. base)
    if not fetch(base, name, archive) then
      return false, "there is no release asset " .. name .. " at " .. base .. " (HTTP 404)"
    end
    if not fetch(base, "SHA256SUMS", tmp .. "/SHA256SUMS") then
      return false, "there is no SHA256SUMS at " .. base .. " (HTTP 404)"
    end
    local sums = io.open(tmp .. "/SHA256SUMS"):read("*a")
    local want = (sums .. "\n"):match("(%x+)%s+%*?" .. vim.pesc(name) .. "%s")
    local got = sha256(archive)
    if not want or got ~= want then
      error("the checksum of " .. name .. " does not match SHA256SUMS", 0)
    end
    attest(archive)
    local out = tmp .. "/out"
    vim.fn.mkdir(out, "p")
    run({ "tar", "-xf", archive, "-C", out }, LIMIT.extract)
    if vim.fn.filereadable(out .. "/" .. exe) ~= 1 then
      error(name .. " has no " .. exe, 0)
    end
    if not windows then
      vim.uv.fs_chmod(out .. "/" .. exe, tonumber("755", 8))
    end
    install(out .. "/" .. exe)
    return true
  end)
  vim.fn.delete(tmp, "rf")
  if not ok then
    error(res, 0)
  end
  return res, why
end

local function build(reason)
  if vim.fn.executable("cargo") ~= 1 then
    error(
      (reason or "there is no release download for this machine")
        .. ", and cargo is not installed to build the server: install Rust (https://rustup.rs) and run this again",
      0
    )
  end
  say((reason and reason .. "; " or "") .. "building with cargo (a few minutes)")
  run({ "cargo", "build", "--release", "--locked" }, LIMIT.build, { cwd = root })
  vim.fn.mkdir(root .. "/bin", "p")
  local built = root .. "/target/release/" .. exe
  local staged = root .. "/bin/." .. exe .. ".new"
  local ok, err = vim.uv.fs_copyfile(built, staged)
  if not ok then
    os.remove(staged)
    error("could not copy " .. built .. " to bin/: " .. tostring(err), 0)
  end
  if not windows then
    vim.uv.fs_chmod(staged, tonumber("755", 8))
  end
  local installed, why = pcall(install, staged)
  os.remove(staged)
  if not installed then
    error(why, 0)
  end
end

local ok, why = download()
if not ok then
  build(why)
end
local v = exec({ root .. "/bin/" .. exe, "--version" }, LIMIT.probe)
say("installed " .. vim.trim(v.stdout ~= "" and v.stdout or v.stderr))
