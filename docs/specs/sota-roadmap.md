# Spec: turtle-lsp from "fast and correct" to state of the art

Status: done in 0.2.0 · 2026-10-13. Its tickets were kept in a local folder (`.scratch/`) that is not part of the repository.

## Problem Statement

turtle-lsp is now fast (≈9 ms keystroke → diagnostics on 30k lines) and at
feature parity with the old Node server, but it still only *reads* Turtle. A
person who writes ontologies, SHACL shapes and instance data every day still has
to:

- format files by hand, and argue about layout in reviews;
- read opaque identifiers (`ex:P0042`, `obj:a81f…`) without knowing what they
  denote;
- see every IRI in the same colour, so a class, a property, an individual, a
  misspelt local name and a deprecated term look identical;
- find out only in CI (or never) that instance data breaks the SHACL shapes
  sitting next to it in the same repository, or that a shape is itself broken
  (no `sh:path`, a `sh:targetClass` nobody defines, `sh:minCount 2` with
  `sh:maxCount 1`);
- guess which properties make sense for a subject, although the ontology's
  `rdfs:domain`/`rdfs:range` and the shapes say exactly that;
- notice typos in their *own* namespace (`sd:fillsPositon`) by eye, because the
  vocabulary check only covers well-known vocabularies;
- browse class hierarchies with grep;
- copy imported ontologies into the workspace by hand to get hover and
  completion for them;
- give up on TriG and N-Triples files entirely;
- live with an "add prefix" quick fix that proposes the W3C namespace for `sd:`
  while every other file in the project binds `sd:` to the project namespace.

Under the hood, memory is higher than it needs to be (≈230 MB on the benchmark
workspace), the server always pushes diagnostics even to clients that prefer to
pull them, and a burst of requests can't be cancelled.

## Solution

One release that turns the server into a full editing environment for RDF
projects, without giving back any speed:

- **Formatting** that is idempotent and never changes the graph or loses a
  comment.
- **Semantic tokens** that colour IRIs by what they denote, and mark deprecated
  and undescribed terms.
- **Inlay hints** that show the human label after opaque IRIs.
- **Refactorings**: organise prefixes, shorten/expand an IRI, extract a prefix
  for a repeated namespace.
- **Ontology-aware completion**: properties whose domain (or whose SHACL shape)
  fits the subject come first; objects whose class fits the property's range
  come first; `sh:in` values are offered directly.
- **Type hierarchy** for classes and properties.
- **SHACL**: shape lint as you type; full validation of the workspace's data
  against its shapes on save, in the background, with violations shown on the
  offending triple.
- **Workspace lint**: terms in your own namespaces that are used but described
  nowhere, with "did you mean" fixes.
- **`owl:imports`**: imported ontologies are loaded from the workspace, a local
  catalog or a cache (network fetching only when you opt in) and feed hover,
  completion and checks.
- **Conformance**: the W3C Turtle (and TriG / N-Triples) test suites run in CI.
- **Leaner memory** through interned IRIs.
- **Protocol**: pull diagnostics when the client supports them, request
  cancellation, and TriG and N-Triples documents.
- **Loose ends**: the add-prefix fix prefers the project's own binding; unused
  test dependency removed.

Typing latency stays at the current budget (p95 keystroke → diagnostics ≤ 12 ms
on the 30k-line benchmark), verified by `npm run bench` after every ticket.

## User Stories

Formatting
1. As an ontology author, I want to format a whole Turtle file with my editor's format command, so that layout is consistent without manual work.
2. As an ontology author, I want formatting to keep every comment exactly where it belongs (before or after the statement it annotates), so that documentation in comments is never lost.
3. As an ontology author, I want formatting to keep statement order, so that diffs stay reviewable and my narrative order survives.
4. As a reviewer, I want formatting to be idempotent, so that formatting an already formatted file changes nothing and CI checks can rely on it.
5. As an ontology author, I want the formatter never to change the graph (same triples before and after), so that I can format without fear.
6. As an ontology author, I want literals and IRIs written exactly as I wrote them (no re-escaping, no shortening), so that formatting is not a hidden refactoring.
7. As an ontology author, I want the formatter to use my editor's indent settings (tab size, spaces vs tabs), so that it matches my project.
8. As an ontology author, I want each predicate on its own line under the subject and short object lists on one line, so that files read like the Turtle examples in the specs.
9. As an ontology author, I want short blank-node property lists and collections kept inline and long ones broken over lines, so that nesting stays readable.
10. As an ontology author, I want the prefix block kept together at the top with aligned IRIs, so that the header is easy to scan.
11. As an ontology author, I want at most one blank line between statements, so that files don't drift into random spacing.
12. As an ontology author, I want formatting to refuse (make no change) on a file with syntax errors, so that a half-typed statement is never mangled.
13. As an author of RDF 1.2 data, I want triple terms, reified triples and annotations formatted correctly, so that new syntax is first class.
14. As an author of TriG files, I want graph blocks formatted with their contents indented, so that TriG gets the same treatment.

Semantic highlighting
15. As an ontology author, I want classes, properties and individuals coloured differently, so that I can read a statement's structure at a glance.
16. As an ontology author, I want prefixes coloured as namespaces, so that the prefix and local part are visually distinct.
17. As an ontology author, I want deprecated terms rendered as deprecated (e.g. strikethrough), so that I stop using them.
18. As an ontology author, I want terms of my own namespace that are described nowhere marked distinctly, so that typos stand out before I read the diagnostics.
19. As an ontology author, I want terms defined in this file marked as declarations, so that I can tell definitions from uses.
20. As an ontology author, I want terms from well-known vocabularies marked as library terms, so that I can tell my terms from borrowed ones.
21. As a Neovim user, I want the semantic token types to use standard LSP names, so that my colour scheme highlights them without extra configuration.

Inlay hints
22. As a data engineer, I want the label of an opaque IRI (`ex:P0042`) shown next to it, so that I know what it denotes without hovering.
23. As a data engineer, I want no hint when the local name already says the same as the label (`sd:Asset` / "Asset"), so that hints don't clutter readable code.
24. As a data engineer, I want to switch label hints off, or show them for every labelled term, so that I can pick the density I like.
25. As a data engineer, I want hints to pick up labels from anywhere in the workspace and from known vocabularies, so that a term labelled in another file is still explained.

Refactorings
26. As an ontology author, I want an "organise prefixes" action that sorts the prefix block and removes unused and duplicate declarations, so that headers stay tidy.
27. As an ontology author, I want to shorten `<http://xmlns.com/foaf/0.1/Person>` to `foaf:Person` when a prefix is in scope, so that files stay readable.
28. As an ontology author, I want to shorten every such IRI in the file at once, so that I can clean up pasted data.
29. As an ontology author, I want to expand `foaf:Person` to its full IRI, so that I can copy it to tools that don't know my prefixes.
30. As an ontology author, I want an "extract prefix" action on an IRI with no matching prefix, which adds a declaration with a sensible name and rewrites every IRI in that namespace in the file, so that pasted data becomes idiomatic in one step.
31. As an ontology author, I want refactorings never offered when they'd change meaning (for example, sorting prefixes that are redefined midway), so that I can trust them.

Ontology-aware completion
32. As a data author, I want properties whose `rdfs:domain` matches the subject's class (or a superclass) listed first, so that completion knows my ontology.
33. As a data author, I want properties required by a SHACL shape that targets the subject's class listed first and marked as required, so that I fill in what validation will demand.
34. As a data author, I want, after a property whose `rdfs:range` (or `sh:class`) is a class, the instances of that class (and its subclasses) listed first, so that I link to the right things.
35. As a data author, I want the allowed values of an `sh:in` list offered directly as completions, so that enumerations are one keystroke away.
36. As a data author, I want domain/range knowledge from well-known vocabularies too (e.g. `foaf:knows` ranges over `foaf:Person`), so that standard vocabularies help as much as mine.

Type hierarchy
37. As an ontology author, I want to open the type hierarchy on a class and see its superclasses and subclasses, so that I can navigate the taxonomy.
38. As an ontology author, I want the same for properties (`rdfs:subPropertyOf`), so that property hierarchies are navigable too.
39. As an ontology author, I want hierarchy entries to include superclasses from well-known vocabularies (e.g. `sosa:Sensor`), so that the chain doesn't stop at my workspace border.

SHACL
40. As a shapes author, I want an error when a property shape has no `sh:path`, so that broken shapes are caught as I type.
41. As a shapes author, I want a warning when `sh:targetClass` or `sh:class` names a class that is defined nowhere, so that typos in class names are caught.
42. As a shapes author, I want a warning when `sh:datatype` is not a datatype, so that `sh:datatype xsd:strng` or `sh:datatype sd:Asset` is caught.
43. As a shapes author, I want a warning when a node-shape-only parameter is used on a property shape (or the reverse), so that misplaced parameters don't silently do nothing.
44. As a shapes author, I want a warning when `sh:node` or `sh:property` points to a shape that is described nowhere, so that dangling references are caught.
45. As a shapes author, I want an error when `sh:minCount` exceeds `sh:maxCount`, so that unsatisfiable shapes are caught.
46. As a data author, I want my data validated against the workspace's shapes when I save, so that I see violations in the editor, not in CI.
47. As a data author, I want each violation shown on the offending triple (or on the focus node when the value is missing), with the shape's message, so that I know what to fix and where.
48. As a data author, I want to jump from a violation to the shape that raised it, so that I can read the rule.
49. As a data author, I want validation to run in the background and never slow down typing, so that SHACL support has no cost while I edit.
50. As a data author, I want validation results to disappear as soon as I fix and save, and stale results never to overwrite newer ones, so that the editor tells the truth.
51. As a user of a large workspace, I want a setting to disable validation and a size cap above which it is skipped (with a message saying so), so that huge data dumps don't make the editor sluggish.
52. As a data author, I want shapes and data split across many headerless fragments to work, so that my project's layout is supported.

Workspace lint
53. As an ontology author, I want a warning when I use a term in my own namespace that is described nowhere in the workspace, so that typos like `sd:fillsPositon` are caught.
54. As an ontology author, I want a "did you mean" quick fix to the closest described term, so that fixing a typo is one action.
55. As an ontology author, I want terms from imported ontologies and well-known vocabularies not to be flagged, so that the lint has no false positives for borrowed terms.
56. As a data engineer, I want instance namespaces that are mostly referenced (not described) in the workspace not to flood me with warnings, so that the lint stays useful.

Imports
57. As an ontology author, I want `owl:imports` of an ontology that lives in my workspace to just work, so that I don't configure anything for the common case.
58. As an ontology author, I want imports resolved through a local `catalog-v001.xml` (Protégé convention), so that projects that already have a catalog work.
59. As an ontology author, I want imported ontologies fetched once and cached when I opt in to network access, so that hover, completion and lint know them offline afterwards.
60. As a privacy-minded user, I want no network access unless I enable it, so that the editor never phones out by itself.
61. As an ontology author, I want an information diagnostic on an import that can't be resolved, telling me how to resolve it, so that missing knowledge is visible.
62. As an ontology author, I want RDF/XML imports supported, so that classic ontologies published as RDF/XML work.

Correctness, performance, protocol
63. As a maintainer, I want the W3C Turtle, TriG and N-Triples test suites in CI, so that parser conformance is proven, not claimed.
64. As a user on a laptop, I want the server to use about half the memory it uses today on the benchmark workspace, so that it stays a good citizen.
65. As a Neovim/VS Code user, I want pull diagnostics when my editor supports them, so that the editor controls when diagnostics are computed.
66. As a user, I want cancelled requests (e.g. superseded completions) not to be computed, so that the server never works on answers nobody wants.
67. As a user, I want `.trig` files to get the same features as Turtle (diagnostics, completion, navigation, formatting), so that named graphs are supported.
68. As a user, I want `.nt` files checked as N-Triples (no prefixes, no abbreviations), so that I get errors for constructs N-Triples doesn't allow.
69. As a project user, I want the add-prefix quick fix to propose the binding my project already uses for that prefix name (e.g. `sd:` → project namespace) before the well-known one, so that the fix matches the project.
70. As a maintainer, I want unused dev dependencies removed, so that the dependency surface stays minimal.
71. As a user, I want every new feature to keep keystroke latency within budget, so that the server stays the fastest Turtle LSP.

## Implementation Decisions

Architecture (prefactor, lands first)
- **Interned IRIs.** IRIs become a small `Copy` handle backed by a global,
  append-only, thread-safe interner (a proven crate such as `lasso`, or an
  equivalent in-house type if the crate doesn't fit). The handle resolves to a
  `'static str`. All per-document indexes (`defs`, `refs_by_iri`, `labels`,
  `types`, `supers`, …) and the new workspace index key by handle. The
  interner never frees: memory grows only with distinct IRIs ever seen, which
  is bounded in practice. This is a wide refactor, done expand–contract inside
  one ticket because the crate is small and has one owner.
- **Triples carry spans.** Each recorded triple keeps the span of its predicate
  and of its object (and, through the statement, its subject), so diagnostics
  from SHACL and lints can point at the exact triple. Blank-node labels become
  unique per document, and the workspace qualifies them per document when it
  merges graphs.
- **Workspace knowledge index.** A new module owns workspace-wide facts derived
  from every analysis (open documents win over disk; imported ontologies are a
  third, read-only source): for each IRI, where it is described, its types,
  its labels/comments, super- and sub-classes/properties, `rdfs:domain` and
  `rdfs:range`, and the SHACL shapes that target a class with their property
  constraints. It also counts prefix-name → namespace bindings across files.
  The index is updated **incrementally**: replacing a document's analysis
  subtracts its old contributions and adds the new ones, so a keystroke costs
  O(size of that document), not O(workspace). Features that currently loop
  over every analysis (hover, completion, implementation, type definition,
  symbols) query the index instead.
- **Well-known vocabularies gain structure.** The vocabulary generator also
  records each term's `rdfs:domain`, `rdfs:range` and direct super terms. The
  embedded data format grows by these columns; lookup remains lazy per
  vocabulary.
- **Document languages.** An analysis knows its syntax: Turtle, TriG or
  N-Triples (from the language id on open, else the file extension). One parser
  handles all three: TriG adds graph blocks (`{ }`, `GRAPH g { }`, `g { }`);
  N-Triples parses with the Turtle grammar and reports constructs N-Triples
  forbids. Triples get an optional graph name. The workspace scan indexes
  `.ttl .turtle .trig .nt`.

Features (each is a module under the existing features area, each reads one
analysis plus the knowledge index)
- **Formatter.** Works from the token stream and the parse, not from the
  graph: it re-lays-out tokens and never rewrites token text, which is what
  guarantees graph preservation and comment preservation. It returns one edit
  replacing the document, or no edits when the document has syntax errors.
  Layout rules: prefix/base/version directives first in their original order
  with IRIs aligned; one blank line between statements (existing single blank
  lines kept, runs collapsed); subject then predicates indented one level, one
  per line, ` ;` separators, ` .` on the last; object lists on one line when
  they fit in the line width (default 100) else one per line; blank-node
  property lists and collections inline when short, else broken and indented;
  comments keep their attachment (own-line comments stay above the following
  token; trailing comments stay at the end of their line). Indent from the
  client's `FormattingOptions`. Full-document formatting only.
- **Semantic tokens.** Full-document and range requests. Token types use LSP
  standard names: `namespace` (prefix part), `class`, `property`,
  `enumMember` (individuals), `type` (datatypes), `keyword` (`a`, directives).
  Modifiers: `declaration` (top-level subject), `deprecated`,
  `defaultLibrary` (well-known vocabulary), and one custom `undescribed`
  (own-namespace term described nowhere, same rule as the workspace lint).
  Classification order: knowledge-index types (owl/rdfs class, property types,
  `sh:NodeShape`), then vocabulary term kind, then role (predicate position ⇒
  property, object of `rdf:type` ⇒ class). Literals, numbers and comments are
  left to the grammar (TextMate / tree-sitter).
- **Inlay hints.** For refs in the requested range whose IRI has a label
  (workspace or vocabulary) and whose written local name is opaque: a label is
  shown when the normalised local name (lowercased, non-alphanumerics removed,
  camelCase split) is not equal to and not contained in the normalised label.
  Setting `labels: "opaque" | "all" | "off"`, default `"opaque"`. The hint is
  placed after the term, padded left.
- **Refactorings** (code actions of kind `source.organizeImports` and
  `refactor.rewrite`, alongside the existing quick fixes): organise prefixes
  (only on the leading block of directives; comments stay with the directive
  below them; not offered when a prefix is redefined or a `@base` is
  interleaved); shorten IRI at cursor / in whole file (longest matching
  namespace, result must be a valid local name); expand prefixed name at
  cursor; extract prefix for the cursor IRI's namespace (prefix name: the
  well-known prefix for that namespace if any, else derived from the last
  meaningful path segment, made unique) rewriting all IRIs of that namespace in
  the file.
- **Completion ranking.** A new ranking stage before the existing sort. In
  predicate position, the subject's types (from the knowledge index, plus
  `a X` already typed in this statement) expand through superclasses; properties
  with a matching domain, and paths of SHACL property shapes targeting those
  classes, get the top rank. Shape-required properties (`sh:minCount ≥ 1`) show
  "required by <shape>" as detail. In object position, the predicate's range
  (or `sh:class` from a matching shape) selects instances of that class and its
  subclasses; `sh:in` values become candidates. Existing caps (200 items,
  `isIncomplete`) stay.
- **Type hierarchy.** `prepareTypeHierarchy` on an IRI that is a class or
  property; supertypes and subtypes from the knowledge index (workspace,
  imports, vocabulary super terms). Every item needs a location: the term's
  first description, else its first reference in the workspace, with detail
  naming the vocabulary for borrowed terms.
- **SHACL shape lint** (diagnostics with codes `shacl-*`, computed per
  keystroke for the edited document from its triples plus the knowledge index):
  property shape without `sh:path`; `sh:targetClass` / `sh:class` naming a
  class known nowhere (known = vocabulary class, typed as a class, has
  sub/superclasses, or has instances); `sh:datatype` not an XSD/RDF datatype
  nor a declared `rdfs:Datatype`; node-only parameters on property shapes and
  property-only parameters on node shapes; `sh:node` / `sh:property` /
  `sh:qualifiedValueShape` pointing to an IRI described nowhere; `sh:minCount >
  sh:maxCount`. Terms in a well-known namespace that are already flagged as
  unknown are not flagged twice.
- **SHACL validation.** First step of the ticket is a time-boxed evaluation of
  existing Rust engines (rudof's `shacl` crate, `purrdf-shapes`,
  `oxirs-shacl`) against: licence allowed by `deny.toml`, dependency weight
  and build time, ability to take our triples without an extra parse, results
  with focus node / path / value / source shape / message, and pass rate on
  the W3C SHACL Core test suite. If none passes, implement SHACL Core
  ourselves over the knowledge index's graph (property paths including
  sequence/alternative/inverse/zero-or-more/one-or-more/zero-or-one, and the
  core constraint components) and test it against the same suite. The
  decision and the numbers are recorded in `docs/research/`. Execution: on
  `didSave` (and once after the initial index), a background worker validates
  the union of all workspace (and imported) triples as both data and shapes
  graph. A generation counter discards results from superseded runs. Results
  become diagnostics (code `shacl-violation`, severity from `sh:severity`,
  message from `sh:message` or the component) on the triple (focus, first path
  predicate, value) when present, else on the focus node's description, in
  the file where that lies; `relatedInformation` points to the shape. Settings:
  `shacl.enabled` (default true), `shacl.maxTriples` (default 2,000,000; over
  the cap one information diagnostic explains why validation was skipped).
  Workspaces without shapes cost nothing.
- **Workspace lint** (code `undescribed-term`): a ref to IRI *x* is flagged
  when *x*'s namespace is an "own" namespace and *x* is described nowhere
  (workspace, imports). Own namespace: not a well-known vocabulary, and the
  workspace describes at least 3 terms in it and describes at least 80% of the
  distinct terms it references there (keeps reference-heavy instance
  namespaces quiet). Tuned on the private corpus: at 20% it raised 36
  warnings and 555 information diagnostics, at 80% none and 9. Severity:
  warning when a described term within edit distance ≤ 2 exists (≤ 1 for
  local names of 3 to 5 characters, none for shorter ones; same first-letter
  case; not differing only in digits), with a replace quick fix; otherwise information. Refs in
  subject position are descriptions and never flagged, nor are objects of
  ontology links (`owl:imports`, `owl:versionIRI`, `owl:priorVersion`,
  `owl:backwardCompatibleWith`, `owl:incompatibleWith`). The SHACL shape
  lint's "known nowhere" rules use a looser *project* namespace test (≥ 3
  described, ≥ 20% of referenced) so a partly present ontology is still
  checked; that stayed silent on the corpus. Setting
  `lint.undescribedTerms` (default true). Threshold values are tuned on the
  private real-world corpus (must stay near-silent there).
- **Imports.** For `?o owl:imports <iri>`: resolve in this order, stopping at
  the first hit: an ontology described in the workspace (`<iri> a
  owl:Ontology`, or a version IRI); `catalog-v001.xml` files in the workspace
  (Protégé XML catalog `uri name=… uri=…`); the cache directory (per-user
  cache dir, `turtle-lsp/imports/`); network fetch only when
  `imports.network` is true (content negotiation for Turtle, N-Triples and
  RDF/XML; size cap 20 MB; timeout 10 s; result written to the cache).
  Resolution runs on a background thread and transitively (with a depth cap of
  5 and cycle detection). Imported ontologies are parsed (Turtle via our
  parser, RDF/XML via `oxrdfxml`) and added to the knowledge index as
  read-only sources; they are never diagnosed. Unresolved import: information
  diagnostic on the import IRI with the resolution hint. HTTP via a small
  blocking client (`ureq` with rustls), compiled in but idle unless enabled.
- **Pull diagnostics.** When the client advertises
  `textDocument.diagnostic`, the server registers `diagnosticProvider`
  (`interFileDependencies: true`, no workspace diagnostics) and stops pushing
  for open documents; reports carry a `resultId` derived from document version
  plus knowledge generation, so unchanged results answer `unchanged`. When
  knowledge changes in ways that affect other documents (prefixes.ttl, SHACL
  results, imports, described-term set) the server sends
  `workspace/diagnostic/refresh` if supported. Push stays the default for
  other clients.
- **Cancellation.** The main loop collects the whole queued batch before
  answering; a request whose `$/cancelRequest` is in the batch is answered
  with `RequestCancelled` without being computed. Background work (index,
  SHACL, imports) is superseded by generation counters.
- **Settings.** New settings are read from `initializationOptions` and from
  `workspace/didChangeConfiguration` (section `turtle`): `inlayHints.labels`,
  `shacl.enabled`, `shacl.maxTriples`, `lint.undescribedTerms`,
  `imports.network`, `format.lineWidth`. The VS Code extension exposes each
  as `turtle.*` settings (resource scope where safe; `imports.network` is
  machine-overridable but also respects workspace trust: off in untrusted
  workspaces).
- **Server capabilities added**: document formatting, semantic tokens
  (full + range), inlay hints, type hierarchy, `didSave`, pull diagnostics
  (conditional), more code action kinds.
- **Editors.** VS Code: register `trig` and `ntriples` languages with
  grammars (TriG = Turtle + graph blocks; N-Triples = subset), new settings,
  trust handling for `imports.network`. Neovim: the user's config learns the
  `trig` filetype; the README documents semantic-token highlight groups and
  inlay hints.
- **Add-prefix fix.** The undeclared-prefix diagnostic carries every
  candidate namespace: first the project's most common binding for that name
  (from the knowledge index, with "used in N files"), then the well-known one
  if different. The first is the preferred fix.
- **Dependencies.** Remove `n3` from the test package. Every new crate must
  pass `cargo deny check`.

Performance budget (enforced per ticket with `npm run bench`, numbers in the
ticket's commit message)
- keystroke → diagnostics p95 ≤ 12 ms on the 30k-line file (today ≈ 9 ms);
- completion ≤ 5 ms; semantic tokens (full, 30k lines) ≤ 30 ms; inlay hints
  for a 100-line range ≤ 2 ms; format 30k lines ≤ 50 ms;
- RSS on the benchmark workspace ≤ 120 MB after interning (today ≈ 230 MB);
- the bench gains rows for semantic tokens, inlay hints, formatting and RSS.

## Testing Decisions

- **Good tests test behaviour at the highest seam**: requests and
  notifications over stdio against the release binary, asserting what an
  editor would show (diagnostics, edits after applying them, completion
  order, hierarchy items). Tests never reach into internal data structures.
  Unit tests are reserved for pure, property-like behaviour where the stdio
  seam would be slow or indirect.
- **Seams (all existing, none new):**
  1. *LSP stdio* — `test/*.test.mjs` with the shared test client. Every
     feature ticket adds tests here, using neutral fixtures (`example.com`
     namespaces) in `test/fixtures/workspace` and `test/fixtures/project`
     (extended with SHACL shapes, violating data, an imports case with a
     local catalog, a TriG and an N-Triples file).
  2. *Rust library API* — property and corpus tests in `tests/`: the oxttl
     oracle (extended to TriG/N-Triples), formatter properties (idempotent;
     same triples as the input per our parser and per oxttl; same multiset of
     comments) over every fixture including the private corpus when present,
     the W3C conformance runners, the SHACL engine against the W3C SHACL Core
     suite, and the chaos test (extended to call formatting, semantic tokens,
     inlay hints, code actions, type hierarchy and SHACL lint at random
     positions in both encodings).
  3. *Editors* — the headless Neovim spec and the VS Code extension suite
     gain checks that the new capabilities are live in a real client:
     formatting changes the buffer, semantic tokens are produced, inlay hints
     appear, type hierarchy answers, a SHACL violation shows after save,
     `.trig` attaches.
- **Prior art:** `test/lsp.test.mjs` (protocol behaviour), `test/project.test.mjs`
  (headerless fragments + prefixes.ttl), `tests/oxttl_oracle.rs`,
  `tests/chaos.rs`, `test/nvim/spec.lua`, `editors/vscode/test/suite.cjs`,
  `bench/perf.mjs`.
- **Real corpus:** `test/real.test.mjs` asserts the new lints and SHACL stay
  quiet on the private real-world fragments (no new warnings), skipped when
  the corpus is absent.
- **Gates per ticket:** `cargo fmt --check`, `cargo clippy -D warnings`,
  `npm test`, `npm run test:vscode` when the extension or protocol surface
  changes, `npm run bench`, `cargo deny check` when dependencies change.

## Out of Scope

- SHACL-SPARQL (`sh:sparql`) and SHACL Advanced Features (rules, functions).
- ShEx.
- Range formatting and on-type formatting.
- Workspace pull diagnostics (`workspace/diagnostic`) and diagnostics for
  files that aren't open (SHACL results land on open documents; others show
  when opened).
- N-Quads, JSON-LD and RDF/XML *documents* (RDF/XML is supported only as an
  import source).
- OWL reasoning beyond `rdfs:subClassOf` / `rdfs:subPropertyOf` closure
  (no inferred types from domain/range, no equivalence).
- Publishing: no pushes, tags or Marketplace releases as part of this work.
- Deleting the remote `rust-rewrite` branch (needs a push; left to the owner).

## Further Notes

- The knowledge index is the keystone: semantic tokens, inlay hints, ranking,
  hierarchy, both lints, SHACL and imports read it. It and the interning land
  before anything that depends on them; formatter, refactorings, conformance
  and TriG/N-Triples parsing don't need it and can run in parallel with it.
- Neovim 0.11 supports semantic tokens, inlay hints (`vim.lsp.inlay_hint.enable`),
  type hierarchy (`vim.lsp.buf.typehierarchy`), formatting and pull
  diagnostics; VS Code with `vscode-languageclient` 10 supports all of them.
- RDF 1.2 Turtle is a W3C Working Draft (2026-10-07); conformance targets the
  rdf-tests repository's rdf11 and rdf12 Turtle/TriG/N-Triples suites at a
  pinned commit, vendored into the repo so CI is offline and reproducible.
- No tracked file may mention the owner's employer or private project;
  fixtures stay neutral.
