-- Drives the real Neovim LSP client against the built server.
-- Run: nvim --headless --clean -u test/nvim/spec.lua  (env: TURTLE_LSP_SERVER, TURTLE_LSP_FIXTURES)
-- Prints one "ok - name" / "not ok - name: reason" line per check, then exits 0 or 1.

local server = assert(os.getenv("TURTLE_LSP_SERVER"), "TURTLE_LSP_SERVER unset")
local fixtures = vim.fs.normalize(assert(os.getenv("TURTLE_LSP_FIXTURES"), "TURTLE_LSP_FIXTURES unset"))
local ws = fixtures .. "/workspace"
local real = fixtures .. "/real"
local project = fixtures .. "/project"
local datasets = fixtures .. "/datasets"
local query = fixtures .. "/query"
local TIMEOUT = 10000

-- The repository is the plugin under test: its plugin/ (file types, modelines,
-- the reference-lens command) and lsp/turtle_lsp.lua (filetypes, root markers)
-- are the shipped files, not a copy. Only the server's command is replaced,
-- and each client's exit is recorded.
local repo = vim.fs.dirname(vim.fs.dirname(fixtures))
vim.opt.runtimepath:prepend(repo)
vim.cmd("filetype on")
vim.cmd("runtime plugin/turtle_lsp.lua")
-- .ru is Ruby (Rack) by default; see README "Neovim setup".
vim.filetype.add({
  extension = { ru = "sparql" },
  filename = { ["config.ru"] = "ruby" },
})
vim.api.nvim_create_autocmd("BufReadPre", {
  pattern = { "*.ru" },
  callback = function()
    vim.opt_local.modeline = false
  end,
})
-- Each client's exit, by id: what Neovim's own exit report says.
local exited = {}
vim.lsp.config("turtle_lsp", {
  cmd = { server },
  on_exit = function(code, signal, id)
    exited[id] = { code = code, signal = signal }
  end,
})
vim.lsp.enable("turtle_lsp")

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

-- Remove from `errors`, after its first `from`, the report Neovim makes of
-- the exit `e` of a client called `name`, if it makes one: one message, so
-- another client's exit report stays.
local function drop_exit_report(errors, from, name, e)
  -- Neovim reports an exit unless it is code 0 with no signal or SIGTERM.
  if not e or (e.code == 0 and (e.signal == 0 or e.signal == 15)) then
    return
  end
  local report = string.format("Client %s quit with exit code %s and signal %s.", name, e.code, e.signal)
  for i = from + 1, #errors do
    if tostring(errors[i]):sub(1, #report) == report then
      table.remove(errors, i)
      return
    end
  end
end

local function open(path)
  vim.cmd.edit(vim.fn.fnameescape(path))
  local buf = vim.api.nvim_get_current_buf()
  local attached = vim.wait(TIMEOUT, function()
    return #vim.lsp.get_clients({ bufnr = buf, name = "turtle_lsp" }) > 0
      and vim.lsp.get_clients({ bufnr = buf, name = "turtle_lsp" })[1].initialized
  end, 20)
  assert(attached, "client did not attach/initialize within timeout")
  return buf, vim.lsp.get_clients({ bufnr = buf, name = "turtle_lsp" })[1]
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

check("a .trig buffer (filetype trig) attaches with no warnings", function()
  local b = open(datasets .. "/graphs.trig")
  assert(vim.bo[b].filetype == "trig", "filetype " .. vim.bo[b].filetype)
  local d = problems(wait_diagnostics(b))
  assert(#d == 0, #d .. " problems: " .. fmt(vim.list_slice(d, 1, 5)))
  local r = request(b, "textDocument/definition", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "a dx:Pers"),
  })
  local locs = r and (r.uri and { r } or r) or {}
  assert(#locs == 1, #locs .. " locations: " .. vim.inspect(locs))
  local got = vim.uri_to_fname(locs[1].uri or locs[1].targetUri)
  assert(got:match("vocab%.ttl$"), "wrong file " .. got)
end)

check("a .nt buffer (filetype ntriples) attaches with no warnings", function()
  local b = open(datasets .. "/people.nt")
  assert(vim.bo[b].filetype == "ntriples", "filetype " .. vim.bo[b].filetype)
  local d = problems(wait_diagnostics(b))
  assert(#d == 0, #d .. " problems: " .. fmt(vim.list_slice(d, 1, 5)))
end)

check("a .nq buffer (filetype nquads) attaches with no warnings", function()
  local b = open(query .. "/data.nq")
  assert(vim.bo[b].filetype == "nquads", "filetype " .. vim.bo[b].filetype)
  local d = problems(wait_diagnostics(b))
  assert(#d == 0, #d .. " problems: " .. fmt(vim.list_slice(d, 1, 5)))
end)

check("a .jsonld buffer (filetype jsonld) attaches with no diagnostics; a .json one does not", function()
  local dir = vim.fn.tempname()
  vim.fn.mkdir(dir, "p")
  local file = dir .. "/people.jsonld"
  vim.fn.writefile({ '{ "@id": "http://example.org/bob", "http://example.org/name": "Bob" }' }, file)
  local ok, err = pcall(function()
    local b = open(file)
    assert(vim.bo[b].filetype == "jsonld", "filetype " .. vim.bo[b].filetype)
    local d = wait_diagnostics(b)
    assert(#d == 0, #d .. " diagnostics: " .. fmt(vim.list_slice(d, 1, 5)))
    assert(vim.filetype.match({ filename = "package.json" }) == "json")
  end)
  vim.fn.delete(dir, "rf")
  assert(ok, err)
end)

-- An RDF/XML file in a folder of its own: its own client, as for the .jsonld above.
local owl_dir = vim.fn.tempname()
vim.fn.mkdir(owl_dir, "p")
local owl_file = owl_dir .. "/onto.owl"
vim.fn.writefile({
  '<?xml version="1.0"?>',
  '<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"',
  '         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"',
  '         xmlns:owl="http://www.w3.org/2002/07/owl#">',
  '  <owl:Class rdf:about="http://example.org/Widget">',
  "    <rdfs:comment>A thing with parts.</rdfs:comment>",
  "  </owl:Class>",
  '  <rdf:Description rdf:about="http://example.org/w1">',
  '    <rdf:type rdf:resource="http://example.org/Widget"/>',
  "  </rdf:Description>",
  "</rdf:RDF>",
}, owl_file)
local owl_buf

check("a .owl buffer keeps filetype xml, the client attaches and hover answers", function()
  local b, c = open(owl_file)
  owl_buf = b
  assert(vim.bo[b].filetype == "xml", "filetype " .. vim.bo[b].filetype)
  wait_diagnostics(b)
  local r = request(b, "textDocument/hover", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, 'owl:Class rdf:about="http://example.org/Wid'),
  })
  assert(r and r.contents, "no hover")
  local text = table.concat(vim.lsp.util.convert_input_to_markdown_lines(r.contents), "\n")
  assert(text:find("A thing with parts.", 1, true), "hover: " .. text)
  -- Neovim does not opt in to the registrations: formatting and rename are
  -- static, and the server writes nothing in a .owl.
  assert(c:supports_method("textDocument/formatting", b), "no formatting capability")
  local edits = request(b, "textDocument/formatting", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    options = { tabSize = 2, insertSpaces = true },
  })
  assert(edits == nil or #edits == 0, "formatting edits in a .owl: " .. vim.inspect(edits))
  local res = vim.lsp.buf_request_sync(b, "textDocument/rename", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, 'owl:Class rdf:about="http://example.org/Wid'),
    newName = "ex:Thing",
  }, TIMEOUT)
  local answer = res and res[c.id]
  local message = answer and answer.err and answer.err.message or vim.inspect(answer)
  assert(message:find("RDF/XML document is not edited", 1, true), "rename in a .owl: " .. message)
  -- An xml buffer by any other name gets no client.
  local xml = owl_dir .. "/pom.xml"
  vim.fn.writefile({ "<project/>" }, xml)
  vim.cmd.edit(vim.fn.fnameescape(xml))
  local other = vim.api.nvim_get_current_buf()
  assert(vim.bo[other].filetype == "xml", "filetype " .. vim.bo[other].filetype)
  vim.wait(200)
  assert(#vim.lsp.get_clients({ bufnr = other }) == 0, "a client attached to pom.xml")
  vim.api.nvim_buf_delete(other, { force = true })
end)

check("a .ofn buffer gets filetype ofn, the server's errors, hover and definition; formatting and rename write nothing in it", function()
  local file = owl_dir .. "/onto.ofn"
  local text = {
    "Prefix(:=<http://example.org/o#>)",
    "Ontology(<http://example.org/o>",
    "Declaration(Class(:A))",
    "SubClassOf(:A nope:B)",
    'AnnotationAssertion(rdfs:label :A "Apple")',
    ")",
  }
  vim.fn.writefile(text, file)
  local b, c = open(file)
  assert(vim.bo[b].filetype == "ofn", "filetype " .. vim.bo[b].filetype)
  local d = wait_diagnostics(b)
  assert(#d == 1, "diagnostics: " .. vim.inspect(d))
  assert(d[1].code == "ofn-syntax" and d[1].lnum == 3 and d[1].col == 14, vim.inspect(d[1]))
  assert(d[1].message == "Undeclared prefix 'nope:'", d[1].message)
  local edits = request(b, "textDocument/formatting", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    options = { tabSize = 2, insertSpaces = true },
  })
  assert(edits == nil or #edits == 0, "formatting edits in a .ofn: " .. vim.inspect(edits))
  local res = vim.lsp.buf_request_sync(b, "textDocument/rename", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, ":A))"),
    newName = "ex:Thing",
  }, TIMEOUT)
  local answer = res and res[c.id]
  local message = answer and answer.err and answer.err.message or vim.inspect(answer)
  assert(message:find("OWL Functional syntax document is not edited", 1, true), "rename in a .ofn: " .. message)
  -- Its axioms are read: the label in hover, the declaration as definition.
  local h = request(b, "textDocument/hover", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "Class(:"),
  })
  assert(h and h.contents.value:find("Apple", 1, true), "hover in a .ofn: " .. vim.inspect(h))
  local def = request(b, "textDocument/definition", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "rdfs:label :"),
  })
  def = def and (def.range and { def } or def) or {}
  assert(#def == 1 and def[1].range.start.line == 2 and def[1].range.start.character == 18, "definition in a .ofn: " .. vim.inspect(def))
  vim.api.nvim_buf_delete(b, { force = true })
end)

check("completing in a .ofn buffer offers the slot's classes and adds a Prefix line when applied", function()
  local file = owl_dir .. "/complete.ofn"
  vim.fn.writefile({
    "Prefix(:=<http://example.org/o#>)",
    "Ontology(<http://example.org/o>",
    "Declaration(Class(:Pump))",
    "Declaration(ObjectProperty(:feeds))",
    "SubClassOf(:Pump skos:Conc)",
    ")",
  }, file)
  local b, c = open(file)
  wait_diagnostics(b)
  local function items(line, character)
    local r = request(b, "textDocument/completion", {
      textDocument = vim.lsp.util.make_text_document_params(b),
      position = { line = line, character = character },
    })
    return r.items or r
  end
  -- A class slot: the class, not the property.
  local labels = {}
  for _, it in ipairs(items(4, #"SubClassOf(:Pump ")) do
    labels[it.label] = true
  end
  assert(labels[":Pump"] and labels["ObjectSomeValuesFrom"] and not labels[":feeds"], vim.inspect(vim.tbl_keys(labels)))
  local item
  for _, it in ipairs(items(4, #"SubClassOf(:Pump skos:Conc")) do
    if it.label == "skos:Concept" then
      item = it
    end
  end
  assert(item, "skos:Concept not offered")
  vim.lsp.util.apply_text_edits({ item.textEdit }, b, c.offset_encoding)
  vim.lsp.util.apply_text_edits(item.additionalTextEdits or {}, b, c.offset_encoding)
  local text = table.concat(vim.api.nvim_buf_get_lines(b, 0, -1, false), "\n")
  assert(text:find("Prefix(:=<http://example.org/o#>)\nPrefix(skos:=<http://www.w3.org/2004/02/skos/core#>)\nOntology(", 1, true), "no Prefix line:\n" .. text)
  assert(text:find("SubClassOf(:Pump skos:Concept)", 1, true), "term not completed:\n" .. text)
  vim.api.nvim_buf_delete(b, { force = true })
end)

check("a .owx buffer gets filetype xml, the server's errors, hover and definition; nothing is written in it", function()
  local file = owl_dir .. "/onto.owx"
  -- No XML declaration: Neovim's own detection names such a text xml.
  vim.fn.writefile({
    '<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">',
    '  <Prefix name="" IRI="http://example.org/o#"/>',
    '  <Prefix name="rdfs" IRI="http://www.w3.org/2000/01/rdf-schema#"/>',
    '  <Declaration><Class abbreviatedIRI=":A"/></Declaration>',
    '  <SubClassOf><Class abbreviatedIRI=":A"/><Class abbreviatedIRI="nope:B"/></SubClassOf>',
    '  <AnnotationAssertion><AnnotationProperty abbreviatedIRI="rdfs:label"/><AbbreviatedIRI>:A</AbbreviatedIRI><Literal>Apple</Literal></AnnotationAssertion>',
    "</Ontology>",
  }, file)
  local b, c = open(file)
  assert(vim.bo[b].filetype == "xml", "filetype " .. vim.bo[b].filetype)
  assert(#vim.lsp.get_clients({ bufnr = b, name = "turtle_lsp" }) == 1, "not one turtle_lsp client")
  local d = wait_diagnostics(b)
  assert(#d == 1, "diagnostics: " .. vim.inspect(d))
  assert(d[1].code == "owx-syntax" and d[1].lnum == 4 and d[1].col == 65, vim.inspect(d[1]))
  local edits = request(b, "textDocument/formatting", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    options = { tabSize = 2, insertSpaces = true },
  })
  assert(edits == nil or #edits == 0, "formatting edits in a .owx: " .. vim.inspect(edits))
  local res = vim.lsp.buf_request_sync(b, "textDocument/rename", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, '<Declaration><Class abbreviatedIRI=":'),
    newName = "ex:Thing",
  }, TIMEOUT)
  local answer = res and res[c.id]
  local message = answer and answer.err and answer.err.message or vim.inspect(answer)
  assert(message:find("OWL/XML document is not edited", 1, true), "rename in a .owx: " .. message)
  -- Its axioms are read: the label in hover, the declaration as definition.
  local h = request(b, "textDocument/hover", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, '<Declaration><Class abbreviatedIRI=":'),
  })
  assert(h and h.contents.value:find("Apple", 1, true), "hover in a .owx: " .. vim.inspect(h))
  local def = request(b, "textDocument/definition", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "<AbbreviatedIRI>:"),
  })
  def = def and (def.range and { def } or def) or {}
  assert(#def == 1 and def[1].range.start.line == 3 and def[1].range.start.character == 38, "definition in a .owx: " .. vim.inspect(def))
  vim.api.nvim_buf_delete(b, { force = true })
end)

check("a .ofn, a .owx and a Functional .owl or .rdf read no modelines: ' ex:' in their last lines raises no E518", function()
  local functional = {
    "Prefix(ex:=<http://example.org/>)",
    "Ontology(<http://example.org/o>",
    "SubClassOf(ex:Foo ex:Bar))",
  }
  local owx = {
    '<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">',
    '  <Prefix name="ex" IRI="http://example.org/"/>',
    "  <!-- ex:Foo is a kind of ex:Bar -->",
    '  <SubClassOf><Class abbreviatedIRI="ex:Foo"/><Class abbreviatedIRI="ex:Bar"/></SubClassOf>',
    "</Ontology>",
  }
  for _, name in ipairs({ "/modeline.ofn", "/modeline.owl", "/modeline.rdf", "/modeline.owx" }) do
    local file = owl_dir .. name
    vim.fn.writefile(name:find("%.owx$") and owx or functional, file)
    local ok, b = pcall(open, file)
    assert(ok, name .. ": " .. tostring(b))
    assert(vim.bo[b].modeline == false, name .. ": modeline on")
    wait_diagnostics(b)
    vim.api.nvim_buf_delete(b, { force = true })
  end
end)

check("a Turtle buffer gets static formatting and rename with prepareRename", function()
  local ttl, tc = open(people)
  assert(tc:supports_method("textDocument/formatting", ttl), "no formatting in people.ttl")
  assert(tc:supports_method("textDocument/rename", ttl), "no rename in people.ttl")
  assert(tc:supports_method("textDocument/prepareRename", ttl), "no prepareRename in people.ttl")
  local rename = tc.server_capabilities.renameProvider
  assert(type(rename) == "table" and rename.prepareProvider == true, "renameProvider " .. vim.inspect(rename))
  for _, method in ipairs({ "textDocument/formatting", "textDocument/rename", "textDocument/completion", "textDocument/codeAction" }) do
    assert(#((tc.registrations or {})[method] or {}) == 0, method .. " registered dynamically")
  end
  local prepared = request(ttl, "textDocument/prepareRename", {
    textDocument = vim.lsp.util.make_text_document_params(ttl),
    position = pos_of(ttl, "ex:bo"),
  })
  assert(prepared, "prepareRename gave nothing on ex:bob")
end)

check("a .owl buffer's client comes back after vim.lsp.enable's FileType handler detaches it", function()
  -- What vim.lsp.enable() runs when a server is enabled after startup, and
  -- what a file type set again runs for one buffer.
  vim.cmd("doautoall nvim.lsp.enable FileType")
  vim.api.nvim_buf_call(owl_buf, function()
    vim.cmd("doautocmd FileType xml")
  end)
  local back = vim.wait(TIMEOUT, function()
    vim.wait(50)
    return #vim.lsp.get_clients({ bufnr = owl_buf, name = "turtle_lsp" }) > 0
  end, 20)
  assert(back, "client not attached after FileType")
end)

check("a client's exit report is dropped only for that client's own exit", function()
  local log = ". Check log for errors: lsp.log"
  local errors = {
    "an earlier error",
    "Client turtle_lsp quit with exit code 1 and signal 0" .. log,
    "Client other quit with exit code 1 and signal 0" .. log,
    "Client turtle_lsp quit with exit code 2 and signal 0" .. log,
    "Client turtle_lsp quit with exit code 1 and signal 0" .. log,
  }
  drop_exit_report(errors, 0, "turtle_lsp", { code = 1, signal = 0 })
  local left = table.concat(errors, " || ")
  assert(#errors == 4 and errors[1] == "an earlier error", left)
  assert(errors[2]:find("^Client other ") and errors[3]:find("exit code 2 "), left)
  assert(errors[4]:find("exit code 1 "), "the second report of the same exit stays: " .. left)
  -- Code 0 and SIGTERM (15) are not reported: nothing is dropped.
  drop_exit_report(errors, 0, "turtle_lsp", { code = 0, signal = 15 })
  drop_exit_report(errors, 0, "turtle_lsp", nil)
  assert(#errors == 4, table.concat(errors, " || "))
  -- Only after `from`.
  local before = { "Client turtle_lsp quit with exit code 1 and signal 0" .. log }
  drop_exit_report(before, 1, "turtle_lsp", { code = 1, signal = 0 })
  assert(#before == 1, "dropped a report from before")
end)

check("a .owl buffer's client that exits or is stopped does not come back", function()
  -- No client attached: gone, after its detach has had time to act.
  local function stays_gone(b, what)
    local gone = vim.wait(TIMEOUT, function()
      return #vim.lsp.get_clients({ bufnr = b, name = "turtle_lsp" }) == 0
    end, 20)
    assert(gone, what .. ": still attached")
    vim.wait(500)
    assert(#vim.lsp.get_clients({ bufnr = b, name = "turtle_lsp" }) == 0, what .. ": attached again")
  end
  local ok, err = pcall(function()
    local c = vim.lsp.get_clients({ bufnr = owl_buf, name = "turtle_lsp" })[1]
    assert(c and c.id ~= client.id, "the .owl buffer shares the workspace client")
    -- The server exits on its own (as on a crash): the client is not restarted.
    -- Neovim reports that exit when its code is not 0, as on Windows; that
    -- one message is this check's own doing, not an error to surface.
    local before = #client_errors
    c.rpc.terminate()
    stays_gone(owl_buf, "exited")
    assert(vim.wait(TIMEOUT, function()
      return exited[c.id] ~= nil
    end, 20), "no exit recorded")
    drop_exit_report(client_errors, before, c.name, exited[c.id])
    -- Read again, the file gets a new client; stopped, it stays stopped.
    vim.api.nvim_buf_call(owl_buf, function()
      vim.cmd.edit()
    end)
    local back = vim.wait(TIMEOUT, function()
      local cs = vim.lsp.get_clients({ bufnr = owl_buf, name = "turtle_lsp" })
      return #cs > 0 and cs[1].initialized
    end, 20)
    assert(back, "no client after :edit")
    vim.lsp.get_clients({ bufnr = owl_buf, name = "turtle_lsp" })[1]:stop()
    stays_gone(owl_buf, "stopped")
  end)
  vim.api.nvim_buf_delete(owl_buf, { force = true })
  vim.fn.delete(owl_dir, "rf")
  assert(ok, err)
end)

check("a .OWL or .Rdf buffer gets the client in any letter case, with nofileignorecase", function()
  local dir = vim.fn.tempname()
  vim.fn.mkdir(dir, "p")
  local was = vim.o.fileignorecase
  vim.o.fileignorecase = false
  local ok, err = pcall(function()
    for _, name in ipairs({ "ONTO.OWL", "Data.Rdf" }) do
      local file = dir .. "/" .. name
      vim.fn.writefile({
        '<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">',
        '  <rdf:Description rdf:about="http://example.org/a"/>',
        "</rdf:RDF>",
      }, file)
      -- Neovim's own detection gives these names no file type then; the
      -- client starts by name.
      local b, c = open(file)
      wait_diagnostics(b)
      c:stop()
      local gone = vim.wait(TIMEOUT, function()
        return c:is_stopped()
      end, 20)
      assert(gone, name .. ": client not stopped")
      vim.api.nvim_buf_delete(b, { force = true })
    end
  end)
  vim.o.fileignorecase = was
  vim.fn.delete(dir, "rf")
  assert(ok, err)
end)

check("a .OWX buffer gets the client with nofileignorecase; an ordinary .xml buffer gets none", function()
  local dir = vim.fn.tempname()
  vim.fn.mkdir(dir, "p")
  local was = vim.o.fileignorecase
  vim.o.fileignorecase = false
  local ok, err = pcall(function()
    local file = dir .. "/ONTO.OWX"
    vim.fn.writefile({
      '<Ontology xmlns="http://www.w3.org/2002/07/owl#" ontologyIRI="http://example.org/o">',
      '  <Declaration><Class IRI="http://example.org/A"/></Declaration>',
      "</Ontology>",
    }, file)
    -- Read as OWL/XML (its name gives the syntax): no errors.
    local b, c = open(file)
    local d = wait_diagnostics(b)
    assert(#d == 0, #d .. " diagnostics: " .. fmt(d))
    local xml = dir .. "/data.xml"
    vim.fn.writefile({ '<Ontology xmlns="http://www.w3.org/2002/07/owl#"/>' }, xml)
    vim.cmd.edit(vim.fn.fnameescape(xml))
    local other = vim.api.nvim_get_current_buf()
    assert(vim.bo[other].filetype == "xml", "filetype " .. vim.bo[other].filetype)
    vim.wait(200)
    assert(#vim.lsp.get_clients({ bufnr = other, name = "turtle_lsp" }) == 0, "a client attached to data.xml")
    vim.api.nvim_buf_delete(other, { force = true })
    c:stop()
    assert(vim.wait(TIMEOUT, function()
      return c:is_stopped()
    end, 20), "client not stopped")
    vim.api.nvim_buf_delete(b, { force = true })
  end)
  vim.o.fileignorecase = was
  vim.fn.delete(dir, "rf")
  assert(ok, err)
end)

check("a .rq buffer (filetype sparql) attaches: no warnings, definition into the ontology", function()
  local b = open(query .. "/people.rq")
  assert(vim.bo[b].filetype == "sparql", "filetype " .. vim.bo[b].filetype)
  local d = problems(wait_diagnostics(b))
  assert(#d == 0, #d .. " problems: " .. fmt(vim.list_slice(d, 1, 5)))
  local r = request(b, "textDocument/definition", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "ex:worksF"),
  })
  local locs = r and (r.uri and { r } or r) or {}
  assert(#locs == 1, #locs .. " locations: " .. vim.inspect(locs))
  local got = vim.uri_to_fname(locs[1].uri or locs[1].targetUri)
  assert(got:match("ontology%.ttl$"), "wrong file " .. got)
end)

check("a .ru buffer is SPARQL, config.ru stays Ruby", function()
  local b = open(query .. "/update.ru")
  assert(vim.bo[b].filetype == "sparql", "filetype " .. vim.bo[b].filetype)
  assert(vim.filetype.match({ filename = "config.ru" }) == "ruby")
end)

check("type hierarchy answers: sd:MeasurementPoint has sosa:Sensor as a supertype", function()
  local b = open(project .. "/core/vocab.ttl")
  local items = request(b, "textDocument/prepareTypeHierarchy", {
    textDocument = vim.lsp.util.make_text_document_params(b),
    position = pos_of(b, "sd:MeasurementPo"),
  })
  assert(items and #items == 1, "prepare: " .. vim.inspect(items))
  local supers = request(b, "typeHierarchy/supertypes", { item = items[1] })
  local names = vim.tbl_map(function(i)
    return i.name
  end, supers or {})
  assert(vim.tbl_contains(names, "sosa:Sensor"), "supertypes: " .. vim.inspect(names))
end)

check("LSP folding works", function()
  local b = open(people)
  local r = request(b, "textDocument/foldingRange", { textDocument = vim.lsp.util.make_text_document_params(b) })
  assert(r and #r > 0, "no folding ranges")
end)

check("vim.lsp.buf.format() lays out a scratch buffer", function()
  local b = open(ws .. "/__nvim_format.ttl")
  vim.bo[b].expandtab = true
  vim.bo[b].shiftwidth = 4
  vim.api.nvim_buf_set_lines(b, 0, -1, false, {
    "@prefix ex: <http://example.org/> .",
    "ex:a a ex:Thing;ex:knows ex:b .",
  })
  wait_diagnostics(b)
  vim.lsp.buf.format({ bufnr = b, async = false, timeout_ms = TIMEOUT })
  local text = table.concat(vim.api.nvim_buf_get_lines(b, 0, -1, false), "\n")
  local want = "@prefix ex: <http://example.org/> .\n\nex:a a ex:Thing ;\n    ex:knows ex:b ."
  assert(text == want, "buffer after format:\n" .. text)
  vim.cmd("bwipeout!")
end)

check("semantic tokens highlight a buffer (foaf:Person is a class)", function()
  local b = open(people)
  local at = pos_of(b, "a foaf:P")
  local types = {}
  local ok = vim.wait(TIMEOUT, function()
    types = {}
    for _, t in ipairs(vim.lsp.semantic_tokens.get_at_pos(b, at.line, at.character) or {}) do
      table.insert(types, t.type)
    end
    return vim.tbl_contains(types, "class")
  end, 20)
  assert(ok, "token types at foaf:Person: " .. vim.inspect(types))
  local r = request(b, "textDocument/semanticTokens/full", { textDocument = vim.lsp.util.make_text_document_params(b) })
  assert(r and #r.data > 0 and #r.data % 5 == 0, "no semantic tokens")
end)

check("inlay hints show the label after an opaque name", function()
  local b = open(ws .. "/__nvim_inlay.ttl")
  vim.api.nvim_buf_set_lines(b, 0, -1, false, {
    "@prefix ex: <http://example.org/> .",
    "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .",
    'ex:P0042 rdfs:label "pump pressure" .',
    "ex:x ex:p ex:P0042 .",
  })
  wait_diagnostics(b)
  vim.lsp.inlay_hint.enable(true, { bufnr = b })
  local hints = {}
  local ok = vim.wait(TIMEOUT, function()
    hints = vim.lsp.inlay_hint.get({ bufnr = b })
    return #hints == 2
  end, 20)
  assert(ok, "inlay hints: " .. vim.inspect(hints))
  local h = hints[2].inlay_hint
  assert(h.label == "pump pressure", vim.inspect(h))
  assert(h.position.line == 3 and h.position.character == #"ex:x ex:p ex:P0042", vim.inspect(h.position))
  vim.lsp.inlay_hint.enable(false, { bufnr = b })
  vim.cmd("bwipeout!")
end)

check("running a reference lens fills the quickfix list", function()
  -- The plugin registers the lens command (vim.lsp.commands["turtle.showReferences"]).
  local b, c = open(people)
  wait_diagnostics(b)
  local lenses = request(b, "textDocument/codeLens", { textDocument = vim.lsp.util.make_text_document_params(b) })
  local bob = pos_of(b, "ex:bob a")
  local lens
  for _, l in ipairs(lenses or {}) do
    if l.range.start.line == bob.line then
      lens = l
    end
  end
  assert(lens and not lens.command, "lenses: " .. vim.inspect(lenses))
  lens = request(b, "codeLens/resolve", lens)
  assert(lens.command and lens.command.title:match("^%d+ references?$"), vim.inspect(lens))
  vim.fn.setqflist({}, "r", { items = {} })
  c:exec_cmd(lens.command, { bufnr = b })
  local items = vim.fn.getqflist()
  assert(#items == #lens.command.arguments[3] and #items > 0, "quickfix: " .. vim.inspect(items))
  assert(vim.fn.getqflist({ winid = 0 }).winid ~= 0, "quickfix window not open")
  vim.cmd("cclose")
end)

check("a file in data/ and one in shapes/ share one client; the nested prefixes.ttl is not a root", function()
  local nested = vim.fs.normalize(fixtures .. "/nested-prefixes")
  local a, ca = open(nested .. "/data/people.ttl")
  local _, cb = open(nested .. "/shapes/person-shape.ttl")
  assert(ca.id == cb.id, "two clients: " .. ca.id .. " and " .. cb.id)
  assert(vim.fs.normalize(ca.root_dir) == nested, "root " .. tostring(ca.root_dir))
  local same = vim.tbl_filter(function(c)
    return c.root_dir and vim.fs.normalize(c.root_dir) == nested
  end, vim.lsp.get_clients({ name = "turtle_lsp" }))
  assert(#same == 1, #same .. " clients for the project")
  -- The client sees the shapes from the data file: alice has no ex:name.
  local seen = {}
  local ok = vim.wait(TIMEOUT, function()
    seen = vim.tbl_filter(function(d)
      return d.code == "shacl-violation"
    end, vim.diagnostic.get(a))
    return #seen > 0
  end, 20)
  assert(ok, "no SHACL violation in data/people.ttl: " .. fmt(vim.diagnostic.get(a)))
end)

check("the shipped root markers: turtle-lsp.json outranks .git, which outranks a nested prefixes.ttl", function()
  local markers = vim.lsp.config["turtle_lsp"].root_markers
  -- Forward slashes, as vim.fs.root answers (tempname has backslashes on Windows).
  local base = vim.fs.normalize(vim.fn.tempname())
  local function touch(rel)
    vim.fn.mkdir(vim.fs.dirname(base .. "/" .. rel), "p")
    vim.fn.writefile({}, base .. "/" .. rel)
  end
  local ok, err = pcall(function()
    -- A repository with a prefixes.ttl in a subfolder: the repository is the root.
    vim.fn.mkdir(base .. "/repo/.git", "p")
    touch("repo/data/prefixes.ttl")
    touch("repo/data/a.ttl")
    touch("repo/shapes/s.ttl")
    assert(vim.fs.root(base .. "/repo/data/a.ttl", markers) == base .. "/repo", "data/ file")
    assert(vim.fs.root(base .. "/repo/shapes/s.ttl", markers) == base .. "/repo", "shapes/ file")
    -- turtle-lsp.json marks a project inside a repository.
    touch("repo/proj/turtle-lsp.json")
    touch("repo/proj/data/prefixes.ttl")
    touch("repo/proj/data/b.ttl")
    assert(vim.fs.root(base .. "/repo/proj/data/b.ttl", markers) == base .. "/repo/proj", "turtle-lsp.json")
  end)
  vim.fn.delete(base, "rf")
  assert(ok, err)
end)

check(".turtle files are Turtle", function()
  assert(vim.filetype.match({ filename = "x.turtle" }) == "turtle")
end)

check("no LSP errors surfaced to the user", function()
  assert(#client_errors == 0, table.concat(client_errors, " || "))
end)

check("server still alive at end", function()
  assert(#exits == 0 or not client:is_stopped(), "client exited")
end)

vim.cmd((failed == 0) and "qall!" or "cquit! 1")
