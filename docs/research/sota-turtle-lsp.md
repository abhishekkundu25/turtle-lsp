# State of the art: a Turtle language server for Neovim

Research date: 2026-10-13. Every claim below links to the source that owns it. Where a source was unreachable or a claim could not be checked, the text says so.

## Summary

- **No existing Turtle server is clearly better than what this repo could become.** The Stardog servers (which this repo builds on) offer only hover, folding and diagnostics for Turtle, and are pinned to chevrotain 4.7.0 and vscode-languageserver 5. The newer projects are small and young: zed-rdf's `rdf-lsp` (Rust), shifty-lsp (Python, SHACL plus `owl:imports`) and onto-vis (VS Code, cross-file IRI index). The best design reference is **Qlue-ls**, a SPARQL server. It uses a resilient LL parser over a lossless rowan tree and has prefix code actions and diagnostics that carry over to Turtle directly.
- **The parser is the decision that matters most.** Neither millan nor tree-sitter-turtle supports RDF 1.2 syntax. N3.js does support RDF 1.2, but it stops at the first error. A hand-written resilient recursive-descent parser that produces a lossless CST is the best fit. Turtle's grammar is small and all features can share one tree.
- **RDF 1.2 Turtle is still a Working Draft** (7 Oct 2026). RDF 1.2 Concepts is a Candidate Recommendation (7 Apr 2026). Support `<<( )>>`, `<< >>`, `~`, `{| |}`, `@lang--dir` and `VERSION` now, but treat the details as subject to change.
- **Neovim 0.11 (installed locally: 0.11.5) already supports** pull diagnostics, semantic tokens full/delta, inlay hints, type hierarchy, codelens and `didChangeWatchedFiles` on macOS. Neovim 0.12 (current stable 0.12.6) adds `gx` on documentLink, semanticTokens/range, onTypeFormatting, selectionRange and workspace diagnostics.

## Landscape

| Tool | Stack | Turtle features | Notes / status |
|---|---|---|---|
| [stardog-union/stardog-language-servers](https://github.com/stardog-union/stardog-language-servers) `turtle-language-server` | TS, millan | Hover, folding, diagnostics ([source](https://github.com/stardog-union/stardog-language-servers/blob/master/packages/turtle-language-server/src/TurtleLanguageServer.ts)) | README lists only hovers, completion and diagnostics across all its servers. `stardog-language-utils` depends on `vscode-languageserver ^5.2.1` ([package.json](https://github.com/stardog-union/stardog-language-servers/blob/master/packages/stardog-language-utils/package.json)). This repo uses v9 and casts `conn as any` (`src/server.ts:48`). Last push 2026-04-06. |
| [stardog-union/millan](https://github.com/stardog-union/millan) | TS, chevrotain 4.7.0 | Error-tolerant CST and token list | `recoveryEnabled: true` ([TurtleParser.ts](https://github.com/stardog-union/millan/blob/master/src/turtle/TurtleParser.ts)). Its `<< >>` and `{ }` "edge properties" are Stardog's own non-nesting RDF\* variant, which the source says "intentionally does not conform to the RDF\* spec". It has no RDF 1.2 syntax. Chevrotain is now 13.2.0 ([npm](https://registry.npmjs.org/chevrotain/latest)). |
| [IoannisNezis/Qlue-ls](https://github.com/IoannisNezis/Qlue-ls) | Rust, `ll-sparql-parser` (logos + rowan) | SPARQL only: completion, formatting, on-type formatting, hover, rename, references, diagnostics, code actions ([capabilities](https://docs.qlue-ls.com/02_capabilities/)) | "A resilient LL parser for SPARQL 1.2" with a lossless CST ([crate](https://github.com/IoannisNezis/Qlue-ls/tree/main/crates/sparql-parser)). Diagnostics include undeclared prefix, unused prefix, duplicate prefix and uncompacted URI. Code actions include declare prefix, remove prefix, shorten URI(s) and contract same-subject triples. A Neovim plugin exists ([qluels-nvim](https://github.com/DeaconDesperado/qluels-nvim)). |
| [sparkling/zed-rdf](https://github.com/sparkling/zed-rdf) `rdf-lsp` | Rust | Diagnostics, hover (513 curated terms), completion, prefix goto, document symbols, formatting, prefix rename, code actions (sort, add or extract prefix), semantic tokens ([README](https://github.com/sparkling/zed-rdf)) | Claims to pass all W3C Turtle and TriG manifests and to use statement-boundary re-parsing. It has 1 star and every claim is self-reported. It works offline and has no network access. |
| [gtfierro/shifty-lsp](https://github.com/gtfierro/shifty-lsp) | Python, rdflib, ontoenv, shifty | SHACL plus SHACL-AF validation as diagnostics, `owl:imports` resolution with 404s reported as diagnostics, completion, hover, goto, repo-wide references, outline, quickfix for missing prefix plus `owl:imports` ([README](https://github.com/gtfierro/shifty-lsp)) | Debounces about 0.8 s and runs on a background thread, and drops stale validation results. Ships a Neovim ≥0.11 `lsp/` config. |
| [rsl-innovation/onto-vis](https://github.com/rsl-innovation/onto-vis) | VS Code extension | Diagnostics (multiple errors at once), semantic highlighting by term role, hover from `rdfs:label`/`comment`, completion that inserts `@prefix`, goto, references, cross-file and cross-format rename, workspace symbols, formatting, graph preview ([README](https://github.com/rsl-innovation/onto-vis)) | Its index is "keyed by resolved IRI" across `.ttl`, `.owl` and `.rdf` files. That is the right model for this repo too. |
| [BrickSchema/langserver](https://github.com/BrickSchema/langserver) | Python | Syntax check including undefined namespaces, completion after `loadGraphs` ([README](https://github.com/BrickSchema/langserver)) | Last push 2021. Configured through coc.nvim. |
| [organization-research-group/turtle-lsp](https://github.com/organization-research-group/turtle-lsp) | Node, N3.js | Hover expansion of CURIEs only ([README](https://github.com/organization-research-group/turtle-lsp)) | Parses only the prefix preamble on the fast path. |
| [symbolith/rdf-lsp](https://github.com/symbolith/rdf-lsp) | Rust, tower-lsp | Not documented ([README](https://github.com/symbolith/rdf-lsp)) | No feature information available. |
| [GordianDziwis/tree-sitter-turtle](https://github.com/GordianDziwis/tree-sitter-turtle) | tree-sitter | Grammar only | Last commit 2024-07-02. Has no `<<` rules, and `lang_tag` has no `--dir` suffix ([grammar.js](https://github.com/GordianDziwis/tree-sitter-turtle/blob/main/grammar.js)), so it supports no RDF 1.2. nvim-treesitter pins it for `turtle` ([parsers.lua](https://github.com/nvim-treesitter/nvim-treesitter/blob/main/lua/nvim-treesitter/parsers.lua)). |
| [GordianDziwis/tree-sitter-sparql](https://github.com/GordianDziwis/tree-sitter-sparql) | tree-sitter | Grammar only | Pinned by nvim-treesitter for `sparql` (same file). |
| [TopQuadrant/shacl](https://github.com/TopQuadrant/shacl) | Java, Jena | SHACL Core, SHACL-SPARQL, SHACL-AF | "Serves as a reference implementation of the SHACL spec" ([README](https://github.com/TopQuadrant/shacl)). It is a validator, not an editor. |
| Protégé | Java desktop | Class and object-property hierarchy views (asserted or inferred), Usage view, Annotations view ([docs](https://protegeproject.github.io/protege/views/)) | These map to LSP typeHierarchy, references and hover. |
| Zazuko | JS libraries | [rdf-vocabularies](https://github.com/zazuko/rdf-vocabularies), [rdf-validate-shacl](https://github.com/zazuko/rdf-validate-shacl), [shacl-playground](https://github.com/zazuko/shacl-playground) | I found no Zazuko Turtle editor extension or language server. Their VS Code repo is [SPARQL snippets](https://github.com/zazuko/vscode-sparql-snippets). |

I could not identify a project named "mtt". A GitHub search for "rdf lsp" and "turtle language server" turned up nothing beyond the projects above.

## Findings by area

### 1. RDF 1.2 and Turtle 1.2 status

- RDF 1.2 Turtle is a **W3C Working Draft dated 7 October 2026** ([TR](https://www.w3.org/TR/rdf12-turtle/)), and so is TriG ([TR](https://www.w3.org/TR/rdf12-trig/)). RDF 1.2 Concepts is a **Candidate Recommendation Snapshot dated 7 April 2026** ([TR](https://www.w3.org/TR/rdf12-concepts/)). SHACL 1.2 Core is a WD dated 11 October 2026 ([TR](https://www.w3.org/TR/shacl12-core/)).
- New Turtle syntax ([Turtle 1.2](https://www.w3.org/TR/rdf12-turtle/)):
  - `tripleTerm` `<<( s p o )>>`: object position only, and it can nest.
  - `reifiedTriple` `<< s p o ~ r >>`: sugar for an `rdf:reifies` triple. Without a reifier the parser allocates a fresh blank node.
  - `reifier` `'~' (iri | BlankNode)?`
  - `annotation` and `annotationBlock` `{| predicateObjectList |}`: these may carry several reifiers.
  - `LANG_DIR` `@en--ltr`/`--rtl`: the literal gets the datatype `rdf:dirLangString`.
  - `VERSION "1.2"` or `@version`: only a hint. Parsers "may warn".
- RDF 1.2 Concepts defines **Full** conformance (with triple terms) and **Basic** conformance (without) ([Concepts](https://www.w3.org/TR/rdf12-concepts/)). Basic-only files are a good candidate for a lint.
- Parser support: N3.js says it handles RDF 1.2 and has `maxTripleTermDepth` and a `version` writer option ([README](https://github.com/rdfjs/N3.js)). The repo pins `n3@1.26.0` while npm latest is 2.13.12 ([npm](https://registry.npmjs.org/n3/latest)). millan and tree-sitter-turtle do not support RDF 1.2 (see the table).

### 2. LSP 3.17 features that matter for Turtle

All of the following are in the [3.17 spec](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/). The repo's `vscode-languageserver@9.0.1` / protocol `3.17.5` ships helpers for each (`lib/common/{diagnostic,semanticTokens,inlayHint,typeHierarchy,callHierarchy}.d.ts` in `node_modules/vscode-languageserver`).

| Feature | Spec point | Turtle use | Neovim support |
|---|---|---|---|
| Incremental sync | `TextDocumentSyncKind.Incremental = 2` | Avoid re-sending the whole file on each keystroke. Required for incremental re-parsing. | Default `allow_incremental_sync = true` ([lsp.txt](https://github.com/neovim/neovim/blob/master/runtime/doc/lsp.txt)) |
| Pull diagnostics | `textDocument/diagnostic`. A report is `'full'` or `'unchanged'` (with `resultId`). `interFileDependencies` is "typically uncommon for linters". `workspace/diagnostic/refresh` lets the server ask the client to re-pull. | The client decides when to pull, so no server-side push loop is needed. Return `unchanged` when the version has not changed. Set `interFileDependencies: true` once diagnostics depend on other workspace files such as imported ontologies or shapes. | Since 0.10 ([news-0.10](https://github.com/neovim/neovim/blob/master/runtime/doc/news-0.10.txt)). 0.11.5 re-pulls after every `didChange`/`didOpen`, for visible buffers only ([diagnostic.lua](https://github.com/neovim/neovim/blob/v0.11.5/runtime/lua/vim/lsp/diagnostic.lua)). Since 0.11, a new request cancels the one in flight ([news-0.11](https://github.com/neovim/neovim/blob/master/runtime/doc/news-0.11.txt)). `workspace/diagnostic` and refresh arrived in 0.12 ([news-0.12](https://github.com/neovim/neovim/blob/master/runtime/doc/news-0.12.txt)). |
| Semantic tokens | full, full/delta (`resultId` plus edits), range | Colour by role: prefix label vs local name, class vs property vs individual (from the index), undefined term, deprecated (`owl:deprecated`). tree-sitter cannot do this. | 0.9+. 0.11.5 advertises `full = { delta = true }, range = false` ([protocol.lua](https://github.com/neovim/neovim/blob/v0.11.5/runtime/lua/vim/lsp/protocol.lua)). Range arrived in 0.12. Highlights are added on top of treesitter, not in place of it ([lsp.txt](https://github.com/neovim/neovim/blob/master/runtime/doc/lsp.txt)). |
| Code actions | `textDocument/codeAction` | Add missing prefix (from prefix.cc or the Zazuko table), remove unused prefix, sort prefixes, shorten `<iri>` to a CURIE, expand a CURIE. All of these exist in Qlue-ls ([capabilities](https://docs.qlue-ls.com/02_capabilities/)) or zed-rdf. | `gra` default keymap ([lsp.txt](https://github.com/neovim/neovim/blob/master/runtime/doc/lsp.txt)) |
| Formatting and range formatting | `textDocument/formatting`, `rangeFormatting` | See §6 | `formatexpr` is set to `vim.lsp.formatexpr()`, so `gq` works. 0.11 supports `rangesFormatting` ([news-0.11](https://github.com/neovim/neovim/blob/master/runtime/doc/news-0.11.txt)). |
| On-type formatting | `textDocument/onTypeFormatting` | Qlue-ls aligns predicates on Enter after `;` ([capabilities](https://docs.qlue-ls.com/02_capabilities/)) | 0.12 only, opt-in |
| Inlay hints | `textDocument/inlayHint` plus `inlayHint/resolve` | Show the `rdfs:label`/`skos:prefLabel` after opaque IRIs (`wd:Q42`, `obo:GO_0008150`). | 0.10+. Must be enabled with `vim.lsp.inlay_hint.enable()` ([lsp.txt](https://github.com/neovim/neovim/blob/master/runtime/doc/lsp.txt)). |
| Document links | `textDocument/documentLink` plus resolve | Make `<http://…>` IRIs and expanded CURIEs clickable. `owl:imports` targets can point at a local file. | In 0.12, `gx` opens documentLink items ([news-0.12](https://github.com/neovim/neovim/blob/master/runtime/doc/news-0.12.txt)) |
| Type hierarchy | `prepareTypeHierarchy`, `typeHierarchy/supertypes`/`subtypes` | Map supertypes and subtypes to `rdfs:subClassOf`, and `rdfs:subPropertyOf` for properties. This is Protégé's Class Hierarchy view. | `vim.lsp.buf.typehierarchy()` since 0.10 |
| Call hierarchy | `prepareCallHierarchy`, incoming/outgoing | Possible misuse: "incoming" could mean subjects that point to this node and "outgoing" the objects it points to. This is not standard semantics. | `incoming_calls()` / `outgoing_calls()` exist |
| Workspace symbols | `workspace/symbol` | Every subject defined across `*.ttl` files, labelled by `rdfs:label` | `vim.lsp.buf.workspace_symbol()`. `tagfunc` falls back to it. |
| Watched files | `workspace/didChangeWatchedFiles` | Re-index ontology and shape files that change outside the editor | Enabled on macOS and Windows only. Advertised as `dynamicRegistration` only, so **the server must call `client/registerCapability`**. Neovim has no static watching ([protocol.lua](https://github.com/neovim/neovim/blob/v0.11.5/runtime/lua/vim/lsp/protocol.lua), [news-0.10](https://github.com/neovim/neovim/blob/master/runtime/doc/news-0.10.txt)). |
| Position encoding | `general.positionEncodings`, default `utf-16` | Neovim offers `utf-8`, `utf-16` and `utf-32`. `vscode-languageserver-textdocument` works in UTF-16, so keep UTF-16 unless the CST stores byte offsets. | ([protocol.lua](https://github.com/neovim/neovim/blob/v0.11.5/runtime/lua/vim/lsp/protocol.lua)) |
| Selection range, folding | `selectionRange`, `foldingRange` | Expand from term to triple to subject block to document | `v_an`/`v_in` in 0.12 |

### 3. Fault-tolerant and incremental parsing

| Option | Fault tolerance | Incremental | RDF 1.2 | Cost in Node |
|---|---|---|---|---|
| **web-tree-sitter** | Built in. tree-sitter "aims to be … robust enough to provide useful results even in the presence of syntax errors" ([docs](https://github.com/tree-sitter/tree-sitter/blob/master/docs/src/index.md)) | Yes: `tree.edit()` then `parser.parse(newText, oldTree)` ([binding_web README](https://github.com/tree-sitter/tree-sitter/blob/master/lib/binding_web/README.md)) | No, the grammar needs a fork | Wasm "in Node.js is considerably slower than running Node.js bindings" (same README). ABI and dynamic-linking compatibility needs care. Error nodes are generic (`ERROR`/`MISSING`), so diagnostics need their own message layer. |
| **chevrotain** (millan) | Single-token insertion and deletion, re-sync between rules and in repetitions ([tutorial](https://github.com/Chevrotain/chevrotain/blob/master/packages/website/docs/tutorial/step4_fault_tolerance.md)) | No, it re-parses the whole file | millan: no | millan is stuck on chevrotain 4.7.0. Upgrading means forking millan. |
| **N3.js** | None. `_error()` calls the callback and then sets `this._callback = noop` ([N3Parser.js](https://github.com/rdfjs/N3.js/blob/main/src/N3Parser.js)), so parsing stops at the first error | No | Yes | Good as a *validator* and quad producer. It has no positions apart from `onToken`. Source positions need an experimental add-on ([README](https://github.com/rdfjs/N3.js)). |
| **Hand-written resilient LL** | Recovery sets, "fuel" against infinite loops, error nodes, lossless tree ([matklad](https://matklad.github.io/2023/05/21/resilient-ll-parsing-tutorial.html)). This is the approach Qlue-ls ([crate](https://github.com/IoannisNezis/Qlue-ls/tree/main/crates/sparql-parser)) and rust-analyzer take. | Statement-level re-parse is easy, since Turtle statements end at a top-level `.` | You write it to the spec | Plain TS with no wasm. Turtle has about 20 productions. |

Recommendation: write a **hand-written lexer plus resilient recursive-descent parser in TS** that produces a lossless CST with offsets. Recover at `.`, `;`, `,`, `]`, `)` and at directive keywords. On each change, re-lex and re-parse the whole file first. A 10k-line file is cheap to parse; measure before adding statement-window re-parsing like zed-rdf's (self-reported 562 µs for semantic tokens on 10k lines ([README](https://github.com/sparkling/zed-rdf))). Use the W3C test manifests ([rdf11/rdf-turtle and rdf12/rdf-turtle in w3c/rdf-tests](https://github.com/w3c/rdf-tests/tree/main/rdf/rdf12/rdf-turtle)) as a conformance check. Keep tree-sitter on the Neovim side for syntax highlighting only.

### 4. Vocabulary and ontology resolution

- **Bundled offline vocabularies.** The old monolithic `@zazuko/rdf-vocabularies` package (which this repo uses) has been replaced by `@zazuko/prefixes` plus per-vocabulary `@vocabulary/*` packages, or `@zazuko/vocabularies`. These are **ESM-only**, and each vocabulary is exported as a JS module, "imported synchronously (no parsing required)" ([README](https://github.com/zazuko/rdf-vocabularies)). The repo is CommonJS (`package.json` `"type": "commonjs"`), so migrating needs ESM or dynamic `import()`.
- **prefix.cc.** Lookup is `https://prefix.cc/{prefix}.file.{format}`, for example `foaf.file.json` → `{"foaf":"http://xmlns.com/foaf/0.1/"}`. "Bulk lookup … is not supported. Please download the entire namespace data" ([API page](https://prefix.cc/about/api)). `https://prefix.cc/context` returned 200 (204 KB). **Observed on 2026-10-13: prefix.cc's TLS certificate expired on 31 Dec 2025.** Node's `fetch` will reject it, so ship a vendored snapshot rather than calling it live.
- **LOV.** v2 has `term/search` (with `q`, `type`, `vocab`, `page_size`) and `term/autocomplete` according to a [third-party mirror of the API doc](https://backiee.wasmer.app/https_lov_linkeddata_es/dataset/lov/api). **I could not verify this:** `lov.linkeddata.es` timed out or returned 404 from here on 2026-10-13. Treat LOV as optional, opt-in and network-gated.
- **`owl:imports`.** Follow OWL 2 import semantics ([OWL 2 Structural Spec §3.4](https://www.w3.org/TR/owl2-syntax/#Imports)). shifty-lsp shows a working pattern ([README](https://github.com/gtfierro/shifty-lsp)):
  - map imports to workspace files first, then to a download cache
  - report unresolved imports as diagnostics on the `owl:imports` line
  - cache the import closure per document and rebuild it only when the import set changes
  - never download from request handlers
  - remember failures for a minute instead of retrying on every keystroke
- **Recommended resolution order:**
  1. open buffers
  2. workspace `*.ttl`/`*.owl` files, indexed by IRI (onto-vis model)
  3. `owl:imports` cache on disk, e.g. under `$XDG_CACHE_HOME`
  4. bundled Zazuko vocabularies
  5. prefix snapshot
  6. network (opt-in)

  Every layer must work offline.

### 5. In-editor SHACL validation

- **shacl-engine** (rdf-ext) covers SHACL Core and SPARQL-based constraints and targets via `shacl-engine/sparql.js`, but not SHACL-AF. Its README claims it is "15-26x faster than other JavaScript or Python packages" ([README](https://github.com/rdf-ext/shacl-engine), [benchmark post](https://www.bergnet.org/2023/03/2023/shacl-engine/)). Its `coverage`/`debug` output could map results back to source triples.
- **rdf-validate-shacl** (Zazuko) is pure JS on RDF/JS and returns a `ValidationReport` with `focusNode`, `path`, `severity` and `sourceShape` ([README](https://github.com/zazuko/rdf-validate-shacl/tree/master/packages/shacl)).
- Neither README gives absolute latency numbers. Measure on representative files.
- Design: validate on **save or idle, not on every keystroke**. Run validation in a `worker_thread` ([Node docs](https://nodejs.org/api/worker_threads.html)), drop stale results (as shifty-lsp does), and map `focusNode`/`value` to CST ranges through the term index rather than with text search. Shapes come from workspace files that are typed `sh:NodeShape`, or from a configured path.

### 6. Formatting

| Formatter | Input | Notes |
|---|---|---|
| Jena RIOT `TURTLE_PRETTY` / `BLOCKS` / `FLAT` / `LONG` ([docs](https://jena.apache.org/documentation/io/rdf-output.html)) | Graph | PRETTY groups by subject, uses `( )` and `[ ]` where possible, and needs a full scan first. `ttl:indentStyle=long` avoids wide predicate-object alignment. `ttl:directiveStyle` chooses `PREFIX` or `@prefix`. |
| rdflib `TurtleSerializer` ([source](https://github.com/RDFLib/rdflib/blob/main/rdflib/plugins/serializers/turtle.py)) | Graph | `topClasses = [RDFS.Class]` comes first, `predicateOrder = [RDF.type, RDFS.label]`, 4-space indent. |
| turtle-formatter (atextor) ([README](https://github.com/atextor/turtle-formatter)) | Jena Model | Archived and moved to cool-rdf. Its options are the best catalogue of style choices: `alignPrefixes`, `firstPredicateInNewLine`, `useAForRdfType`, `keepUnusedPrefixes` (off by default), `subjectOrder`, `predicateOrder` (default `rdf:type`, `rdfs:label`, `rdfs:comment`, `dcterms:description`), `wrapListItems`, `anonymousNode`, and lists always written as `( )`. |
| turtlefmt (helsing-ai) ([README](https://github.com/helsing-ai/turtlefmt)) | Text | Its example output keeps comments. `--check` prints a patch. "Output format is not stable yet." |
| @jeswr/pretty-turtle ([README](https://github.com/jeswr/pretty-turtle)) | RDF/JS quads | Node. Supports RDF-star. Options: `prefixes`, `compact`, `ordered`, `baseIri`. |

Every graph-based formatter (Jena, rdflib, turtle-formatter, pretty-turtle) starts from triples, so it **cannot keep comments, blank lines, literal forms or the author's ordering**. That is unacceptable for an editor format command. An LSP formatter should **work on the CST**:
- re-indent and re-space tokens
- align `;` predicate lists
- put one object per line once a line exceeds its width
- normalize directive style
- optionally sort prefixes
- **never reorder statements** unless the user asks through an explicit code action ("sort subjects/predicates")

Output must be idempotent (zed-rdf states this as a requirement ([README](https://github.com/sparkling/zed-rdf))). Range formatting should snap to whole statements.

### 7. Performance, cancellation and Neovim specifics

- **Budgets.** 0.1 s is the "limit for users feeling that they are directly manipulating objects" ([NN/g](https://www.nngroup.com/articles/response-times-3-important-limits/)). Neovim already debounces `didChange` by 150 ms by default (`debounce_text_changes`, [lsp.txt](https://github.com/neovim/neovim/blob/master/runtime/doc/lsp.txt)). Suggested server-side targets, which are my own and not from a spec:
  - parse plus index: under 10 ms for a 5k-line file
  - completion and hover: under 20 ms
  - pull diagnostics (syntax and local lints): under 50 ms
  - semantic tokens: under 30 ms
  - SHACL: off the main thread, on save or idle
- **Cancellation.** The spec says servers should not cancel requests just because a later change is queued. When internal state invalidates a request in flight, the server *may* return `ContentModified` (-32801), and clients should not show that error to the user ([spec, Implementation Considerations](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)). Neovim cancels its own pull-diagnostic and inlay-hint requests when it sends new ones (0.11 news). Implications, which are my inference: make every handler synchronous over a cached per-version snapshot (parse once per version, lazily, memoized), and check `CancellationToken` only in long loops such as workspace symbol and references. Node runs JS on one thread, so a cancel message cannot arrive while a handler is busy.
- **Pull vs push.** In Neovim 0.11, pull diagnostics re-request after every change ([diagnostic.lua](https://github.com/neovim/neovim/blob/v0.11.5/runtime/lua/vim/lsp/diagnostic.lua)). So the server needs no debounce timer of its own for syntax diagnostics. Compute them on request and cache them by `(uri, version)` → `resultId`. Keep push only for slow asynchronous results such as SHACL, or use `workspace/diagnostic/refresh`, which needs 0.12.

## Prioritized features

**P0: foundation (refactor)**
1. A single document model: incremental sync → text → **one** lexer/CST per version, memoized. Every feature reads from it. Remove the regex line scanning and the repeated parses.
2. Drop `stardog-language-utils`/millan (vscode-languageserver 5 seam, chevrotain 4.7, no RDF 1.2). Write a resilient TS Turtle 1.2 parser and check it against the W3C test suite.
3. Pull diagnostics: syntax errors, undefined prefix, duplicate prefix, unused prefix (marked with the `Unnecessary` tag), invalid IRI and language tag, literal datatype lexical form (opt-in). Support the `unchanged` report.
4. Semantics derived from the CST: prefix map with ranges, term occurrences keyed by expanded IRI, subjects with their `rdf:type` and labels.
5. Completion (CURIEs from the prefix map, the index and bundled vocabularies; prefix names; `a`; directives), hover (expanded IRI, label, comment, types, source), definition, references, rename (prefix label and IRI), document symbols, folding.
6. Code actions: add missing prefix, remove unused prefix(es), sort prefixes, shorten IRI to CURIE.

**P1: workspace and Neovim polish**
1. Workspace index of `*.ttl` files keyed by IRI. Watch files via **dynamic** `didChangeWatchedFiles` registration. Add workspace symbols.
2. Semantic tokens full/delta (class, property, individual, prefix, undefined, deprecated).
3. A CST formatter that preserves comments, plus range formatting.
4. Inlay hints showing labels for opaque IRIs (opt-in, resolved lazily).
5. Type hierarchy over `rdfs:subClassOf` and `rdfs:subPropertyOf`.
6. `owl:imports` resolution with a disk cache and diagnostics for unresolved imports. Offline-first vocabulary layers: Zazuko ESM packages plus a vendored prefix.cc snapshot.

**P2: heavy and optional**
1. SHACL validation (shacl-engine in a worker) on save.
2. Document links for IRIs (Neovim 0.12 `gx`).
3. On-type formatting (align after `;`), selection range.
4. Remote vocabulary lookup (LOV) behind an opt-in setting.
5. TriG and N-Triples sharing the same lexer.
6. Lint for RDF 1.2 Basic vs Full conformance and for a missing `VERSION`.

## Architecture recommendations (TS, Node, Neovim)

- **Layers:**
  1. `syntax` (lexer, CST, errors; pure and synchronous)
  2. `semantics` (prefixes, terms, statements derived from the CST; memoized per version)
  3. `workspace` (IRI index across files plus vocabulary layers; updated incrementally per file)
  4. `features` (one module per LSP method, reading snapshots)
  5. `server` (vscode-languageserver 9 wiring only)

  Every layer below `server` can be tested without LSP.
- **Snapshots, not events.** Handlers call `getSnapshot(uri)`, which parses lazily and is keyed by document version. Nothing parses in `onDidChangeContent`.
- **Offsets everywhere.** Convert to and from LSP `Position` only at the edge, through `TextDocument.positionAt`/`offsetAt` (UTF-16).
- **Background work** (imports, SHACL, network) runs in a worker or async queue, with stale-result dropping and a cache keyed by content hash. Request handlers must never block on I/O.
- **Neovim config:** ship an `lsp/turtle.lua` for `vim.lsp.config`/`vim.lsp.enable` (0.11+). Document that `vim.lsp.inlay_hint.enable()` needs an explicit opt-in and that file watching relies on dynamic registration (works on macOS).
- **Dependencies:** upgrade `n3` to 2.x if it stays in use as the quad producer for SHACL. Replace `@zazuko/rdf-vocabularies` with `@zazuko/prefixes` plus `@vocabulary/*`, which needs ESM output or `import()`.

## Open questions

1. How much RDF 1.2 to support while Turtle 1.2 is a WD: everything behind a flag, or by default?
2. Formatter style defaults (`PREFIX` vs `@prefix`, alignment, line width). Should the tool take an `.editorconfig` or a `turtle-lsp.toml` like Qlue-ls's TOML config ([README](https://github.com/IoannisNezis/Qlue-ls))?
3. Where SHACL shapes come from: a convention, a configured path, or `owl:imports`?
4. Whether LOV is still operated. It was unreachable from here, and only a mirror documents its API.
5. Whether to hijack call hierarchy for incoming and outgoing triple edges, or keep to references only.
6. Actual parse and validation timings on the user's real files. All the numbers here are targets or self-reported, not measured.
