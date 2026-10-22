-- turtle-lsp for Neovim 0.11+: enable it with vim.lsp.enable("turtle_lsp").
-- The server is the one build.lua put in this plugin's bin/, or else
-- turtle-lsp on PATH (installed by hand or by Mason).
local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h:h")
local bundled = root .. "/bin/turtle-lsp" .. (vim.fn.has("win32") == 1 and ".exe" or "")

---@type vim.lsp.Config
return {
  cmd = { vim.fn.executable(bundled) == 1 and bundled or "turtle-lsp" },
  filetypes = { "turtle", "trig", "ntriples", "nquads", "jsonld", "sparql" },
  -- In priority order: the nearest turtle-lsp.json, else the repository
  -- root, else the nearest prefixes.ttl. A prefixes.ttl in a subfolder is
  -- part of the project, not a project of its own.
  root_markers = { "turtle-lsp.json", ".git", "prefixes.ttl" },
}
