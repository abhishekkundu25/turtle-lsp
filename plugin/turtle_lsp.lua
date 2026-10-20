-- File types turtle-lsp serves that Neovim does not detect itself. Neovim
-- already knows .trig, .nt, .rq and .sparql; .ru stays Ruby (Rack). A
-- .jsonld file gets its own type, so plain .json buffers stay unattached.
vim.filetype.add({ extension = { ttl = "turtle", nq = "nquads", jsonld = "jsonld" } })

-- "@prefix ex: <...>" and "PREFIX ex: <...>" look like a modeline (" ex:")
-- and raise E518 on open, so these files read no modelines.
vim.api.nvim_create_autocmd("BufReadPre", {
  group = vim.api.nvim_create_augroup("turtle_lsp_no_modeline", { clear = true }),
  pattern = { "*.ttl", "*.turtle", "*.trig", "*.rq", "*.sparql" },
  callback = function()
    vim.opt_local.modeline = false
  end,
})
