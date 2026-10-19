-- Puts the turtle-lsp server for this machine in bin/, for Neovim. lazy.nvim
-- runs this file when it installs or updates the plugin; without lazy.nvim,
-- run `nvim -l build.lua` in this directory.
--
-- The server comes from the GitHub release of this checkout's version
-- (Cargo.toml), checked against that release's SHA256SUMS. A machine with no
-- release download builds it with cargo instead.
-- TURTLE_LSP_RELEASE_URL replaces the release's download URL (for tests).

local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h")
local windows = vim.fn.has("win32") == 1
local exe = windows and "turtle-lsp.exe" or "turtle-lsp"

-- lazy.nvim runs this file inside Neovim, where print() goes to its log;
-- under `nvim -l` it goes to stdout.
local function say(msg)
  print("turtle-lsp: " .. msg)
end

local function run(cmd, opts)
  local r = vim.system(cmd, vim.tbl_extend("force", { text = true }, opts or {})):wait()
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
    local out = run({ "certutil", "-hashfile", file, "SHA256" })
    return (out:match("\n(%x[%x ]+)\r?\n") or ""):gsub("%s", ""):lower()
  end
  local cmd = vim.fn.executable("sha256sum") == 1 and { "sha256sum", file } or { "shasum", "-a", "256", file }
  return run(cmd):match("^(%x+)")
end

local function download()
  local name = asset()
  if not name then
    return false
  end
  local base = os.getenv("TURTLE_LSP_RELEASE_URL")
    or ("https://github.com/abhishekkundu25/turtle-lsp/releases/download/v" .. version())
  local bin = root .. "/bin"
  vim.fn.mkdir(bin, "p")
  local archive = bin .. "/" .. name
  say("downloading " .. name .. " from " .. base)
  run({ "curl", "-fsSL", "-o", archive, base .. "/" .. name })
  local sums = run({ "curl", "-fsSL", base .. "/SHA256SUMS" })
  local want = (sums .. "\n"):match("(%x+)%s+%*?" .. vim.pesc(name) .. "%s")
  local got = sha256(archive)
  if not want or got ~= want then
    os.remove(archive)
    error("the checksum of " .. name .. " does not match SHA256SUMS", 0)
  end
  os.remove(bin .. "/" .. exe)
  run({ "tar", "-xf", archive, "-C", bin })
  os.remove(archive)
  if not windows then
    vim.uv.fs_chmod(bin .. "/" .. exe, tonumber("755", 8))
  end
  return true
end

local function build()
  if vim.fn.executable("cargo") ~= 1 then
    error("there is no release download for this machine; install Rust (cargo) to build the server", 0)
  end
  say("building with cargo (a few minutes)")
  run({ "cargo", "build", "--release", "--locked" }, { cwd = root })
  vim.fn.mkdir(root .. "/bin", "p")
  vim.uv.fs_copyfile(root .. "/target/release/" .. exe, root .. "/bin/" .. exe)
end

if not download() then
  build()
end
local v = vim.system({ root .. "/bin/" .. exe, "--version" }, { text = true }):wait()
say("installed " .. vim.trim(v.stdout ~= "" and v.stdout or v.stderr))
