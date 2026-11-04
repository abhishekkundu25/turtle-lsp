# 06: Close-out

Status: todo. Blocked by: 04, 05.

## What to build

- `editors/vscode/README.md` and `README.md`: "In the browser" — what works
  (the feature list) and what does not (Run query, imports, the file cap,
  folders added later), and how settings and `turtle-lsp.json` apply.
- `CHANGELOG.md` under `## Unreleased`: the web extension.
- `ARCHITECTURE.md`: the browser entry, the worker, the layers.
- The spec's status line: done.
- Delete `docs/tickets/web-extension/` (the repository keeps specs, not tickets).

## Acceptance

- [ ] Full validation, run with nothing else running: every Rust, Node, browser and VS Code suite (desktop and web), lint and format, `cargo test --doc`.
- [ ] The budgets in the spec measured and recorded in the spec with what bounds them: the 201-file workspace in headless Chromium, the web `.vsix` size, the engine budget unchanged.
