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

In scope:

- the `turtle-lsp` binary: the language server and the `check`, `format` and
  `query` commands;
- the VS Code extension in `editors/vscode`;
- the Neovim plugin: `lsp/turtle_lsp.lua`, `plugin/turtle_lsp.lua` and
  `build.lua`, which downloads the server;
- the browser editor and toolbox (`web/app`, with the engine in
  `web/engine`), published on GitHub Pages;
- this repository's release pipeline.

Examples: a file that makes the server run code, read files outside the
workspace, or send a request you did not ask for; a `turtle-lsp.json` or an
extension setting that a cloned repository can abuse; a way to tamper with
released `.vsix` files, archives or the downloads of `build.lua`; a file
that makes a page of the browser editor run script.

## What the project does to stay safe

### Network

The server and `check` make web requests in two cases only
(`src/imports/mod.rs`, `src/jsonld/loader.rs`):

- `imports.network` is on (an editor setting, or `--network` for `check`).
  Then `owl:imports` and JSON-LD `@context` URLs that the workspace, a
  catalog, the import cache and the bundled contexts cannot answer are
  fetched. It is off by default.
- You run the quick fix "Fetch `<IRI>` and cache it" on one unresolved
  import. It fetches that import, with the setting on or off.

Every request goes through `src/fetch.rs`:

- Only `http` and `https`. The server follows redirects itself, at most 10
  per document, and checks each hop. A redirect from `https` to `http` is
  refused.
- Loopback, private, link-local, cloud metadata, carrier-grade NAT,
  multicast and unspecified addresses are refused. A host name is checked
  after DNS answers, on the addresses the connection then uses, so a name
  that resolves to an internal address is refused too. Setting
  `TURTLE_LSP_ALLOW_PRIVATE_HOSTS=1` in the server's environment allows
  them.
- One document may take 20 MB and 10 s, its redirects included. One pass
  over the imports may make 100 requests and read 100 MB.

The browser build has no network client (`src/fetch.rs` is not compiled for
it). Its "Fetch" quick fix asks the browser to download one import, at most
10 MB, without cookies or a referrer (`web/app/editor.js`).

### Files it reads and writes

The server reads:

- the documents the editor opens;
- under the workspace folders, the RDF files it indexes (`.ttl`, `.turtle`,
  `.trig`, `.nt`, `.nq`, `.jsonld`), `prefixes.ttl`, `catalog-v001.xml`
  files and `turtle-lsp.json`; and SPARQL files (`.rq`, `.sparql`, `.ru`)
  when a rename or a reference search needs them. A scan skips `.git`,
  `node_modules`, `target`, `dist`, `build`, `.venv`, `venv` and
  `__pycache__`, takes at most 20,000 files, and skips files over 50 MB
  (`src/workspace.rs`);
- `file:` imports, the files that catalog entries name, and `file:` JSON-LD
  contexts, only inside the workspace folders. The path is compared with the
  folders before any file is touched, and a missing file and a file outside
  get the same error. A `file:` URL that names a host other than
  `localhost` is refused, and so is a Windows network share unless the
  workspace is on one (`src/imports/load.rs`);
- the per-user cache: `$TURTLE_LSP_CACHE_DIR`, else
  `~/Library/Caches/turtle-lsp` (macOS), `%LOCALAPPDATA%\turtle-lsp`
  (Windows), or `$XDG_CACHE_HOME/turtle-lsp` (`~/.cache/turtle-lsp`)
  (`src/imports/mod.rs`).

The server writes only into that cache: fetched documents under `imports/`
and query results under `query-results/`. A cached document is named by the
SHA-256 of its URL and records the URL (in its first line, or in a `.url`
file beside a cached JSON-LD document), which is checked when the file is
read, so a file that another URL produced, or that someone planted, is not
taken for it. Files are written to a temporary name of their own and renamed.
Changes to workspace files go to the editor as edits; the server does not
write them.

`turtle-lsp format` and `turtle-lsp check --fix` write the files they are
given, through a temporary file in the same folder, and only if the file has
not changed since it was read (`src/cli/files.rs`).

### Settings a repository ships

`turtle-lsp.json` comes with the repository, so it is not trusted
(`src/settings.rs`). It cannot turn on `imports.network`, and it cannot
raise `query.timeoutSeconds`, `query.maxRows`, `query.maxBufferedRows` or
`shacl.maxTriples` past their defaults. The server and the command line
ignore such a key with a warning that names it. A file larger than 1 MB is
not read.

### Editors

- **VS Code**: `turtle.server.path` is a machine setting, and it must be an
  absolute path to a file that exists. In an untrusted workspace,
  `turtle.server.path`, `turtle.imports.network`, `turtle.query.*`,
  `turtle.shacl.maxTriples` and `turtle.diagnostics.workspace` take your
  user settings only, network imports are off, and no server binary inside
  the workspace is run. A relative or empty `PATH` entry is never searched
  for the server (`editors/vscode/src/server.ts`).
- **Neovim**: `build.lua` downloads the server from the GitHub release of
  the plugin's version over `https` only, redirects included, with TLS 1.2
  or newer. It checks the archive against the release's `SHA256SUMS` and,
  when the `gh` command is installed, with `gh attestation verify`. A failed
  check stops the install, and a `gh` that is not signed in fails the
  check. With `TURTLE_LSP_RELEASE_URL` set, the download is checked against
  that location's `SHA256SUMS` only, and `build.lua` prints a warning.
- **Browser**: both pages set a Content Security Policy that loads scripts
  only from the site itself (`web/app/index.html`, `web/app/toolbox.html`).
  Files stay in the tab and in the browser's IndexedDB; nothing is uploaded.

### Isolation

The language server runs its main loop on a thread with a 64 MiB stack
(`src/main.rs`). Each message, each answer from a worker thread, each timer
and each phase after a batch of messages runs under a guard: a panic is
reported on stderr and the session goes on. A document whose analysis panics
gets an `internal-error` diagnostic. Background threads start through one
guarded helper, `spawn_guarded` (`src/server.rs`, `src/server/worker.rs`).

A stack overflow cannot be caught, so the inputs that recurse are bounded
before they reach recursive code: Turtle nesting (256 levels, annotation
blocks included), SPARQL nesting (256 levels), operator chains (2,000),
property paths (2,000 steps), runs of unary operators (3,000), SHACL shape
and path depth (64), and JSON-LD remote contexts (32 levels and a work
budget). Work is bounded by time too: a query stops after
`query.timeoutSeconds` (default 30 s), a query that joins more than 200
patterns at once is refused, and a SHACL-SPARQL query stops after 10 s.

### Tests

- `tests/chaos.rs` mutates real Turtle, TriG, N-Triples and N-Quads files
  (SHACL shapes with SPARQL among them), JSON-LD documents and SPARQL
  queries and updates, and runs the parser, every editor feature, SHACL
  validation, catalog parsing and RDF/XML conversion on each result. Every
  CI run does 30,000 iterations of each family; any panic fails it. It does
  not reach the LSP transport or the query engine.
- `test/protocol-errors.test.mjs` sends the server unknown methods, params
  of the wrong shape and requests before `initialize`, which get an error
  while the session goes on, and malformed frames, invalid JSON and bodies
  that are not UTF-8, which end the session without a panic or a hang.
- `test/resources.test.mjs` checks that an idle server uses no CPU and that
  no server outlives its session.

### Releases and dependencies

- Releases are built by GitHub Actions from a tagged commit, and published
  only after CI passed on that commit and the server inside each `.vsix` ran
  on its own platform. Each release includes `SHA256SUMS` and build
  provenance attestations:
  `gh attestation verify <file> --repo abhishekkundu25/turtle-lsp`.
- The Marketplace token `VSCE_PAT` lives only in the GitHub environment
  `release`. The job that uses it installs packages without install
  scripts and cannot write to the repository.
- On every push to `main` and every pull request, CI runs `cargo deny` on
  the server and on the browser engine (`web/engine`), `npm audit` on the
  extension's runtime dependencies and on the root package tree (which
  ships Monaco to the site), and `tools/check-vendor.sh`, which checks that
  `vendor/spareval` is spareval 0.2.7 plus its patch and nothing else.
- Dependabot proposes updates weekly for both Cargo lock files, both npm
  trees and the GitHub Actions. Actions are pinned to full commit SHAs.
