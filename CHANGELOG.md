# Changelog

## 0.1.1 — 2026-10-13

### Security

- **`turtle.server.path` can no longer be set by a workspace.** The setting is
  now machine-scoped and disabled in untrusted workspaces, so a cloned repository
  can no longer make VS Code launch an arbitrary program through its
  `.vscode/settings.json`. Upgrading is recommended.

### Fixes

- Requests that arrive while the workspace is still being indexed now wait for
  the index. Before, go-to-definition, references and rename right after opening
  a project could miss other files.
- Fixed a crash when a client sends a position past the end of a line or file.
  It was found by the new fuzz (chaos) test.
- A failure inside one request or diagnostics run is reported as an error and no
  longer takes the server down.
- On Windows, the different spellings of the same file URI (`c%3A` vs `C:`) are
  treated as the same file.
- Large files (over 50 MB) are skipped during workspace indexing, and indexing is
  capped at 20,000 files.

### Extension

- The extension is now listed as **Turtle RDF Language Server (TTL, OWL, SHACL)**,
  with a fuller description and keywords, so it is easier to find in search.

### Release

- Each GitHub Release now includes `SHA256SUMS` and signed build provenance for
  every `.vsix`.

## 0.1.0 — 2026-10-13

First release of the Rust server, a from-scratch rewrite of the earlier
Node/TypeScript server.

- Turtle 1.2 parser that recovers from errors, checked against `oxttl`, with RDF
  1.2 triple terms, reifiers and annotations.
- Diagnostics: syntax errors, undeclared, duplicate and unused prefixes, unknown
  and deprecated terms in 84 well-known vocabularies (matched by namespace IRI),
  with quick fixes.
- Project prefixes from `prefixes.ttl`, so files without their own prefix block
  work.
- Completion (in context, auto-imports prefixes), hover, go-to-definition, type
  definition, implementation, references, highlights, rename, document and
  workspace symbols, folding.
- About 9 ms from keystroke to diagnostics on a 30,000-line file, compared with
  4.9 s for the old server.
- VS Code extension for macOS, Linux, Alpine and Windows; Neovim setup in the
  README.
