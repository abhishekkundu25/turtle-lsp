-- Drives the real Neovim LSP client against the built server.
-- Run: nvim --headless --clean -u test/nvim/spec.lua  (env: TURTLE_LSP_SERVER, TURTLE_LSP_FIXTURES)
-- Prints one "ok - name" / "not ok - name: reason" line per check, then exits 0 or 1.

local server = assert(os.getenv("TURTLE_LSP_SERVER"), "TURTLE_LSP_SERVER unset")
local fixtures = vim.fs.normalize(assert(os.getenv("TURTLE_LSP_FIXTURES"), "TURTLE_LSP_FIXTURES unset"))
local ws = fixtures .. "/workspace"
local real = fixtures .. "/real"
local project = fixtures .. "/project"
local TIMEOUT = 10000

vim.cmd("filetype on")
vim.filetype.add({ extension = { ttl = "turtle" } })
-- "@prefix ex: <...>" looks like a Vim modeline (" ex:") and raises E518 on open.
-- Turtle buffers need modelines off; see README "Neovim setup".
vim.api.nvim_create_autocmd("BufReadPre", {
  pattern = "*.ttl",
  callback = function()
    vim.opt_local.modeline = false
  end,
})
vim.lsp.config("turtle", {
  cmd = { server },
  filetypes = { "turtle" },
  root_markers = { "prefixes.ttl", ".git" },
})
vim.lsp.enable("turtle")

local failed = 0
local function check(name, fn)
  local ok, err = pcall(fn)
  if ok then
    io.stdout:write("ok - " .. name .. " ::\n")
  else
    failed = failed + 1
    io.stdout:write("not ok - " .. name .. " :: " .. (tostring(err):gsub("\n", " | ")) .. "\n")
  end
end

local client_errors = {}
local exits = {}
vim.api.nvim_create_autocmd("LspDetach", {
  callback = function(ev)
    table.insert(exits, ev.data.client_id)
  end,
})
local orig_notify = vim.notify
vim.notify = function(msg, level, ...)
  if level and level >= vim.log.levels.WARN then
    table.insert(client_errors, msg)
  end
  return orig_notify(msg, level, ...)
end

local function open(path)
  vim.cmd.edit(vim.fn.fnameescape(path))
  local buf = vim.api.nvim_get_current_buf()
  local attached = vim.wait(TIMEOUT, function()
    return #vim.lsp.get_clients({ bufnr = buf, name = "turtle" }) > 0
      and vim.lsp.get_clients({ bufnr = buf, name = "turtle" })[1].initialized
  end, 20)
  assert(attached, "client did not attach/initialize within timeout")
  return buf, vim.lsp.get_clients({ bufnr = buf, name = "turtle" })[1]
end

local function wait_diagnostics(buf)
  -- diagnostics arrive asynchronously; wait until the server has published for this buffer
  local ns_seen = vim.wait(TIMEOUT, function()
    return vim.b[buf].turtle_published == true
  end, 20)
  assert(ns_seen, "no publishDiagnostics for buffer")
  return vim.diagnostic.get(buf)
end

-- mark buffers when the server publishes diagnostics for them (even an empty list)
local orig_handler = vim.lsp.handlers["textDocument/publishDiagnostics"]
vim.lsp.handlers["textDocument/publishDiagnostics"] = function(err, result, ctx, cfg)
  local buf = vim.uri_to_bufnr(result.uri)
  vim.b[buf].turtle_published = true
  return orig_handler(err, result, ctx, cfg)
end

-- Neovim 0.11 pulls diagnostics (textDocument/diagnostic) from servers that
-- offer it; the server then publishes nothing. Mark buffers on each report.
local orig_pull_handler = vim.lsp.handlers["textDocument/diagnostic"]
vim.lsp.handlers["textDocument/diagnostic"] = function(err, result, ctx, cfg)
  local ret = orig_pull_handler(err, result, ctx, cfg)
  if not err and result then
    vim.b[vim.uri_to_bufnr(ctx.params.textDocument.uri)].turtle_published = true
  end
  return ret
end

local function request(buf, method, params)
  local res, err = vim.lsp.buf_request_sync(buf, method, params, TIMEOUT)
  assert(res, "request " .. method .. " failed: " .. tostring(err))
  for _, r in pairs(res) do
    assert(not r.err, method .. " error: " .. vim.inspect(r.err))
    return r.result
  end
end

local function pos_of(buf, needle)
  for i, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    local s = line:find(needle, 1, true)
    if s then
      return { line = i - 1, character = s - 1 + #needle }
    end
  end
  error("needle not found: " .. needle)
end

local function fmt(diags)
  local out = {}
  for _, d in ipairs(diags) do
    table.insert(out, string.format("%d:%d %s", d.lnum, d.col, d.message))
  end
  return table.concat(out, "; ")
end

local function problems(diags)
  return vim.tbl_filter(function(d)
    return d.severity <= vim.diagnostic.severity.WARN
  end, diags)
end

------------------------------------------------------------------------

local people = ws .. "/people.ttl"
local org = ws .. "/org.ttl"
local buf, client

check("client attaches and initializes", function()
  buf, client = open(people)
end)

check("server stays alive after open", function()
  vim.wait(500)
  assert(#exits == 0, "client detached/exited")
  assert(not client:is_stopped(), "client stopped")
end)

check("valid file shows no warnings in Neovim", function()
  local d = problems(wait_diagnostics(buf))
  assert(#d == 0, fmt(d))
end)

check("hover on foaf:Person shows documentation", function()
  local r = request(buf, "textDocument/hover", {
    textDocument = vim.lsp.util.make_text_document_params(buf),
    position = pos_of(buf, "a foaf:Pers"),
  })
  assert(r and r.contents, "no hover")
  local lines = vim.lsp.util.convert_input_to_markdown_lines(r.contents)
  assert(table.concat(lines, ""):find("person", 1, true) or table.concat(lines, ""):find("Person", 1, true), "unexpected hover: " .. vim.inspect(lines))
end)

check("completion after 'foaf:' offers foaf:Person, bounded", function()
  vim.api.nvim_buf_set_lines(buf, -1, -1, false, { "ex:carol a foaf:" })
  local last = vim.api.nvim_buf_line_count(buf) - 1
  local r = request(buf, "textDocument/completion", {
    textDocument = vim.lsp.util.make_text_document_params(buf),
    position = { line = last, character = #"ex:carol a foaf:" },
  })
  local items = r.items or r
  local found = false
  for _, it in ipairs(items) do
    if it.label == "foaf:Person" then
      found = true
    end
  end
  assert(found, "foaf:Person missing among " .. #items)
  assert(#items < 1000, #items .. " items")
  vim.cmd("silent! undo")
  vim.api.nvim_buf_set_lines(buf, last, last + 1, false, {})
end)

check("completing an undeclared well-known prefix inserts @prefix when applied", function()
  local scratch = ws .. "/__nvim_autoimport.ttl"
  local b = open(scratch)
  vim.api.nvim_buf_set_lines(b, 0, -1, false, { "@prefix ex: <http://example.org/> .", "ex:a a skos:Conc" })
  local r = request(b, "textDocument/completion", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = { line = 1, character = #"ex:a a skos:Conc" },
  })
  local item
  for _, it in ipairs(r.items or r) do
    if it.label == "skos:Concept" then
      item = it
    end
  end
  assert(item, "skos:Concept not offered")
  if item.textEdit then
    vim.lsp.util.apply_text_edits({ item.textEdit }, b, client.offset_encoding)
  end
  vim.lsp.util.apply_text_edits(item.additionalTextEdits or {}, b, client.offset_encoding)
  local text = table.concat(vim.api.nvim_buf_get_lines(b, 0, -1, false), "\n")
  assert(text:find("@prefix skos:", 1, true), "no @prefix skos inserted:\n" .. text)
  assert(text:find("ex:a a skos:Concept", 1, true), "term not completed:\n" .. text)
  vim.cmd("bwipeout!")
end)

check("go to definition from org.ttl lands on ex:alice in people.ttl", function()
  local b = open(org)
  local r = request(b, "textDocument/definition", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "ex:alic"),
  })
  local locs = r and (r.uri and { r } or r) or {}
  assert(#locs == 1, #locs .. " locations: " .. vim.inspect(locs))
  local loc = locs[1]
  local got = vim.fs.normalize(vim.uri_to_fname(loc.uri or loc.targetUri))
  assert(got:lower() == vim.fs.normalize(people):lower(), "wrong file " .. (loc.uri or loc.targetUri))
  local range = loc.range or loc.targetSelectionRange
  assert(range.start.line == 4, "wrong line " .. range.start.line)
end)

check("rename ex:bob applies cleanly through Neovim", function()
  vim.cmd.edit(vim.fn.fnameescape(people))
  local b = vim.api.nvim_get_current_buf()
  local edit = request(b, "textDocument/rename", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "ex:bo"),
    newName = "ex:robert",
  })
  assert(edit, "no workspace edit")
  local ok, err = pcall(vim.lsp.util.apply_workspace_edit, edit, client.offset_encoding)
  assert(ok, "apply failed: " .. tostring(err))
  local text = table.concat(vim.api.nvim_buf_get_lines(b, 0, -1, false), "\n")
  assert(not text:find("ex:bob", 1, true), "ex:bob still present")
  local _, n = text:gsub("ex:robert", "")
  assert(n == 2, "expected 2 occurrences of ex:robert, got " .. n .. ":\n" .. text)
  vim.cmd("silent! %bwipeout!")
end)

check("project fragment (prefixes from prefixes.ttl): no warnings", function()
  local b = open(project .. "/data/instances.ttl")
  local d = problems(wait_diagnostics(b))
  assert(#d == 0, #d .. " problems: " .. fmt(vim.list_slice(d, 1, 5)))
end)

-- test/fixtures/real is local-only (gitignored).
if vim.fn.isdirectory(real) == 1 then
  check("real fragment (prefixes from workspace prefixes.ttl): no warnings", function()
    local b = open(real .. "/design_guide_examples_c3-instance.ttl")
    local d = problems(wait_diagnostics(b))
    assert(#d == 0, #d .. " problems: " .. fmt(vim.list_slice(d, 1, 5)))
  end)
end

check("LSP folding works", function()
  local b = open(people)
  local r = request(b, "textDocument/foldingRange", { textDocument = vim.lsp.util.make_text_document_params(b) })
  assert(r and #r > 0, "no folding ranges")
end)

check("no LSP errors surfaced to the user", function()
  assert(#client_errors == 0, table.concat(client_errors, " || "))
end)

check("server still alive at end", function()
  assert(#exits == 0 or not client:is_stopped(), "client exited")
end)

vim.cmd((failed == 0) and "qall!" or "cquit! 1")
