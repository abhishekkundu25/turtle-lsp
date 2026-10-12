<p align="center"><img src="https://raw.githubusercontent.com/abhishekkundu25/turtle-lsp/main/assets/logo.png" alt="turtle-lsp" width="640"></p>

# Turtle (RDF) for VS Code

Language support for RDF Turtle (`.ttl`) powered by
[turtle-lsp](https://github.com/abhishekkundu25/turtle-lsp), a language server
written in Rust: a keystroke in a 30,000-line file gets fresh diagnostics in
about 9 ms.

- Syntax highlighting, including Turtle 1.2 (`<<( )>>`, `~`, `{| |}`).
- Diagnostics: syntax errors with recovery, undeclared/unused prefixes, unknown or deprecated terms of 80+ well-known vocabularies, with quick fixes.
- Completion of prefixes and terms (adds the `@prefix` line for you), hover docs, go to definition, type definition, implementation, references, rename, outline, workspace symbols, folding.

Install from the Marketplace (`abhishekkundu25.turtle-language-server`), or
run `code --install-extension abhishekkundu25.turtle-language-server`.

## Project prefixes

If your files share a prefix header, put it in a `prefixes.ttl`: every Turtle
file in that folder or below can use those prefixes. Or set `turtle.prefixes`.

## Settings

- `turtle.server.path`: use a different `turtle-lsp` binary.
- `turtle.prefixes`: prefixes available in every file.
- `turtle.trace.server`: log LSP traffic to the Output panel.
