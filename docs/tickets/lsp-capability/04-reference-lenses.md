# 04: Reference lenses

**What to build:** `textDocument/codeLens` with one lens per subject a document describes; `codeLens/resolve` fills the title ("1 reference" / "N references") from the existing reference search and the command `turtle.showReferences` (uri, position, locations). The command is client-side: not advertised by the server. VS Code maps it to `editor.action.showReferences`; the Neovim plugin registers it to fill the quickfix list. Setting `codeLens.references` (default true). Turtle, TriG, N-Triples, N-Quads, JSON-LD; no SPARQL. No lenses above a size threshold (follow the existing large-file guard style).

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Node LSP tests: lens ranges, titles after resolve, command and arguments, setting off, none for SPARQL, JSON-LD lenses.
- [ ] VS Code suite: the command opens the references view (editors/vscode/test/suite.cjs).
- [ ] Neovim spec (test/nvim/spec.lua): running the lens fills the quickfix list.
- [ ] Keystroke rows of bench/perf.mjs unchanged; a codeLens request on the 30k-line file is cheap (report its time).
