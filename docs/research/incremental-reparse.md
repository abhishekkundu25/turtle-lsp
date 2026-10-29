# Incremental re-parsing: is it common, and how should turtle-lsp do it?

Research date: 2026-10-29, against commit `757e29d` of this repo. Every
claim about another project links to the source that owns it, pinned to a
commit or tag. Claims about this repo cite `file:line` at that commit. No
code was run for this note: the cost figures are the 2026-10-29 profile
quoted below and estimates derived from it.

## Summary

- **Most language servers re-parse the whole file on every change.** The
  JSON and YAML servers, the Stardog Turtle server, Mentor, Qlue-ls (SPARQL)
  and zed-rdf all do. zed-rdf's `incremental.rs` claims statement-boundary
  re-parsing, but its code always does a full parse. rust-analyzer has an
  incremental re-parser, but its main `parse` query does not use it, and its
  docs say re-parsing from scratch "seems to be fast enough". TypeScript's
  Go port (TypeScript 7) has not ported the incremental parser.
- **The systems that do reuse the old tree** (tree-sitter, TypeScript 6 and
  earlier, Roslyn, IntelliJ) share four rules: widen the edit by the
  lookahead of the token or node before it; never reuse a node that touched
  the edit, holds an error, or is missing or zero-width; reuse a node only
  if the parser state it was parsed under is the same (TypeScript's context
  flags, Roslyn's preprocessor directives); fall back to a full parse when in
  doubt. They test this against a full parse (rust-analyzer's fuzz target,
  TypeScript's `aggressiveChecks`).
- **The bigger wins downstream come from stable summaries, not from the
  parser.** rust-analyzer's rule is "typing inside a function's body never
  invalidates global derived data". salsa gets this by recomputing a step
  and stopping when the result is unchanged ("backdating"). For turtle-lsp
  that means per-statement facts and diagnostics, because index update plus
  diagnostics are 58% of a keystroke on the big file and parsing is 30%.
- **Recommendation:** statement-level splicing, no new parser technology.
  Keep the edit ranges that `Workspace::change_doc` already computes and now
  throws away. Re-lex from one token before the edit until the new tokens
  meet the old ones again; this is exact because the lexer has no state
  between tokens. Re-parse from one statement before the edit until a
  statement start that lines up with an old one. Shift the spans after it,
  and fall back to a full parse on directives, labelled blank nodes, TriG
  braces, JSON-LD and SPARQL. Then make the knowledge index and the
  diagnostics take the statement delta. Prove it with a chaos-test oracle:
  the incremental `Parsed` must equal a full parse of the same text.
- **Estimated gain** on the 2.4 MB vocabulary: the parser and line table
  alone take a 17 ms keystroke to about 12 to 13 ms. With the index delta
  and per-statement diagnostics it could reach about 5 to 7 ms. Normal
  files (4 to 5 ms today) gain little. Do the index delta first: it is the
  cheapest and lowest-risk part, and it does not need the incremental
  parser.

## What turtle-lsp does today

- The server asks for incremental sync (`TextDocumentSyncKind::INCREMENTAL`,
  `src/server.rs:423`). `Workspace::change_doc` (`src/workspace.rs:820`)
  applies each ranged change to the text and keeps a `LineStarts` current
  through the batch with `LineStarts::replace` (`src/analysis.rs:671`). Then
  it sets `doc.analysis = None` (`src/workspace.rs:854`), so the edit ranges
  and the moved line starts are lost.
- The next analysis is `Analysis::with_loader` (`src/analysis.rs:164`). It
  calls `syntax::parse_with_base` on the whole text. The lexer is
  `tokenize` (`src/syntax/lexer.rs:139`), and the resilient recursive-descent
  parser is `P` (`src/syntax/parser.rs:240`). It then builds `defs`,
  `refs_by_iri`, `labels`, `descriptions`, `types` and `supers` in one pass
  over `parsed.refs` and `parsed.triples`, copies the text into an
  `Arc<str>`, and builds `LineIndex::new(text)` (`src/analysis.rs:414`).
- `Knowledge::set` (`src/knowledge.rs:1330`) computes `DocFacts::new`
  (`src/knowledge.rs:549`), one pass over every triple. `Knowledge::put`
  (`src/knowledge.rs:1423`) then applies the whole new version and releases
  the whole old one. Each step toggles posting lists for every subject,
  object, predicate, `refs_by_iri` key and `defs` key of the document. Some
  diffing already exists: `touched_iris` (`src/knowledge.rs:1145`) merges
  the old and new `curated` summaries. The test
  `incremental_updates_match_a_rebuild` (`src/knowledge.rs:2276`) already
  compares updates against a rebuild.
- Diagnostics are built by `diagnose::document` (`src/diagnose.rs:64`), which
  calls `features::diagnostics::diagnostics_with`
  (`src/features/diagnostics.rs:237`), `curation::diagnostics`
  (`src/features/curation.rs:48`) and the other checks over the whole
  document.

Profile of typing in a 2.4 MB vocabulary (17,000 classes), 2026-10-29:

| Step | Share of a keystroke | About (of 17 ms p50) |
|---|---|---|
| Lexing and parsing | 30% | 5.1 ms |
| Line table (`LineIndex::new`) | 5% | 0.9 ms |
| Rest of `Analysis` | 7% | 1.2 ms |
| Index update (`DocFacts::new` plus `Knowledge::put`) | 27% | 4.6 ms |
| Diagnostics | 31% | 5.3 ms |

Normal files take 4 to 5 ms per keystroke. The roadmap budget is "p95
keystroke → diagnostics ≤ 12 ms" on the 30k-line file
(`docs/specs/sota-roadmap.md:354`).

## How other tools do it

| Tool | Re-parse on edit | Incremental downstream |
|---|---|---|
| rust-analyzer | Full parse in the `parse` query. An incremental `reparse` exists but is used by typing helpers and tests. | salsa queries with backdating; `ItemTree` is a summary that survives edits inside function bodies |
| tree-sitter | Incremental: `ts_tree_edit`, then `ts_parser_parse` with the old tree reuses unchanged subtrees | None (a parsing library); `ts_tree_get_changed_ranges` reports what changed |
| TypeScript ≤ 6 (`tsserver`) | Incremental: `IncrementalParser` reuses whole statements and list elements | Program and checker rebuilt per change |
| TypeScript 7 (Go port) | Full parse: the reuse points are left as `!!! incremental parsing` comments | |
| Roslyn (C#) | Incremental: the `Blender` feeds old nodes and new tokens to the parser | Red-green trees |
| IntelliJ | Incremental for element types marked reparseable (blocks, leaves), otherwise a full parse diffed against the old tree | PSI identity kept by the tree diff |
| vscode-json-languageservice | Full parse, cached by document version | None |
| yaml-language-server | Full parse, cached by document version | None |
| Stardog `turtle-language-server` (millan) | Full parse on every content change | None |
| Mentor (VS Code) | Full re-tokenize 300 ms after the last edit | Triples reloaded |
| Qlue-ls (SPARQL, rowan) | Full parse, cached by version | None |
| zed-rdf `rdf-lsp` | Full parse, despite the module doc | None |
| tree-sitter-turtle | Grammar only; incremental through tree-sitter | None (no prefix resolution) |

### rust-analyzer

- `crates/syntax/src/parsing/reparsing.rs` has two strategies, stated in its
  module doc: "if the edit modifies only a single token ... we replace only
  this token", otherwise "search for the nearest `{}` block which contains
  the edit and try to parse only this block"
  ([reparsing.rs#L1-L7](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/parsing/reparsing.rs#L1-L7)).
- `reparse_token` gives up when a whitespace or comment edit deletes a
  newline, when the token's kind changes, or when the next character would
  join the edited token
  ([reparsing.rs#L46-L95](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/parsing/reparsing.rs#L46-L95)).
  `reparse_block` re-lexes only the block's new text and gives up unless
  the braces are balanced
  ([reparsing.rs#L97-L162](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/parsing/reparsing.rs#L97-L162)).
  `merge_errors` keeps old errors before the block, shifts those after it
  by the length change, and adds the block's new errors
  ([reparsing.rs#L164-L188](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/parsing/reparsing.rs#L164-L188)).
  That is the same splice turtle-lsp would need for `Parsed::errors`.
- `SourceFile::reparse` tries the incremental path and falls back to
  `full_reparse`
  ([lib.rs#L153-L156](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/lib.rs#L153-L156)).
  But the salsa query that every IDE feature reads parses the whole text
  ([editioned_file_id.rs#L22-L28](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/base-db/src/editioned_file_id.rs#L22-L28)).
  `reparse` is called from on-typing assists
  ([typing.rs#L145](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/ide/src/typing.rs#L145)).
- The maintainers' syntax guide: "In practice, incremental reparsing doesn't
  actually matter much for IDE use-cases, parsing from scratch seems to be
  fast enough"
  ([syntax.md#L518-L524](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/docs/book/src/contributing/syntax.md#L518-L524)).
- Equivalence is fuzzed: `CheckReparse::run` re-parses incrementally,
  parses the edited text from scratch, and asserts that every node has the
  same kind and range
  ([fuzz.rs#L50-L72](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/fuzz.rs#L50-L72)).
  The comparison of errors is commented out (line 71), so errors are not
  checked. Every unit test also compares against a full parse
  (`do_check`, [reparsing.rs#L201-L228](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/crates/syntax/src/parsing/reparsing.rs#L201-L228)).
- Downstream, the incremental work happens after parsing. "`ItemTree`
  condenses a single `SyntaxTree` into a 'summary' data structure, which is
  stable over modifications to function bodies", and "typing inside a
  function's body never invalidates global derived data"
  ([architecture.md#L165-L175](https://github.com/rust-lang/rust-analyzer/blob/c59cdaa4c26ffe036a3403e4948f1681995d4125/docs/book/src/contributing/architecture.md#L165-L175)).
- salsa re-runs a query whose input changed, compares the result with the
  previous one, and if they are equal, "backdates" it so that dependants do
  not re-run
  ([algorithm.md#L45-L60](https://github.com/salsa-rs/salsa/blob/840d5cd747347b4808cb8d047559b13e964b6fb3/book/src/reference/algorithm.md#L45-L60),
  [backdate.md](https://github.com/salsa-rs/salsa/blob/840d5cd747347b4808cb8d047559b13e964b6fb3/book/src/plumbing/terminology/backdate.md)).

### tree-sitter

- The library aims to be "fast enough to parse on every keystroke in a text
  editor"
  ([README.md#L7-L10](https://github.com/tree-sitter/tree-sitter/blob/ad067fddf76a32c3071f8f3a31e8719d348e2737/README.md#L7-L10)).
- The caller describes each edit with `ts_tree_edit`, in bytes and in
  (row, column) points
  ([api.h#L454-L464](https://github.com/tree-sitter/tree-sitter/blob/ad067fddf76a32c3071f8f3a31e8719d348e2737/lib/include/tree_sitter/api.h#L454-L464)).
  It then passes the old tree to `ts_parser_parse` "so that the unchanged
  parts of it can be reused"
  ([api.h#L283-L291](https://github.com/tree-sitter/tree-sitter/blob/ad067fddf76a32c3071f8f3a31e8719d348e2737/lib/include/tree_sitter/api.h#L283-L291)).
- Each subtree records `lookahead_bytes`, meaning how far past its end the
  lexer looked. An edit marks a node as changed when it falls inside the
  node or its lookahead
  ([subtree.c#L746-L747](https://github.com/tree-sitter/tree-sitter/blob/ad067fddf76a32c3071f8f3a31e8719d348e2737/lib/src/subtree.c#L746-L747)).
- `ts_parser__reuse_node` refuses a node that has changes, is an error, is
  missing, is "fragile" (built while the parse was ambiguous), or follows a
  different external-scanner state
  ([parser.c#L753-L814](https://github.com/tree-sitter/tree-sitter/blob/ad067fddf76a32c3071f8f3a31e8719d348e2737/lib/src/parser.c#L753-L814)).
- `ts_tree_get_changed_ranges` returns the ranges whose ancestry changed,
  for downstream consumers
  ([api.h#L466-L487](https://github.com/tree-sitter/tree-sitter/blob/ad067fddf76a32c3071f8f3a31e8719d348e2737/lib/include/tree_sitter/api.h#L466-L487)).

### TypeScript

- `IncrementalParser.updateSourceFile` does a full parse when the old file
  has no statements
  ([parser.ts#L9956-L9960](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L9956-L9960)).
  It works in place on the old tree, so "the original source file is
  considered unusable from that point onwards"
  ([#L9962-L9969](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L9962-L9969)).
  It shifts every node after the edit by the length change, then runs the
  normal parser with a syntax cursor over the old tree
  ([#L9988-L10022](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L9988-L10022)).
- `extendToAffectedRange` widens the edit by one token to the left, with
  `maxLookahead = 1`. The example is `/;` becoming `//;`, which turns the
  rest of the line into a comment
  ([#L10308-L10337](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L10308-L10337)).
- `currentNode` refuses a node that is missing, intersects the change or
  contains a parse error ("so that we produce the same set of errors
  again"). It also refuses a node parsed under different context flags,
  for example after `"use strict"` is added above it
  ([#L3125-L3176](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L3125-L3176)).
  This is the closest analogue of an `@prefix` change in Turtle. Only some
  list contexts are reusable: statements, class members, switch clauses,
  enum members, parameters
  ([#L3203-L3240](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L3203-L3240)).
- `aggressiveChecks` re-validates node positions and text during the update
  ([#L9947-L9950](https://github.com/microsoft/TypeScript/blob/v6.0.3/src/compiler/parser.ts#L9947-L9950)).
- TypeScript 7.0.2 (2026-08-20) is the Go port
  ([release](https://github.com/microsoft/TypeScript/releases/tag/v7.0.2)).
  Its parser leaves the reuse point as a `!!! incremental parsing` comment
  ([parser.go#L1133-L1137](https://github.com/microsoft/typescript-go/blob/89d5d5b2849a0db0957065889ca58536fa6d2e4a/internal/parser/parser.go#L1133-L1137)),
  and its parser package has no syntax cursor. So the team shipped a full
  re-parse in its fastest version.

### Roslyn

- Eric Lippert, on the Roslyn design: "we cannot be re-parsing huge wodges of
  text every time you hit a key". The green tree "tracks its width but not
  its absolute position", and an edit rebuilds "typically about O(log n)"
  of its nodes
  ([red-green trees](https://ericlippert.com/2012/06/08/red-green-trees/)).
- The `Blender` collapses several changes into one range
  (`TextChangeRange.Collapse`) and widens it by `maxLookahead = 1` token
  ([Blender.cs#L56-L130](https://github.com/dotnet/roslyn/blob/25bfe90ae370025809a7d156222f804072f26e7a/src/Compilers/CSharp/Portable/Parser/Blender.cs#L56-L130)).
  turtle-lsp needs the same collapse, because several `didChange` batches
  can arrive before the next analysis.
- `CanReuse` refuses zero-width nodes, nodes that intersect the change,
  nodes with diagnostics or skipped text, fabricated or missing tokens, and
  incomplete nodes. A node with preprocessor directives is reused only if
  the directive state before it is "incrementally equivalent"
  ([Blender.Reader.cs#L215-L275](https://github.com/dotnet/roslyn/blob/25bfe90ae370025809a7d156222f804072f26e7a/src/Compilers/CSharp/Portable/Parser/Blender.Reader.cs#L215-L275)).

### IntelliJ

- `BlockSupportImpl.reparse` finds the deepest node whose element type is
  `IReparseableElementTypeBase` or `IReparseableLeafElementType` and
  re-parses only that node. If none qualifies, it calls `makeFullParse`.
  Either way, `mergeTrees` diffs the new subtree against the old one so
  that PSI elements keep their identity
  ([BlockSupportImpl.java#L108-L124](https://github.com/JetBrains/intellij-community/blob/e95ea95eaee586f11deb06b65818b974da3fe8fd/platform/core-impl/src/com/intellij/psi/impl/BlockSupportImpl.java#L108-L124)).
- The highlighting lexer "can be invoked incrementally to process only the
  file's changed part". To restart in the middle of a file it needs a state
  held in one integer
  ([implementing_lexer.md#L27-L36](https://github.com/JetBrains/intellij-sdk-docs/blob/2d04c869622e2f1cfb83217d487a080593cb345c/topics/reference_guide/custom_language_support/implementing_lexer.md#L27-L36)).
  turtle-lsp's lexer has no state between tokens (see below), which makes
  restarting it simpler still.

### JSON and YAML servers

- VS Code's JSON server parses through `getLanguageModelCache`, which
  re-parses when the document version or language id changes
  ([languageModelCache.ts#L35-L43](https://github.com/microsoft/vscode/blob/a48dd23e2ac513740bd36066ae4de756337bba20/extensions/json-language-features/server/src/languageModelCache.ts#L35-L43),
  [jsonServer.ts#L427](https://github.com/microsoft/vscode/blob/a48dd23e2ac513740bd36066ae4de756337bba20/extensions/json-language-features/server/src/jsonServer.ts#L427)).
  `parse` scans the whole text
  ([jsonParser.ts#L1352-L1357](https://github.com/microsoft/vscode-json-languageservice/blob/bf6a8b2c6053a8542c7c053cd518768830fe5e35/src/parser/jsonParser.ts#L1352-L1357)).
  The server still asks for incremental sync
  ([jsonServer.ts#L185](https://github.com/microsoft/vscode/blob/a48dd23e2ac513740bd36066ae4de756337bba20/extensions/json-language-features/server/src/jsonServer.ts#L185)),
  which only saves sending the text.
- yaml-language-server re-parses when the version changes
  ([yaml-documents.ts#L296-L313](https://github.com/redhat-developer/yaml-language-server/blob/7e8061c78564a683956260a645ebebcd7398a181/src/languageservice/parser/yaml-documents.ts#L296-L313)).
- Their shared `TextDocument.update` keeps the line offsets incrementally.
  It splices in the new lines' offsets and shifts the rest by the length
  change
  ([main.ts#L253-L296](https://github.com/microsoft/vscode-languageserver-node/blob/82ed8ac580bcac87229fc128ed0bda1ce83b0bd5/textDocument/src/main.ts#L253-L296)).
  This is what `LineStarts::replace` already does here.

### RDF tooling

- **Stardog `turtle-language-server`** (millan, chevrotain): the base server
  listens to `onDidChangeContent` and calls `parseDocument` on the full text
  ([AbstractLanguageServer.ts#L36](https://github.com/stardog-union/stardog-language-servers/blob/d3e39c4c876ff276da9e36e73f9f6376b36da849/packages/stardog-language-utils/src/AbstractLanguageServer.ts#L36),
  [TurtleLanguageServer.ts#L72-L80](https://github.com/stardog-union/stardog-language-servers/blob/d3e39c4c876ff276da9e36e73f9f6376b36da849/packages/turtle-language-server/src/TurtleLanguageServer.ts#L72-L80)).
- **Mentor** (`faubulous/mentor-vscode`, parsers in
  `@faubulous/mentor-rdf-parsers`): it runs in the extension host, not as a
  language server. Edits are "re-parsed after a short debounce" of 300 ms
  by calling `context.parse(doc.getText())` on the whole document
  ([document-token-source.ts#L8-L19, L45, L167-L199](https://github.com/faubulous/mentor-vscode/blob/1cf9817d5f5e89479c971d4f032bb5f4e715a34a/src/services/document/document-token-source.ts#L8-L199)).
  I did not read `@faubulous/mentor-rdf` or the parsers package itself. The
  call site passes the full text, so no incremental path is in use.
- **zed-rdf `rdf-lsp`**: the module doc says it "only re-parses the changed
  range". The code says "For Phase G the implementation always does a full
  re-parse", and `update_incremental` calls `update_full`
  ([incremental.rs#L1-L72](https://github.com/sparkling/zed-rdf/blob/307bffc4914db075510d40cd62ff6a37dfa23338/crates/rdf-lsp/src/incremental.rs#L1-L72)).
  Its boundary helpers find a statement end with `rfind('.')` and
  `find('.')` on raw text
  ([#L107-L120](https://github.com/sparkling/zed-rdf/blob/307bffc4914db075510d40cd62ff6a37dfa23338/crates/rdf-lsp/src/incremental.rs#L107-L120)),
  which a dot inside an IRI, a string, a decimal or `ex:a.b` breaks. That
  is a warning: statement boundaries must come from tokens, not bytes.
- **Qlue-ls** (SPARQL): keeps one parse, keyed by URI and version, and
  re-parses the whole text on a miss
  ([state.rs#L167-L188](https://github.com/IoannisNezis/Qlue-ls/blob/79817f822b41d0fd46c826b20ed285885f63746f/src/server/state.rs#L167-L188)).
- **tree-sitter-turtle**: the document is `repeat(choice(directive, triple,
  graph))`
  ([grammar.js#L108](https://github.com/GordianDziwis/tree-sitter-turtle/blob/7f789ea7ef765080f71a298fc96b7c957fa24422/grammar.js#L108)),
  so tree-sitter can reuse statements. But the tree does not resolve
  prefixes, so the consumer would still have to find out which statements
  an `@prefix` edit changes. It has no RDF 1.2 syntax
  ([sota-turtle-lsp.md](sota-turtle-lsp.md)).

## How fast a full parse is, and when incremental pays

- The sources give no benchmark numbers. They give judgements instead:
  rust-analyzer finds a from-scratch parse "fast enough", tree-sitter aims
  to parse "on every keystroke", TypeScript 7 shipped without incremental
  parsing, and Roslyn (2012) treated re-parsing "huge wodges of text" per
  keystroke as unaffordable. All the JSON, YAML and RDF servers above
  re-parse in full.
- turtle-lsp's own number: lexing plus parsing 2.4 MB takes about 5 ms
  (30% of 17 ms), about 0.5 GB/s. A typical file of a few hundred KB parses
  in well under a millisecond, which is a small part of a 4 to 5 ms
  keystroke. On the big file, the 5 ms is a real part of the 12 ms p95
  budget.
- So incremental parsing pays only on large files. The pattern in the
  sources is that the downstream work, not the parse, is where incremental
  design pays: rust-analyzer's `ItemTree` and salsa backdating, and
  IntelliJ's tree diff for PSI identity. turtle-lsp's profile agrees:
  index update plus diagnostics are 58% and parsing is 30%.

## Hazards in Turtle, TriG, N-Triples, N-Quads, SPARQL and JSON-LD

1. **Directives.** `@prefix`, `PREFIX`, `@base`, `BASE` and `VERSION`
   change how every later statement reads. The parser keeps `prefixes`,
   `base`, `iri_cache` and `pname_cache` as state
   (`src/syntax/parser.rs:240-290`). This is TypeScript's context-flags
   case and Roslyn's directive case. Rule: if a directive token appears in
   the old or the new window, do a full parse. Directives cluster at the
   top of a file and are edited rarely.
2. **Tokens that run to the end of the file.** Typing `"""`, `"`, `<` or
   `#` can turn everything up to the next matching quote into one token.
   Deleting one can split a token into many. The token-level re-sync below
   handles this exactly: it keeps lexing until the new tokens meet the old
   ones. When they never meet, the re-lex reaches the end of the file and
   the result is a full re-lex.
3. **Lexer lookahead.** A token can depend on bytes after its end. A `.`
   followed by a digit is a number (`src/syntax/lexer.rs:187-195`), a
   prefixed name scans a run of dots and backs off
   (`src/syntax/lexer.rs:434-470`), and `)>>`, `^^`, `{|` and `|}` read
   ahead. So typing `5` after `1.` changes the token before the edit. Rule:
   start re-lexing at the token before the edit, as TypeScript and Roslyn
   do with `maxLookahead = 1`.
4. **Error recovery crosses statements.** After an error the parser skips
   to the next `.`, or to the next statement starter that is at a line
   start (`P::recover`, `src/syntax/parser.rs:576`; `at_line_start`,
   `src/syntax/parser.rs:434`). A broken statement can therefore swallow
   the statements after it, and an edit at column 0 can move a boundary.
   Rule: re-parse from one statement before the edit, and stop only at a
   statement start that lines up with an old statement start past the edit
   (parser re-sync, below).
5. **Blank nodes, property lists and collections across lines.** `[ ... ]`
   and `( ... )` stay inside one statement, so a statement-level unit
   covers them. The numbering is the problem. Fresh blank nodes and
   `_:label` share one per-document counter, and labels map through
   `bnode_labels` (`src/syntax/parser.rs:509-525`). A window that adds one
   `[ ]` shifts the `BlankId` of every later blank node by one. If only
   fresh nodes are involved, the shift is a constant, applied like the
   span shift. If the old or new window contains a `_:label`, the first
   occurrence of a label can move, and the shift stops being constant.
   Rule: full parse.
6. **TriG graph blocks.** `ex:g { ... }` can hold the whole file. The
   parser state inside a block (`graph`, `in_graph`, `graph_indent`) also
   changes recovery. Statements inside a block are units, with the block's
   state taken from `Parsed::graphs`. If a `{`, `}` or `GRAPH` token
   appears in the old or new window, do a full parse.
7. **N-Triples and N-Quads.** One statement per line makes this the easy
   case. The N-Triples restriction errors are added after parsing,
   per statement (`src/syntax/parser.rs:13-21`), so they can be recomputed
   for the window.
8. **Error order.** `Parsed::errors` keeps "the lexer's first, then the
   parser's, then the N-Triples restrictions"
   (`src/syntax/parser.rs:22-26`). A splice has to splice each of the three
   groups separately to match a full parse. Otherwise the editor's
   first-1000 cap (`MAX_SYNTAX_ERRORS`) would pick different errors.
9. **SPARQL.** A query is one statement with nested groups. Reusing the
   prologue barely helps, and query files are small. Qlue-ls and
   TypeScript 7 both re-parse in full. Rule: always a full parse.
   (Embedded SHACL-SPARQL queries are cached per analysis and are not
   affected.)
10. **JSON-LD.** An edit to `@context` changes how every key reads, and
    contexts can be remote. The frontend is separate
    (`crate::jsonld::parse_document_with`). Rule: always a full parse.
    JSON-LD files are not the slow case here.

## Design options

### A. Statement-level splice on the existing parser (recommended)

Find the statements the edit touches, re-lex and re-parse only those, check
that the old and new parses meet again at the end, splice, and fall back
to a full parse on any doubt. It keeps `Parsed`, every feature and the
resilient parser unchanged. The new code is one module and the oracle
tests for it.

### B. Token-level incremental lexing only

Re-lex the window, splice `tokens`, and re-parse the whole token stream.
This is IntelliJ's highlighting lexer. It saves only the lexing part of
the 30%. The profile does not separate lexing from parsing, so measure
that first. It is a useful first step inside option A: the token re-sync
is the same code.

### C. Adopt rowan (red-green trees)

rowan gives relative offsets, so no span shift after the edit, and node
reuse. But every feature reads `Parsed`'s flat vectors (`refs`, `triples`,
`statements`, `prefix_uses`), not a tree. Porting them is a rewrite of the
crate. rust-analyzer, the main user of rowan, does not use its incremental
re-parse for analysis. Not worth it.

### D. Adopt tree-sitter

You get incremental parsing, but you give up turtle-lsp's own resilient
recovery and its error messages, its N-Triples and N-Quads checks and
RDF 1.2. The existing grammar has no RDF 1.2. Prefix resolution, IRI
interning and triple building would still be a second pass over the tree.
tree-sitter's per-node work (stack versions, lookahead bookkeeping) is
also likely to make a full parse slower than the current hand-written
lexer. Not worth it.

## Recommended design

### 1. Keep the edits

`Workspace::change_doc` already turns each change into a byte range and
keeps `LineStarts` current. Store both on the `Doc` until the next
analysis: a list of `(old_range, new_len)`, plus the moved `LineStarts`.
Several batches can arrive before the analysis runs. Collapse them into one
range in the coordinates of the last analysed text, as Roslyn's
`TextChangeRange.Collapse` does. A full-text change, or an edit the
collapse cannot express, clears the list and forces a full parse.

### 2. Re-lex until the tokens meet again (exact)

The lexer keeps no state between tokens. `Lexer::run` skips whitespace and
picks the next token from the current byte alone
(`src/syntax/lexer.rs:171-214`). So when the old and new lexers start a
token at the same position in the unchanged text after the edit, every
token from there on is identical. Algorithm:

1. Find the old token containing the collapsed edit's start, and step back
   one more token for lookahead (hazard 3). Re-lex the new text from that
   token's start.
2. Stop at the first new token that starts at or after the edit's new end
   and has an old token starting at `start - delta`. Everything after that
   is the old tokens shifted by `delta`.
3. If no such token exists before the end of the file, the window runs to
   the end. That is correct, just not cheap: an opened `"""` does this.

### 3. Re-parse whole statements until they meet again

1. Find the statements around the token window with
   `Parsed::statements_around` (`src/syntax/mod.rs:350`). Start one
   statement earlier, because recovery and lookahead can cross statement
   ends (hazard 4).
2. Check the fallback rules. Do a full parse if the old or new window
   holds a directive, a `_:label`, a TriG `{`, `}` or `GRAPH`, or a nesting
   depth near `MAX_NESTING`. Also do a full parse if the syntax is SPARQL or
   JSON-LD, the external prefixes changed, the old parse had no statements,
   or the window covers more than about 25% of the file (a full parse is
   then as cheap and simpler).
3. Rebuild the parser state at the window's start: the prefixes and base in
   effect (from `Parsed::prefixes` and `base` before that offset), the
   graph block state (from `Parsed::graphs`), `bnodes` set to the number of
   blank nodes allocated before the window, and the statement index. This
   needs a constructor for `P` that takes a starting state; it is the only
   change to the parser.
4. Parse statements until one ends at a token past the lexer's meeting
   point, and the next token is the start of an old statement (shifted).
   If the parser state there differs from the old one (inside or outside a
   graph block, nesting depth), keep parsing, or fall back once past the
   25% limit.
5. Splice into `statements`, `triples`, `refs`, `prefix_uses`, `graphs`,
   `tokens` and each of the three error groups (hazard 8). Shift every span
   after the window by `delta`, shift `Triple::statement` by the change in
   statement count, and shift fresh `BlankId`s by the change in blank
   nodes allocated. This is a linear pass over the tail, but it only adds
   to integers. It should cost well under a millisecond on the 2.4 MB
   file; measure it. Roslyn and rowan avoid the pass with relative
   offsets, but that would change every consumer of `Span`.

### 4. Line table

`LineStarts::replace` already updates the line starts per edit. Pass them
to the analysis instead of calling `LineStarts::new` again inside
`LineIndex::new`. The `marks` (UTF-16 and code-point counts every 64
bytes) hold running totals and sit at fixed byte strides, so an edit
invalidates every mark after it. Two options:

- Recompute the marks from the edit onward. This is cheap because it only
  tallies bytes, but it is still O(tail).
- Better: count per line (scan from the line start, which is cheap on
  normal lines) and keep marks only inside long lines (say over 4 KB, such
  as minified JSON-LD or long N-Triples lines). Then an edit touches only
  the marks of the lines it changed.

### 5. Analysis indexes

`defs`, `refs_by_iri`, `labels`, `descriptions`, `types` and `supers` are
the 7% of the keystroke. They store indexes into `parsed.refs` and
`parsed.triples`, and a splice moves those indexes. Rebuild these maps in
full at first. Later, keep them as per-IRI counts updated from the
window's old and new refs and triples, and store statement-relative
indexes so that the tail does not move.

### 6. Knowledge index: apply the delta, not the whole document

`Knowledge::put` adds the whole new version and releases the whole old one
(`src/knowledge.rs:1423-1458`), so a keystroke touches every IRI of the
document twice. Change `DocFacts` to hold counts per IRI for each
contribution (subject, object, predicate, ref, def, binding). A count that
moves between 0 and 1 toggles the posting list; any other change is free.

- With the splice, the delta is the window's old statements minus its new
  ones.
- Without the splice (step 1 of the plan below), get the delta by a sorted
  merge of the old and new `DocFacts` key lists. `touched_iris` already
  does this for `curated`. This still reads the whole document, but it
  replaces two hash-map toggles per IRI with one comparison.

Two facts are per subject, not per statement. `curated` is summarised per
run of triples about one subject (`src/knowledge.rs:556-575`), and a
subject can have several statements. For those, recompute only the
subjects named in the changed statements, from all their statements
(`Analysis::defs` gives the spans).

The `Keyed` label, definition and comment entries carry triple indexes;
make them statement-relative. With nothing changed, skip `put`
altogether, as salsa backdates: for example after typing inside a
comment, the facts are equal.

### 7. Diagnostics per statement

Split the checks by what they read:

- **Statement-local:** syntax errors, `undeclared-prefix` (given unchanged
  prefixes), literal checks, `label-whitespace`. Cache them per statement,
  as spans, and shift them with the statement. Recompute them only for the
  window.
- **Document-wide:** `duplicate-prefix`, `redefined-prefix`, unused
  prefixes, `multiple-labels` (per subject across statements). Recompute
  them in full, or, for per-subject checks, for the subjects in the
  window.
- **Workspace-dependent:** vocabulary terms, `duplicate-label`,
  deprecations, the house-style checks. These read `Knowledge`, which
  changes on every keystroke. Reuse a cached result unless it names an IRI
  or label key whose index entry changed. `note_terms` and `note_texts`
  already collect those keys (`src/knowledge.rs:1145-1262`); expose them
  as the change set of a `put`.

Convert spans to LSP ranges only when answering; the conversion stays
O(number of diagnostics). The LSP pull answer is still the full list, so
encoding it on the wire stays O(n).

### 8. Prove equivalence

- **Oracle:** the incrementally updated `Parsed` must be equal, field by
  field, to `parse_with_base` on the new text. This requires `PartialEq` on
  `Parsed` and its parts (tests only, if preferred). Also, the
  incrementally updated `DocFacts` and index must equal a rebuild, which
  extends `incremental_updates_match_a_rebuild`, and per-statement
  diagnostics must equal `diagnose::document` on a fresh analysis. This is
  rust-analyzer's `CheckReparse` (which skips errors; this oracle must not)
  and TypeScript's `aggressiveChecks`.
- **Chaos:** `tests/chaos.rs` already mutates real files with cuts, stray
  quotes, unbalanced brackets, CRLF and lone CR, and multi-byte characters.
  Apply each mutation as an edit to the previous text, as a client would.
  Chain several edits before one analysis, to exercise the collapse in
  step 1. Run the oracle on every iteration. Add mutations aimed at the
  hazards: insert or delete `"""`, `#`, `<`, `@prefix`, `_:b`, `{`/`}`,
  `.` after a number, and a newline before a column-0 subject. Record
  which path each iteration took (splice or which fallback), so that the
  test fails if the splice path stops being exercised.
- **In the server:** an environment switch that runs both paths and logs
  the first difference (as `first_difference` in `tests/chaos.rs` does) on
  real editing sessions. Keep it off by default.
- **Fallback is the safe default.** Every rule above that says "full
  parse" is cheap to test and costs only speed. Start with strict rules and
  relax them one at a time, each with the oracle.

## Estimate, cost and risk

| Step | Work today (big file) | After | Saving |
|---|---|---|---|
| Lex and parse | 5.1 ms | window parse plus tail shift, about 0.5 to 1 ms | about 4 ms |
| Line table | 0.9 ms | per-line, about 0.1 ms | about 0.8 ms |
| Analysis indexes | 1.2 ms | unchanged at first | 0 |
| Index update | 4.6 ms | delta, about 0.5 to 1 ms | about 3.5 to 4 ms |
| Diagnostics | 5.3 ms | cached local and workspace checks, about 1.5 to 2.5 ms | about 3 ms |

- Parser and line table alone: about 17 ms to 12 to 13 ms on the big file.
  With the index delta and diagnostics caching: about 5 to 7 ms. These are
  estimates from the shares in the profile, not measurements. The tail
  shift and the per-IRI counts are the unknowns to measure first.
- Normal files (4 to 5 ms) gain about a millisecond at most; their parse is
  already small.
- **Cost:** the splice module (re-lex sync, parser start state, splice and
  shift, fallback rules), roughly a few hundred lines plus the oracle
  tests. `DocFacts` with counts and the delta `put` are a medium change to
  `knowledge.rs`. The diagnostics cache is the largest change, because
  every check has to state what it reads.
- **Risk:** a wrong splice gives stale or misplaced diagnostics, which is
  worse than a slow keystroke. The oracle in chaos, plus strict fallback
  rules, keeps it contained. The other risk is that the tail shift and
  the bookkeeping eat most of the parse saving on files where most edits
  are near the top. The 25% rule and measurement guard against that.
  The blank-node rule may fall back often on files that use `_:` labels
  heavily; check how many corpus files do.

## What to do first

1. **Measure.** Split the 30% into lexing and parsing, and the 27% into
   `DocFacts::new` and `Knowledge::put`. Measure a pass that shifts every
   span of the big file. This decides the order of the next two steps.
2. **Index delta without the new parser:** `DocFacts` with per-IRI counts
   and a sorted-merge delta in `put`, checked by
   `incremental_updates_match_a_rebuild`. Low risk, independent of the
   parser, and it targets the second-largest share.
3. **Keep the edits and the line starts** (design step 1 and the line
   table). Small, and a prerequisite for the rest.
4. **Statement splice with the oracle in chaos**, with every fallback rule
   on, behind a setting until chaos and the corpus pass.
5. **Diagnostics per statement,** starting with the statement-local
   checks.

## Not checked

- I did not read `@faubulous/mentor-rdf` or `@faubulous/mentor-rdf-parsers`
  internals; the claim about Mentor rests on its call site.
- None of the sources publish full-parse throughput numbers. The speed
  figures here are turtle-lsp's own.
- The JetBrains SDK docs describe the incremental highlighting lexer but
  not `IReparseableElementType`. The IntelliJ claim rests on
  `BlockSupportImpl.java`.
