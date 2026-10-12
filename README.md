# turtle-lsp

A fast language server for RDF Turtle (`.ttl`), written in Rust.

## Features

- **Diagnostics**: syntax errors with recovery (one mistake does not hide the rest of the file), undeclared/unused/duplicate prefixes, unknown or deprecated terms of well-known vocabularies (matched by namespace IRI, with "did you mean"), invalid IRIs and language tags.
- **Completion**: after `prefix:`, terms of that namespace from 80+ bundled vocabularies and from your workspace, ranked by position (classes after `a`, properties in predicate position). Completing a well-known prefix that is not declared adds its `@prefix` line.
- **Hover**: label, comment, types and where a term is described.
- **Navigation**: go to definition (where a resource is the subject), references, rename across files, document highlight. On the prefix part of `ex:a`, these work on the prefix.
- **Outline and search**: document symbols, workspace symbols, folding.
- **Quick fixes**: add a missing prefix, add all missing prefixes, remove an unused prefix, fix a misspelled term.
- **Turtle 1.2**: triple terms `<<( )>>`, reified triples `<< >>`, `~` reifiers, `{| |}` annotations, `@lang--dir`, `VERSION`.

## Project prefixes

Turtle files often leave out their `@prefix` lines and rely on a shared header.
Put the header in a `prefixes.ttl` file: every Turtle file in that directory or
below it can use those prefixes. You can also pass prefixes from the editor:

```lua
init_options = { prefixes = { ex = "http://example.org/" } }
```

## Install

```sh
cargo build --release   # binary: target/release/turtle-lsp
```

## Neovim (0.11+)

```lua
vim.filetype.add({ extension = { ttl = "turtle" } })

vim.lsp.config("turtle", {
  cmd = { "/path/to/turtle-lsp/target/release/turtle-lsp" },
  filetypes = { "turtle" },
  root_markers = { "prefixes.ttl", ".git" },
})
vim.lsp.enable("turtle")

-- "@prefix ex: <...>" looks like a Vim modeline (" ex:"), which raises
-- E518 on open. Turn modelines off for Turtle files.
vim.api.nvim_create_autocmd("BufReadPre", {
  pattern = { "*.ttl", "*.turtle" },
  callback = function() vim.opt_local.modeline = false end,
})
```

With lazy.nvim, set `build = "cargo build --release"` on the plugin spec.

## Development

```sh
npm install     # test tooling only (LSP client, vocabulary generator)
npm test        # cargo tests, parser cross-check against oxttl, end-to-end LSP tests, Neovim checks
npm run bench   # latency benchmark on a 30k-line file in a 200-file workspace
npm run vocab   # regenerate data/vocab.tsv from @zazuko/rdf-vocabularies
```

- `test/lsp.test.mjs`: behaviour over stdio, the way an editor talks to the server.
- `test/real.test.mjs`: real ontology fragments (`test/fixtures/real`).
- `test/nvim/spec.lua`: runs inside headless Neovim with its built-in LSP client.
- `tests/oxttl_oracle.rs`: on every valid fixture, the parser must produce the same triples as [oxttl](https://crates.io/crates/oxttl).

Vocabulary data comes from [@zazuko/rdf-vocabularies](https://github.com/zazuko/rdf-vocabularies); each vocabulary keeps its own license.
