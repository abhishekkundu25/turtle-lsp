<p align="center"><img src="assets/logo.png" alt="turtle-lsp" width="640"></p>

# turtle-lsp

A fast language server for RDF Turtle (`.ttl`), written in Rust.

A keystroke in a 30,000-line file gets fresh diagnostics in about 9 ms. The
server starts in about 1 ms.

## Features

- **Diagnostics**: syntax errors with recovery (one mistake does not hide the rest of the file), undeclared/unused/duplicate prefixes, unknown or deprecated terms of well-known vocabularies (matched by namespace IRI, with "did you mean"), invalid IRIs and language tags.
- **Completion**: after `prefix:`, terms of that namespace from 80+ bundled vocabularies and from your workspace, ranked by position (classes after `a`, properties in predicate position). Typing a bare local name (`Pers`) finds `foaf:Person`. Completing a well-known prefix that is not declared adds its `@prefix` line.
- **Hover**: label, comment, types and where a term is described.
- **Navigation**: go to definition (where a resource is the subject), type definition (the resource's class), implementation (a class's instances and subclasses), references, rename across files, document highlight. On the prefix part of `ex:a`, these work on the prefix.
- **Outline and search**: document symbols, workspace symbols, folding.
- **Quick fixes**: add a missing prefix, add all missing prefixes, remove an unused prefix, fix a misspelled term.
- **Turtle 1.2**: triple terms `<<( )>>`, reified triples `<< >>`, `~` reifiers, `{| |}` annotations, `@lang--dir`, `VERSION`.

## Performance

Benchmark: one 30,005-line file (5,000 resources) open in a workspace of 200
other Turtle files (40,000 more resources). The client is a script that talks
to the server over stdio, the way an editor does. Times are end to end,
including JSON encoding and the pipe.

| | Previous Node.js server | turtle-lsp (Rust) | Speedup |
|---|---:|---:|---:|
| Keystroke → diagnostics, median | 4,911 ms | **8.5 ms** | ~580× |
| Keystroke → diagnostics, p95 | 8,341 ms | **8.9 ms** | ~940× |
| Start-up (`initialize`) | 2,537 ms | **1.3 ms** | ~1,950× |
| Open file → first diagnostics | 1,884 ms | **21 ms** | ~90× |
| Completion after `foaf:` | 21 ms | **0.4 ms** | ~50× |
| Completion with nothing typed | 476 ms, 48,203 items, 9.5 MB | **0.1 ms**, 6 items, 1 KB | |
| Go to definition | 0.1 ms, 7 locations (every use) | **0.1 ms**, 1 location (the definition) | |
| Memory (resident) | 959 MB | **230 MB** | ~4× less |

Measured on an Apple M6 with macOS 27. Results vary a little between runs.
Reproduce with `npm run bench`.

Why it is fast:

- **One parse per edit.** Each document version is parsed once, by a
  hand-written lexer and parser, and every feature reads that result. The
  previous server parsed each document about seven times per keystroke.
- **Batched edits.** The server reads every queued message before it computes
  diagnostics, so a burst of keystrokes costs one analysis.
- **Small replies.** Completion returns at most 200 ranked items and loads
  their documentation only when you select one.
- **Background indexing.** Workspace files are parsed in parallel on a
  separate thread, so start-up does not wait for them.
- **Embedded vocabularies.** 84 vocabularies (about 21,000 terms) are compiled
  into the binary and parsed only when first used.

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

## VS Code

The extension in [`editors/vscode`](editors/vscode) bundles the server and adds
syntax highlighting. Build and install it:

```sh
cd editors/vscode
npm install
npm run package   # builds the server and writes turtle-lsp-<platform>-<version>.vsix
code --install-extension turtle-lsp-*.vsix
```

Settings: `turtle.server.path` (use another binary), `turtle.prefixes`
(prefixes for every file), `turtle.trace.server` (log LSP traffic).

## Development

```sh
npm install     # test tooling only (LSP client, vocabulary generator)
npm test        # cargo tests, parser cross-check against oxttl, end-to-end LSP tests, Neovim checks
npm run bench   # latency benchmark on a 30k-line file in a 200-file workspace
npm run vocab   # regenerate data/vocab.tsv from @zazuko/rdf-vocabularies
npm run test:vscode  # VS Code extension: grammar tests, then checks inside a downloaded VS Code
```

- `test/lsp.test.mjs`: behaviour over stdio, the way an editor talks to the server.
- `test/project.test.mjs`: a project of headerless fragments sharing one `prefixes.ttl` (`test/fixtures/project`).
- `test/real.test.mjs`: optional private fragments in `test/fixtures/real` (gitignored, skipped when absent).
- `test/nvim/spec.lua`: runs inside headless Neovim with its built-in LSP client.
- `editors/vscode/test/suite.cjs`: runs inside VS Code with the extension loaded; `editors/vscode/test/grammar.test.mjs` checks the highlighting grammar.
- `tests/oxttl_oracle.rs`: on every valid fixture, the parser must produce the same triples as [oxttl](https://crates.io/crates/oxttl).

Vocabulary data comes from [@zazuko/rdf-vocabularies](https://github.com/zazuko/rdf-vocabularies); each vocabulary keeps its own license.
