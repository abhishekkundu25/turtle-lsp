-- turtle-lsp for Neovim 0.11+: enable it with vim.lsp.enable("turtle_lsp").
-- The server is the one build.lua put in this plugin's bin/, or else
-- turtle-lsp on PATH (installed by hand or by Mason).
local root = vim.fn.fnamemodify(debug.getinfo(1, "S").source:sub(2), ":p:h:h")
local bundled = root .. "/bin/turtle-lsp" .. (vim.fn.has("win32") == 1 and ".exe" or "")

---@type vim.lsp.Config
return {
  cmd = { vim.fn.executable(bundled) == 1 and bundled or "turtle-lsp" },
  filetypes = { "turtle", "trig", "ntriples", "nquads", "jsonld", "sparql" },
  root_markers = { "prefixes.ttl", ".git" },
}
