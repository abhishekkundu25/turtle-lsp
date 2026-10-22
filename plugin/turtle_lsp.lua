-- File types turtle-lsp serves that Neovim does not detect itself. Neovim
-- already knows .trig, .nt, .rq and .sparql; .ru stays Ruby (Rack). A
-- .jsonld file gets its own type, so plain .json buffers stay unattached.
vim.filetype.add({ extension = { ttl = "turtle", turtle = "turtle", nq = "nquads", jsonld = "jsonld" } })

-- "@prefix ex: <...>" and "PREFIX ex: <...>" look like a modeline (" ex:")
-- and raise E518 on open, so these files read no modelines.
vim.api.nvim_create_autocmd("BufReadPre", {
  group = vim.api.nvim_create_augroup("turtle_lsp_no_modeline", { clear = true }),
  pattern = { "*.ttl", "*.turtle", "*.trig", "*.rq", "*.sparql" },
  callback = function()
    vim.opt_local.modeline = false
  end,
})

-- A reference lens's command (arguments: the document URI, the lens position
-- and the reference locations): the references go to the quickfix list.
vim.lsp.commands["turtle.showReferences"] = function(command, ctx)
  local locations = (command.arguments or {})[3] or {}
  local client = vim.lsp.get_client_by_id(ctx.client_id)
  local encoding = client and client.offset_encoding or "utf-16"
  vim.fn.setqflist({}, " ", {
    title = command.title or "References",
    items = vim.lsp.util.locations_to_items(locations, encoding),
  })
  vim.cmd("botright copen")
end
