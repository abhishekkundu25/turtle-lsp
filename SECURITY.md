# Security policy

## Reporting a vulnerability

Please report security issues privately, not in public issues:
use **[Report a vulnerability](https://github.com/abhishekkundu25/turtle-lsp/security/advisories/new)**
on GitHub (Security tab → Advisories). Include the version, your editor, and a
Turtle file or steps that reproduce the problem.

You should get a reply within 7 days. Fixes ship as a new release on GitHub and
the VS Code Marketplace, and the advisory is published once users can update.

## Supported versions

Only the latest release receives fixes.

## Scope

In scope: the `turtle-lsp` server, the VS Code extension in `editors/vscode`,
and this repository's release pipeline. Examples: a file that makes the server
run code, read files outside the workspace, or send anything over the network;
an extension setting a workspace can abuse; a way to tamper with released
`.vsix` files.

## What the project does to stay safe

- The server makes no network requests and only reads Turtle files inside the
  workspace folders the editor sends.
- In VS Code, `turtle.server.path` is a machine setting and is ignored in
  untrusted workspaces, so a repository cannot choose which binary runs.
- Each request is isolated: an internal error is reported to the editor
  instead of stopping the server. A randomized test feeds the parser and every
  feature thousands of malformed files on every CI run.
- Releases are built by GitHub Actions from a tagged commit. Each release
  includes `SHA256SUMS` and a build provenance attestation:
  `gh attestation verify <file>.vsix --repo abhishekkundu25/turtle-lsp`.
- Dependencies are checked on every push (`cargo deny`, `npm audit`) and
  updated weekly by Dependabot. GitHub Actions are pinned to commit SHAs.
