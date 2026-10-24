# Changelog

## Unreleased

### Fixed

- **The browser editor drew no squiggles in a read-only tab.** A second
  tab on the same workspace listed its errors in Problems but showed
  nothing in the text. Typing in such a tab now says why it is refused, in
  the same notice the tab shows when it opens, instead of Monaco's "Cannot
  edit in read-only editor".
- **A JSON-LD context in the cache could be trusted for a moment without
  being the fetched one.** Saving a fetched context wrote the `.url` file
  that vouches for it before the document, so a check in between read the
  old document as the new one. The document is now written first. Found by
  a test on CI.
- **A SPARQL syntax error after a dot in a prefixed name could crash the
  server** when a multi-byte character followed the dot (`ex:a.😀`): the
  error's position was mapped one byte too far. Found by the nightly chaos
  run.
- **"Shorten all IRIs in file" joined two IRIs written back to back**
  (`<ns#><…#p>` became the one name `ex:sh:p`), which changed the graph.
  Found by the nightly chaos run.
- **A new `@prefix` could land mid-statement or inside a string.** Quick
  fixes, Extract prefix and completion's auto-import put it after the first
  `\n` following the last declaration, found by text search. It now goes in
  at a statement boundary found from the tokens, and files with `\r` line
  endings get it at a real line break. When data shares the last
  declaration's line, the new line now goes right after the declaration,
  not below the data. SPARQL "Add prefix" goes below a leading comment block.
- **Edits no longer fuse or split tokens.** Shorten, Expand, Extract,
  rename, "Change to" and "Remove unused prefix" could turn `ex:a.b.<iri>`,
  `1.<iri>`, `?p?<iri>` or `@prefix:<…>` into different tokens and change
  the data; `check --fix`'s prefix removal could join two statements. Every
  such edit is now checked against the tokens it should leave, and in
  broken SPARQL where no spacing can keep them, the fix is not offered.
  Edits now replace only the characters that change, so their ranges in
  `--format json` and SARIF are smaller.
- **JSON-LD renames write only inside strings**, so an IRI with `//` no
  longer becomes a comment, and adding a prefix to `@context` no longer
  lands inside an unclosed string or after a `//` comment. A bare word
  such as `false` before a key is no longer read as a term.
- **Format Document reaches its final form in one pass.** A `;` that
  nothing follows, with a comment on it (`<o> ; # c` then `.`), is now
  dropped at once. `format --check` fails on files written the old way
  until they are formatted once.
- **Deeply nested SPARQL no longer hangs the server or `check`.**
  `REGEX`, `SUBSTR`, `REPLACE`, the aggregates and `!` nested about twenty
  deep took seconds, two hundred deep never returned, including inside a
  shape's `sh:select`. The vendored SPARQL parser now parses them in linear
  time, and a work bound turns any future blow-up into a "too complex"
  error. `COUNT(DISTINCT:a)` with only `:` declared now reads `DISTINCT:a`
  as a prefixed name.
- **The "did you mean" hint was quadratic in name length.** Two long local
  names sharing a long run took a second per check; it is now linear.
- **`check --fix` re-lexed the whole file for every fix**: 8,000 unused
  prefixes took 30 s, now 1 s.
- **A SHACL shape with a very wide SPARQL query hung `check` and the
  server.** A query joining hundreds of patterns (a run of `~` reifiers)
  spent minutes in query planning; SHACL now refuses it as too complex, as
  `turtle-lsp query` already did.
- **"Remove unused prefix" deleted a prefix used in a statement with a
  syntax error**, which `check --fix` applied without asking. Uses inside
  such statements now count.
- **Renaming a prefix rewrote uses written above its declaration**, which
  that declaration does not cover, so they changed meaning.
- **Extract prefix is no longer offered for an IRI that is not valid as
  written** (a space in it); the new `@prefix` line commented out the rest
  of its line.
- **Positions in files with `\r` line endings were wrong.** A lone `\r` now
  ends a line, as LSP says, so quick fixes and errors land on the right line.

### Internal

- **The chaos tests check every edit, not only that nothing crashes.**
  Quick fixes, renames, SPARQL actions, completion imports and `check --fix`
  are checked against the tokens and the graph they should leave. A
  watchdog names and saves any slow input, `CHAOS_ONLY` replays one, the
  JSON-LD run covers non-ASCII files, and no test is ignored. spargebra
  0.4.7 is vendored with two patches (`vendor/spargebra/PATCH.md`) and
  pinned exactly; a golden record checks the parsed form of every W3C query.

## 0.9.0 — 2026-10-23

### Fixed

- **Browser editor: the Vim status bar follows the focused editor of a
  split.** Giving the focus back to the first editor after `Ctrl+\` could
  leave the status showing the other editor's mode.

### Internal

- **CI runs in about a third of the time and tests more.** The test jobs
  build without fat LTO (the shipped profile stays for the nightly run, the
  Performance job and releases); the wall-clock unit tests run one at a time;
  the chaos loops run as four shards; one "CI passed" check gates the merge.
  New in CI: every `web/test` file (14 never ran), the extension's
  `find-server` and `configuration` tests, the memory budget on every pull
  request, the SHACL corpus files `src/knowledge.rs` reads, Neovim 0.11 and
  the Windows plugin install, the minimum Rust version, formatting of
  `web/engine`, a nightly chaos run with a fresh seed, and the release build
  on pull requests that change how it is packaged. The VS Code checks wait for
  the server's code action instead of asking once. Pages deploys only the tip
  of `main`; the Alpine build is smoke-tested on Alpine.

## 0.8.0 — 2026-10-23

### Security

- **`turtle-lsp.json` cannot turn on the network or lift a limit.** Any
  repository can ship this file, so it can no longer set `imports.network`,
  or raise `query.timeoutSeconds`, `query.maxRows`, `query.maxBufferedRows`
  or `shacl.maxTriples` past their defaults (`0`, no limit, counts as past).
  The server and the command line print one warning that names the keys
  they ignored. Editor settings and `--network` still can. The file is read
  only up to 1 MB.
- **Fetches stay on the public web, within limits.** Imports, JSON-LD
  contexts and the "Fetch … and cache it" quick fix follow at most 10
  redirects, each one checked. A redirect from `https` to `http` is refused. Loopback, private, link-local, cloud
  metadata and other internal addresses are refused, also when a host name
  resolves to one; set `TURTLE_LSP_ALLOW_PRIVATE_HOSTS=1` to allow them. One
  pass over the imports makes at most 100 requests and reads at most 100 MB.
  What it cannot fetch is reported as unresolved, with the reason.
- **Cache files are named by the SHA-256 of their URL** and record the URL
  (in their first line, or in a `.url` file beside a cached JSON-LD
  document), which is checked when the file is read, so a planted file is not
  taken for another URL. Files cached by older versions are not read; they are
  fetched again when needed.
- **`file:` imports, catalog entries and JSON-LD contexts** are read only
  inside the workspace folders. The check happens before any file is
  touched, and a missing file and a file outside get the same error. A
  `file:` URL that names a host other than `localhost` is refused, and so is
  a Windows network share unless the workspace is on one.
- **VS Code**: in an untrusted workspace `turtle.query.*`,
  `turtle.shacl.maxTriples` and `turtle.diagnostics.workspace` take your user
  settings only, as `turtle.server.path` and `turtle.imports.network`
  already did. `turtle.server.path` must be an absolute path to a file that
  exists, and relative `PATH` entries are never searched for the server.
- **Neovim**: `build.lua` downloads over `https` only (redirects included)
  and, when the `gh` command is installed, checks the archive with
  `gh attestation verify`; a failed check stops the install. The old server
  stays in place until the new one is complete.
- **Browser editor and toolbox**: both pages set a Content Security Policy.
- **Releases** are published only after CI passed on the tagged commit and
  the server in each `.vsix` ran on its own platform.

### Changed behaviour

- **A running query shows that it runs, and can be stopped.** ▶ Run query
  shows the cancellable progress "Running <file>" with the seconds so far,
  in editors that show progress. Where code lenses can be refreshed, the
  lens reads "■ Cancel query" until the run ends; it runs the new
  `turtle.cancelQuery` command. Both use only the protocol
  (`window/workDoneProgress`, `workspace/codeLens/refresh`).
- **`check` exits 2 when it did not cover everything**: a file that cannot be
  read or is not UTF-8 (reported as an `io` or `encoding` error, and the run
  goes on), a folder scan that stopped at 20,000 files
  (`TURTLE_LSP_MAX_FILES=N` changes the cap), a file over 50 MB that was
  skipped, a context file that could not be read, or a `--fix` that could
  not be written. Before, a scan stopped silently, and one unreadable file
  ended the run with no report.
- **`format` and `check --fix` replace a file through a temporary file** in
  its folder, with the file's permissions. A symbolic link stays and its
  target is replaced. A file that changed after it was read is left alone,
  and so is a read-only file.
- **The command line rejects what it used to accept quietly**: an unknown
  code in `--select` or `--ignore`, an unknown command (it started the
  language server and waited on stdin), `-` as a path, and `--statistics`
  with `--format` all exit 2. `turtle-lsp --version` prints to stdout, and
  `turtle-lsp help check` (or `format`, `query`) prints that command's
  options.
- SARIF reports percent-encode file paths; a file outside the root gets an
  absolute `file:` URI.
- **Queries**: a query stopped by its time limit or by a newer run is an
  error, never an answer (an ASK could answer `true` when the answer was
  `false`). A query that joins more than 200 patterns or path steps at once
  is refused before it runs. Table and Markdown cells show at most 1,000
  characters.
- **SPARQL**: an expression of more than 2,000 operators, a property path of
  more than 2,000 steps, or more than 3,000 unary operators in a row is one
  "too large to check" syntax error, in `.rq` files and in the queries of
  SHACL shapes, instead of a crash. `TRUE` and `False` are booleans.
- **Turtle, TriG, N-Triples and N-Quads are read against the document's URI
  as the base**, as JSON-LD already was: `<#me>` in two files names two
  resources, a relative `@base` is allowed, and query results show absolute
  IRIs. A prefixed name whose prefix is not declared is reported and no
  longer becomes a triple in queries or SHACL.
- `"text"@en--rtl` is a directional literal in queries: `LANG()` is `en` and
  `LANGDIR()` is `rtl`, and `=` compares the text, the language and the
  direction (the vendored query engine no longer panics on it).
- **SHACL reports what it could not check**, instead of passing it: a query
  that failed and a check cut short (too deep, or over the step budget) are
  warnings (`shacl-failure`); a shape whose `sh:path` cannot be read is an
  error (`shacl-invalid-path`), so `check` exits 1. Past the result budget,
  validation stops with one note. SPARQL targets may use `MINUS`, `VALUES`
  and `SERVICE`, and a shape with only a `sh:target` is validated.
- **`shacl.maxTriples` bounds the whole run**: the shared triples plus every
  TriG, N-Triples, N-Quads and named-graph JSON-LD dataset validated with
  them. Before, it applied to each dataset on its own.
- Annotation blocks `{| |}` count toward the 256-level nesting limit.
- **JSON-LD**: a lone surrogate escape (`"\ud800"`) is a warning
  (`json-lone-surrogate`), not an error; a key written twice is a warning
  (`json-duplicate-key`); formatting keeps a leading byte order mark. The
  undeclared-prefix lint and its fix leave `@json` literals alone, and a term
  rename follows the scoping of its `@context`.
- **Settings**: a time limit that is negative or too large to hold is
  ignored with a warning instead of a crash. In `turtle-lsp.json` it is an
  error for `check`, which exits 2.
- **VS Code sends only the settings you set**, so `turtle-lsp.json` applies
  in VS Code too, and a key you remove from the editor's settings falls back
  to the file. Once the editor sends its `turtle` settings section, that
  section replaces the start-up settings.
- **Neovim**: the project root is the nearest `turtle-lsp.json`, else the
  repository (`.git`), else the nearest `prefixes.ttl`, so a `prefixes.ttl`
  in a subfolder no longer splits a project into two servers. `.turtle`
  files are Turtle.
- **Vocabularies**: GeoSPARQL functions (`geof:sfWithin`) are no longer
  unknown terms; `https://schema.org/` is schema.org; names with a colon in
  the local part (`og:audio:album`) are found.
- **Rename and fixes write valid Turtle**: a new name that is not a valid
  IRI or prefix is refused; an IRI is shortened only to a valid prefixed
  name; an added `@prefix` goes above the first use it serves; inserted
  lines use the file's line ending; "Add all missing prefixes" does nothing
  in N-Triples and N-Quads.
- **Browser editor**: the workspace is kept per file in IndexedDB (database
  version 2; an older build of the editor cannot open it). Only one tab
  edits it; a second tab shows it read-only. Fix All and Format All leave a
  file that changed while they ran as it is. The Run button and Fix All use
  your settings. Opening a folder reads at most 1,000 files of up to 10 MB
  each.
- `npm run corpus` fails when a download fails; with `CORPUS_STRICT=1` a
  changed checksum fails it too.
- **The editor shows at most 1,000 syntax errors per document.** In a
  Turtle, TriG, N-Triples or N-Quads document with more, it shows the first
  1,000 in the text and one more error at the end of the last one: "More
  than 1000 syntax errors; the rest are not shown". Workspace diagnostics
  and the browser toolbox's check show the same. `turtle-lsp check` still
  reports every error, and `format` still counts them all. Before, the
  editor showed every error. JSON-LD keeps its cap of 100 JSON errors.
- **File kinds come from one table**, which changes three corner cases. A
  file named only `.owl`, `.rdf` or `.json` is now an import source or a
  watched file, as a file named only `.ttl` or `.rq` already was. On Linux
  and macOS, a file whose name contains `\` is judged by the part after the
  last `\`, so `x\catalog-v001.xml` is a catalog. A path that ends in a
  separator (`/w/a.owl/`) is no longer an import source.

### Fixed

- **"Organise prefixes" no longer empties a file whose lines end in a bare
  carriage return.** It read such a file as one line, so with every prefix
  unused it removed everything after the declarations too. A bare `\r` now
  ends a line there, as in the Turtle and SPARQL grammars.
- **A JSON-LD string cut off at the end of the file no longer crashes inlay
  hints.** Its span lost its last character, one byte shorter than the name
  it holds; a key written without quotes lost its first and last.
- A backslash before a line break in a short string, and a comment ended by
  a lone carriage return, no longer swallow the next statement.
- A JSON-LD document could get triples from a context it never used, when
  another document of the workspace applied the same remote context after an
  inline one.
- A panic while analysing an edited or indexed file, or in a background
  thread, is reported instead of ending the server. The server runs on a
  64 MiB stack.
- Deleting or renaming a folder removes its files from the index; changes
  made before the first index finished are kept; `.rq`, `.sparql`, `.ru` and
  `.json` context files are watched; a client that sends `shutdown` and
  never `exit` no longer keeps the server running (it ends after 30 s).
- VS Code: "Turtle: Restart Language Server" works after a failed start.
- "Fetch again" on an import that a catalog maps to a URL fetches it again.
- A SHACL validation thread or a workspace-diagnostics thread that cannot
  start is reported with a warning instead of ending the server. SHACL tries
  again on the next validation; workspace diagnostics stay off until the
  settings change.
- **SHACL validation of a deep `sh:node` chain read twice no longer
  hangs.** A chain of 3,000 shapes whose triples the validation reads twice
  (for example, a TriG dataset that repeats the shared shapes) ran past 60
  seconds, and a newer validation could not cancel it. Each shape check now
  counts as one step of the validation's step budget (40 million), so the
  run stops with a `shacl-failure` warning, and cancelling it stops it.

### Performance

- The "did you mean" search for undescribed terms is bounded: with 1,000
  near-miss IRIs against a 7,000-term namespace a keystroke takes about 2 ms
  (it took over 800 ms). At most 100 terms per document get a suggestion.
- SHACL builds the shared graph once and reads each dataset on top of it;
  path closures are computed once per run.
- Nested remote JSON-LD contexts have a work budget, and `@json` literals
  are canonicalized in linear time.
- `check` computes quick fixes only for the output formats that show them.
- Changed files from the file watcher are indexed in parallel; the queries of
  SHACL shapes are parsed once per version; typed completion over a large
  workspace stops at its result cap.
- The release binary is built with whole-program optimisation and stripped
  of symbols (12.2 MB instead of 18 MB), so a panic's backtrace shows no
  function names.
- **Less memory, and faster large answers.** The server writes each answer as JSON text, without a
  `serde_json::Value` tree in between: after 20 `documentSymbol` requests
  on a 100,000-triple file it holds 113 MB instead of 366 MB, and each
  request takes 38.7 ms instead of 49.5 ms. On a 30,000-line file,
  `documentSymbol` takes 6.1 ms instead of 8.3 ms, and full semantic tokens
  3.6 ms instead of 5.3 ms. Tokens are smaller, and `check` and `format` no
  longer keep a file's tokens after parsing it: `format --check` on a
  2.5 MB file peaks at 88 MB instead of about 154 MB (35 bytes per input
  byte instead of 61). At the end of the benchmark's editing run the server
  holds 157 MB instead of 210 MB.

### Internal

- The code is reorganised with no change to the protocol: one interface for
  editor requests (`ide::Session`), workspace changes returned as `Effects`,
  one layout engine for the Turtle and SPARQL formatters, one SHACL shape
  model, one file-kind table, and a public Rust interface cut to what the
  browser engine, the examples and the tests use.
- CI runs on Linux, macOS and Windows with the JSON-LD corpus, VS Code 1.91.0
  next to the stable one, the browser engine and the benchmark. The bench's
  memory budget checks mimalloc's peak commit (at most 64 bytes per input
  byte), the same figure on every system; 0.7.1 commits 88.

## 0.7.1 — 2026-10-22

### Features

- **SPARQL in SHACL shapes.** The query in a `sh:select`, `sh:ask`,
  `sh:construct` or `sh:update` string of a Turtle or TriG file gets the
  SPARQL highlighting, completion (keywords, the query's variables, the
  prefixes its `sh:prefixes` declare, terms) and hover of a `.rq` file, at
  their place in the string, escapes included. VS Code and the browser
  editor show suggestions as you type in Turtle and TriG strings
  (`editor.quickSuggestions` `"strings": "on"` for `[turtle]` and
  `[trig]`).

## 0.7.0 — 2026-10-22

### Features

- **Run SPARQL queries over the workspace.** A "▶ Run query" code lens above
  each query runs it over the workspace's files as the editor has them
  (unsaved edits included) and opens the results as Markdown, beside the
  query in VS Code, in any LSP editor through `window/showDocument`. It runs
  on its own thread with `query.timeoutSeconds` (default 30),
  `query.maxRows` (default 1000) and `query.maxBufferedRows` (default
  10 000 000, about 1 GB: a query that keeps more rows in memory to sort,
  group, deduplicate or join them is stopped). Query files of the same name
  get results files of their own (`people.md`, `people-2.md`).
  `turtle-lsp query` (`--max-buffered-rows`) does the same from the
  command line, with table, JSON, CSV, TSV or Turtle output. The browser editor has a ▶ Run button (Ctrl/Cmd+Enter) with a results pane. The default
  graph is the union of every file's triples; named graphs, updates and
  endpoints are out of scope.
- **Reference lenses**: "N references" above each subject a file describes
  (Turtle, TriG, N-Triples, N-Quads, JSON-LD); clicking lists them (VS Code's
  references view, Neovim's quickfix list, the browser editor's references
  view). Counted only when shown. Setting
  `codeLens.references` (default on).
- **Workspace diagnostics** (`diagnostics.workspace`, default off): problems
  in files that are not open, computed in the background with the same
  checks as `turtle-lsp check`, and kept up to date as files change. An edit
  pauses the work, so typing is not slowed.
- **`imports.unresolved`**: `"ignore"` keeps SHACL results at full severity
  while imports are unresolved, instead of softening them to warnings with a
  note (`"soften"`, the default). For projects that validate without their
  imports, as pySHACL does.

### Fixes

- VS Code passed `shacl.onType` to the server only after a settings change,
  not at start.
- A SHACL-SPARQL query stops after 10 s, with a warning on its `sh:select` or
  `sh:ask` (`shacl-sparql-timeout`) that its constraint's results are
  missing. Before, one slow constraint (two patterns with nothing in common)
  could keep validation busy for minutes, and edits could not stop it.

### Internals

- The graph and the SPARQL dataset adapter moved from SHACL to a shared
  `rdf` module; server commands are parsed in their own module.
- spareval is patched (`vendor/spareval`) so the query time limit and
  cancellation also stop joins that combine rows in memory. Unpatched, a
  cross product or two unconnected property paths ran on past the limit.

## 0.6.2 — 2026-10-21

### Features

- **Ill-typed literals** get a warning (`invalid-literal`) on the literal,
  naming its datatype: `"twelve"^^xsd:integer` gives "'twelve' is not a
  valid xsd:integer". Checked are the lexical forms of the numeric types
  (with the ranges of `xsd:byte`, `xsd:unsignedInt` and the other bounded
  integers), `xsd:boolean`, the date and time types (days per month and
  leap years included), the durations, `xsd:hexBinary`, `xsd:base64Binary`,
  `xsd:language` and `rdf:JSON`, in Turtle, TriG, N-Triples, N-Quads and
  JSON-LD, in the editor and in `turtle-lsp check`. SPARQL queries are not
  checked. SHACL `sh:datatype` checks the same lexical forms, so it now also
  rejects ill-formed `g*` dates, durations, binaries, `xsd:language` and
  `rdf:JSON` values.

### Fixes

- Fetching `owl:imports` and JSON-LD contexts over HTTPS uses `rustls-webpki`
  0.103.16 and `webpki-roots` 1.0.10, which no longer trust the distrusted
  emSign intermediates (RUSTSEC-2026-0381, RUSTSEC-2026-0382).

## 0.6.1 — 2026-10-21

### Fixes

- The VS Code Marketplace page described JSON-LD as it was before 0.6.0
  ("completion, rename and formatting ... not there yet"); it now lists what
  0.6.0 does, and the extension's name and description include JSON-LD.
- The browser toolbox names pasted JSON-LD `pasted.jsonld` instead of
  `pasted.trig`, and its page descriptions include JSON-LD.

### Tests and CI

- CI is green on Windows again. Two SHACL tests removed a folder their
  server was still running in, which Windows refuses; the failure skipped
  stopping the server, and the server held the test run open. A server a
  test forgets to stop now fails its file instead of hanging the run.
- The JSON-LD timing tests compare against Turtle on the same machine
  instead of fixed times, so slower CI runners pass.
- CI runs the build, the Rust tests and the Node tests as separate steps
  with their own time limits (the Node tests 5 minutes).

## 0.6.0 — 2026-10-20

### Features

- **`turtle-lsp check`**: reports from the command line what the editor
  shows (syntax errors, prefixes, vocabulary lints, SHACL validation) at
  file:line:col, for coding agents, CI and pre-commit. Exit codes 0 (clean),
  1 (problems) and 2 (the tool failed). Output formats `concise`, `json`,
  `github` (workflow annotations) and `sarif` (code scanning). `--fix` applies
  the safe quick fixes.
- **`turtle-lsp format`**: formats files in place; `--check` and `--diff`
  change nothing and exit 1 if a file would change.
- **`turtle-lsp.json`**: optional settings file at the workspace root, with
  the keys of the editor settings. The command line and the server both read
  it; the editor's settings override it.
- `.pre-commit-hooks.yaml` with the hooks `turtle-lsp-check` and
  `turtle-lsp-format`.
- **JSON-LD** (`.jsonld`, language id `jsonld`; JSON-LD 1.1), with an
  engine that keeps the source position of every triple:
  - diagnostics: JSON syntax errors with recovery (`json-syntax`), context
    errors (`jsonld-context`), keys the context does not define
    (`jsonld-undefined-term`), invalid IRIs and language tags
    (`invalid-iri`, `invalid-language-tag`), and the vocabulary lints on
    IRIs reached through the context;
  - JSON-LD files join the workspace: their terms count in Turtle and SPARQL
    files and the other way round, SHACL shapes in either format validate
    their data with each result on the key, value or `@id`, and named graphs
    follow the TriG rules;
  - hover on keys and `@type` values, go to definition on `@vocab` keys, and
    an outline of node objects;
  - `turtle-lsp check` and the browser editor cover them;
  - remote contexts come from workspace files, the import cache or the
    bundled schema.org, ActivityStreams, Verifiable Credentials and DID
    contexts; the network is used only with `imports.network` or
    `--network`.
  - W3C JSON-LD 1.1 suites: toRdf 443 pass, expand 365, remote-doc 6; 2
    known failures in the runner's oracle; the rest (JSON-LD 1.0, options
    the engine does not have, document loading) out of scope. On 364 public
    documents, 339 graphs match oxjsonld's; the other 25 are known oxjsonld
    differences from the spec.
  - Not yet: compaction, flattening, framing and `.json` files.
- **Editing inside JSON-LD files**:
  - completion in keys and `@type` values from the context in force at the
    cursor, workspace properties under `@vocab` and `prefix:local` names;
    completing a known prefix the context lacks adds it to the `@context`
    (idea credited to [SWLS](https://github.com/SemanticWebLanguageServer/swls));
  - go to definition on a term reaches its `@context` entry, also in a
    context file of the workspace; references and highlights on terms;
  - rename of terms and keyword aliases (the `@context` entry and every use),
    and of IRIs from any file, each JSON-LD use rewritten in its own form;
    unsafe renames are refused with the reason;
  - formatting in the editor, `turtle-lsp format` and the browser, keeping
    the same JSON value;
  - semantic tokens for keywords, properties and classes;
  - the outline nests a named graph's nodes under the graph.
- **`jsonld-undefined-prefix`**: a warning when `rdfs:label` and the like
  name a prefix the `@context` doesn't define, which JSON-LD reads as an IRI
  with the scheme `rdfs:`; the quick fix (also `check --fix`) adds the prefix
  to the `@context`.

### Performance

- **SHACL-SPARQL constraints** run once for all focus nodes of a shape
  instead of once per node. Validating 30 MB of generated TriG with 71
  SPARQL constraints per file went from 26 s to 0.6 s in the editor and the
  command line, with identical results.
- **JSON-LD** on a 30,000-line file: open to first diagnostics 7.5 ms
  (Turtle of the same length 5.9 ms), a keystroke 7.3 ms (Turtle 5.0 ms),
  hover 0.1 ms, completion 0.4 ms, formatting the whole file 2.1 ms.
  `tests/jsonld_budget.rs` holds release builds to opening within 3x Turtle
  and a keystroke under 10 ms.

### Fixes

- "Did you mean" now counts swapping two neighbouring letters as one edit, so
  `foaf:nmae` and `rdfs:lable` get a suggestion.

## 0.5.2 — 2026-10-20

### Features

- **Neovim with lazy.nvim in one step**: this repository is a Neovim plugin.
  `{ "abhishekkundu25/turtle-lsp", version = "*", config = function()
  vim.lsp.enable("turtle_lsp") end }` installs the server (downloaded for
  your machine from the release and checked against its checksums, or built
  with cargo where there is no download), its config for Turtle, TriG,
  N-Triples, N-Quads and SPARQL, the `.ttl` and `.nq` file types, and the
  modeline fix. No Rust needed.
- Each release has the server on its own for other editors:
  `turtle-lsp-<platform>.tar.gz` (`.zip` on Windows) for macOS, Linux and
  Windows, with checksums and build attestations.

## 0.5.1 — 2026-10-20

### Features

- Hover's "Described in core.ttl:18" is a link that opens the file at that
  line.
- **Try it in the browser**: the [RDF Editor](https://abhishekkundu25.github.io/turtle-lsp/) runs this server,
  compiled to WebAssembly, in a browser tab: a VS Code-like editor with the
  explorer, Problems, SHACL as you type, rename across files, quick fixes,
  Vim mode and a dark or light theme. Nothing is uploaded. A toolbox page
  formats, validates and checks files without the editor. The server's
  behaviour is unchanged.

## 0.5.0 — 2026-10-19

### Features

- **N-Quads** (`.nq`, language id `nquads`): syntax errors with recovery per
  line, highlighting, hover, label hints, navigation, an outline grouped by
  graph, and formatting (one quad per line). Its facts join the workspace like
  a TriG file's. W3C N-Quads suites (RDF 1.1 and 1.2): 155/155.
- **SPARQL** (`.rq`, `.sparql`, `.ru`, language id `sparql`; queries and
  updates, SPARQL 1.1 and 1.2), built on the same knowledge of the workspace
  and the vocabularies as Turtle:
  - diagnostics as you type: syntax errors at the token where the query stops
    making sense, undeclared, unused, duplicate and redefined prefixes,
    unknown or deprecated vocabulary terms, own terms nothing describes, and
    projected variables the `WHERE` clause never binds;
  - completion of keywords by context, variables, prefixes (adding the
    `PREFIX` line), terms ranked by position and by the ontology, and
    built-in functions;
  - hover, label hints, go to definition (terms, prefixes, variables),
    references and highlights (variables scoped by subquery), semantic
    highlighting, folding and an outline;
  - rename of variables, prefixes, and terms across Turtle, N-Quads and SPARQL
    files in both directions;
  - formatting that never changes what the query means (the output must parse
    to the same algebra), and refuses queries with errors;
  - quick fixes and refactorings: add a missing `PREFIX`, add all, remove an
    unused one, fix a misspelled term, organise prefixes, shorten or expand
    IRIs.
  W3C SPARQL 1.1 and 1.2 syntax suites: 425/426. Prefixes from `prefixes.ttl`
  and `turtle.prefixes` count as declared; a query never makes a term look
  described. The server never runs a query.
- The VS Code extension registers N-Quads and SPARQL with their own grammars,
  and is listed as **Turtle RDF & SPARQL Language Server (TTL, OWL, SHACL)**;
  the README shows the Neovim setup for `nquads` and `sparql` buffers.

### Fixes

- SHACL-SPARQL queries with a local name of two dots or more
  (`ec:1.1.1.353`) are no longer reported as syntax errors.
- A SHACL-SPARQL query nested deep enough to overflow the parser's recursion
  is reported instead of crashing validation.
- Two crashes in locating a SHACL-SPARQL syntax error next to a multi-byte
  character are fixed.

### Testing

- `npm run corpus` also checks N-Quads against oxttl (a Web Data Commons
  dump) and 1,317 real SPARQL queries (SIB sparql-examples) against
  spargebra and the formatter.

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
