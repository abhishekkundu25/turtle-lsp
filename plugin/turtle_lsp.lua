-- File types turtle-lsp serves that Neovim does not detect itself. Neovim
-- already knows .trig, .nt, .rq and .sparql; .ru stays Ruby (Rack). A
-- .jsonld file gets its own type, so plain .json buffers stay unattached.
-- A .ofn file is OWL Functional syntax, which the server reads as RDF and
-- never writes. Neovim knows no .owx (OWL/XML): it is XML, as .owl is.
vim.filetype.add({
  extension = { ttl = "turtle", turtle = "turtle", nq = "nquads", jsonld = "jsonld", ofn = "ofn", owx = "xml" },
})

-- "@prefix ex: <...>" and "PREFIX ex: <...>" look like a modeline (" ex:")
-- and raise E518 on open, so these files read no modelines. So does
-- "SubClassOf(ex:A ex:B)" in OWL Functional syntax: a .ofn, or a .owl or
-- .rdf, whose syntax its text gives once read. So does a .owx, whose
-- literals and comments can hold " ex:".
vim.api.nvim_create_autocmd("BufReadPre", {
  group = vim.api.nvim_create_augroup("turtle_lsp_no_modeline", { clear = true }),
  pattern = { "*.ttl", "*.turtle", "*.trig", "*.rq", "*.sparql", "*.ofn", "*.owl", "*.rdf", "*.owx" },
  callback = function()
    vim.opt_local.modeline = false
  end,
})

-- RDF/XML files (.owl, .rdf) keep Neovim's xml file type, OWL/XML files
-- (.owx) get it, and `filetypes` in lsp/turtle_lsp.lua names file types
-- exactly: naming xml there would attach the server to every XML file. So
-- the client starts on these files by name, when
-- vim.lsp.enable("turtle_lsp") is on, with its configuration and root (the
-- client of the project's Turtle files is reused). The server
-- reads them by their name and content and edits none of them. vim.lsp.enable's own
-- FileType handler detaches the client from them (its `filetypes` lacks
-- xml) when a file type is set again or another server is enabled after
-- startup: the client comes back unless it is stopping, has exited or is
-- disabled (a detach by hand, vim.lsp.buf_detach_client, is undone too). A
-- .owl buffer opened before vim.lsp.enable("turtle_lsp") gets no client
-- until it is read again (:edit).
local function xml(buf)
  local name = vim.api.nvim_buf_get_name(buf):lower()
  return name:match("%.owl$") or name:match("%.rdf$") or name:match("%.owx$")
end

local function start_xml(buf)
  local lsp = vim.lsp
  if not (lsp.is_enabled and lsp.is_enabled("turtle_lsp")) then
    return
  end
  if not vim.api.nvim_buf_is_loaded(buf) or vim.bo[buf].buftype ~= "" or not xml(buf) then
    return
  end
  local config = lsp.config.turtle_lsp
  if not config then
    return
  end
  config = vim.deepcopy(config)
  local function start(root)
    config.root_dir = root
    lsp.start(config, { bufnr = buf, reuse_client = config.reuse_client })
  end
  if type(config.root_dir) == "function" then
    config.root_dir(buf, function(root)
      vim.schedule(function()
        start(root)
      end)
    end)
  elseif config.root_dir then
    start(config.root_dir)
  else
    start(config.root_markers and vim.fs.root(buf, config.root_markers) or nil)
  end
end

-- In any letter case, also with 'nofileignorecase' (Linux's default).
local xml_patterns = { "*.[oO][wW][lL]", "*.[rR][dD][fF]", "*.[oO][wW][xX]" }
local xml_group = vim.api.nvim_create_augroup("turtle_lsp_xml", { clear = true })
vim.api.nvim_create_autocmd({ "BufReadPost", "BufNewFile" }, {
  group = xml_group,
  pattern = xml_patterns,
  callback = function(args)
    start_xml(args.buf)
  end,
})
vim.api.nvim_create_autocmd("LspDetach", {
  group = xml_group,
  pattern = xml_patterns,
  callback = function(args)
    local client = vim.lsp.get_client_by_id(args.data.client_id)
    if not client or client.name ~= "turtle_lsp" or client:is_stopped() then
      return
    end
    vim.schedule(function()
      if vim.api.nvim_buf_is_valid(args.buf) and not client:is_stopped() then
        start_xml(args.buf)
      end
    end)
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
