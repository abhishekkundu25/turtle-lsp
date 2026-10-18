# Changelog

## 0.4.2 — 2026-10-19

### Fixes

- Renaming a shared prefix back to a name that some other file declares for
  itself is no longer refused when that file does not use the prefix being
  renamed (for example renaming `ex:` to `eg:` and then back to `ex:`).

### Extension

- The Marketplace page now lists SHACL validation, `owl:imports`,
  formatting, refactorings and TriG/N-Triples support.

## 0.4.1 — 2026-10-19

### Fixes

- **Renaming a shared prefix**: renaming a prefix that a `prefixes.ttl`
  declares, from that file or from a file that uses it, now renames it in
  every file that takes it from there (open or not), and not in files that
  declare it themselves. Before, only the current file changed, which left
  the other files with an undeclared prefix. A rename to a name the affected
  files already declare or use is refused, and a prefix from the
  `turtle.prefixes` setting is refused with a pointer to the setting.

## 0.4.0 — 2026-10-19

### Features

- **Fetch one import on request**: an unresolved web `owl:imports` has a
  quick fix, "Fetch <IRI> and cache it", that fetches that one import and
  caches it, with `turtle.imports.network` off (which stays the default).
  Asking again after a failed fetch retries at once.
- **Cached imports open as files**: go to definition on an imported term
  opens the cached copy, and hover names the URL it came from. The cached copy
  is read-only: it gets no diagnostics, and its terms are not counted twice.

## 0.3.0 — 2026-10-18

### Features

- **SHACL-SPARQL**: `sh:sparql` constraints (`sh:SPARQLConstraint` with
  `sh:select`) on node and property shapes, with pre-bound `$this`, `$PATH`,
  `$currentShape` and `$shapesGraph`, `sh:prefixes` / `sh:declare`, message
  templates (`{?value}`, `{$this}`) and `sh:deactivated`; SPARQL-based targets
  and SPARQL-based constraint components. W3C SHACL-SPARQL suite: 23/23.
- **SPARQL queries in shapes are checked as you type**: syntax errors and
  undeclared prefixes (errors, on the exact spot in the query), and the
  constructs SHACL-SPARQL forbids (MINUS, VALUES, SERVICE, LATERAL, rebinding
  a pre-bound variable) as warnings.
- **SHACL validation while typing**: the workspace is validated about a
  second after typing stops, on the unsaved text; saving still validates at
  once. Setting `turtle.shacl.onType` (default on) turns it off.
- **Progress** for indexing, `owl:imports` resolution and SHACL runs that
  last more than 200 ms, for clients that support work done progress. A SHACL
  run ends with a summary such as "2 errors, 1 warning in 2 files".
- The undeclared-prefix quick fix can also **add the `owl:imports`** of the
  workspace ontology that defines the namespace.

### Fixes

- Unknown-term warnings no longer appear on terms the workspace or a resolved
  import describes, and files with thousands of them stay fast (a 2.3 MB file
  went from 2.3 s per edit to about 60 ms).
- SHACL errors that unresolved `owl:imports` could change are shown as
  warnings that name the missing imports.
- The formatter's safety check keeps the document's base IRI, so documents
  with relative IRIs are not refused.

### Testing

- `npm run corpus` checks the parser against oxttl and the formatter's
  properties on public RDF files (68 cases, 2.17 million triples), and
  measures the server on each.

## 0.2.0 — 2026-10-18

### Features

- **Formatting** for Turtle, TriG and N-Triples: idempotent, keeps comments
  and statement order, never changes the graph, refuses files with syntax
  errors.
- **Semantic tokens** (classes, properties, individuals, datatypes, prefixes;
  declaration, deprecated, library and undescribed modifiers) and **label
  inlay hints** for opaque identifiers.
- **Refactorings**: organise prefixes, shorten or expand an IRI, extract a
  prefix for a namespace.
- **Ontology-aware completion**: properties required by SHACL shapes or
  fitting `rdfs:domain` first; instances fitting the range or `sh:class`
  first; `sh:in` values offered.
- **Type hierarchy** for classes and properties.
- **Lints**: undescribed terms in your own namespaces (with "did you mean"),
  and SHACL shape mistakes as you type.
- **SHACL validation on save** with an in-house SHACL Core engine (W3C SHACL
  Core suite: 98/98).
- **`owl:imports`** from the workspace, a `catalog-v001.xml`, a cache, or the
  network when `turtle.imports.network` is on (Turtle, N-Triples, RDF/XML).
- **TriG and N-Triples** documents.
- **Pull diagnostics** for clients that support refresh, request
  cancellation, and live settings (`workspace/didChangeConfiguration`).
- The add-prefix quick fix offers the project's own binding first.

### Fixes

- The server now exits when the editor goes away, after `shutdown`/`exit`,
  and with code 1 on a bare `exit`. Before, every session left a process
  behind, and once the index was built an idle server could use a full CPU
  core.
- W3C Turtle, TriG and N-Triples suites (982/982): stricter string and IRI
  escapes, long-string endings, triple-term contents, language tags (invalid
  tags are now errors).
- Diagnostics are published under the URI the editor opened the file with.
  Before, a path the editor spelled differently from the server (`%7E` for
  `~`, as in Windows short names like `RUNNER~1`) got no diagnostics.

### Security

- `rustls` 0.23.46 (RUSTSEC-2026-0346 to 0349), used only when network
  imports are on. RDF/XML imports cap entity expansion, so a small file can
  no longer exhaust memory.

### Performance

- Memory on the benchmark workspace went from 230 MB to about 140 MB
  (interned IRIs), and editing a 10 MB TriG file went from 1.2 GB to under
  600 MB (mimalloc; no second copy of open files). Keystroke → diagnostics
  median: 8.5 → 4.3 ms.

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
