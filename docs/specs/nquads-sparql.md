# N-Quads and SPARQL documents

Status: in progress · 2026-10-19 · Branch: `nquads-sparql`

## Problem Statement

turtle-lsp covers Turtle, TriG and N-Triples. Two formats that the same
people work with every day get nothing:

- **N-Quads** (`.nq`) is the format of dataset dumps and triple-store
  backups (Web Data Commons, Wikidata exports, backups of any quad store). A
  `.nq` file opens as plain text: no highlighting, no syntax errors, no labels
  on hover, and its facts do not reach the rest of the workspace.
- **SPARQL** (`.rq`, `.sparql`, `.ru`) is how ontologies and data are
  queried and updated. Generic SPARQL extensions know the grammar but not the
  project: they cannot complete `ex:` terms from the workspace, say that
  `ex:hasPartt` is described nowhere, show a term's label on hover, or jump
  to where it is defined. The server already parses SPARQL for SHACL-SPARQL
  and already knows every term in the workspace and in 80+ vocabularies;
  none of that reaches a `.rq` file.

## Solution

1. **N-Quads** is a fourth RDF syntax next to Turtle, TriG and N-Triples, with
   everything N-Triples has: syntax errors with recovery, highlighting,
   hover, navigation, outline (grouped by graph), formatting (one quad per
   line), and its facts join the workspace like a TriG file's. It passes the
   W3C N-Quads suites (RDF 1.1 and 1.2).
2. **SPARQL** documents (queries and updates, SPARQL 1.1 and 1.2) get the
   language-server features the RDF documents have, built on the same
   knowledge of the workspace and the vocabularies: syntax errors at the
   right spot, undeclared / unused / duplicate prefixes, unknown and
   deprecated vocabulary terms, completion of keywords, variables, prefixes
   and terms, hover, go to definition, references, document highlight,
   rename (variables, prefixes, and terms across Turtle and SPARQL files),
   formatting, semantic highlighting, folding, outline and quick fixes. It
   passes the W3C SPARQL 1.1 and 1.2 syntax suites.
3. Both are checked against public real-world files in `npm run corpus`, and
   the README badges show the new suite results.

## User Stories

### N-Quads

1. As a data engineer, I want `.nq` files recognised as N-Quads, so that a
   dump opens with highlighting instead of as plain text.
2. As a data engineer, I want syntax errors in an N-Quads file reported per
   line with recovery, so that one bad line in a dump does not hide the rest.
3. As a data engineer, I want a graph label that is not an IRI or blank node
   (a literal, a prefixed name, a relative IRI) reported as an error, so that
   I find quads my store would reject.
4. As a data engineer, I want a fifth term on a line reported as an error,
   so that I find corrupted lines.
5. As a data engineer, I want invalid IRIs, escapes and language tags
   reported exactly as in N-Triples, so that the two formats behave the same.
6. As a data engineer, I want RDF 1.2 triple terms in N-Quads accepted, so
   that RDF 1.2 dumps are not flagged.
7. As a data engineer, I want the outline to group statements by graph, so
   that I can see which graphs a dump holds.
8. As a data engineer, I want hover on an IRI in a `.nq` file to show its
   label and where it is described, so that opaque identifiers make sense.
9. As a data engineer, I want label inlay hints in `.nq` files, so that
   Wikidata-style IRIs are readable.
10. As a data engineer, I want go to definition, references and document
    highlight in `.nq` files, so that I can follow a resource through a dump.
11. As a data engineer, I want formatting to give one quad per line with
    single spaces, keep comments and statement order, and never change the
    dataset, so that dumps diff cleanly.
12. As an ontology author, I want terms described in a `.nq` file to count as
    described in the workspace, so that my Turtle files do not warn about
    them.
13. As an ontology author, I want `.nq` files in the workspace indexed like
    other RDF files, so that completion and hover in Turtle know their terms.
14. As a user, I want no prefix diagnostics or prefix quick fixes in N-Quads,
    so that a format without prefixes is not told to declare them.
15. As a user of another editor, I want the server to treat a document as
    N-Quads when the client sends `nquads` / `n-quads` or the name ends in
    `.nq`, so that it works without editor-specific setup.
16. As a maintainer, I want the W3C N-Quads suites (RDF 1.1 and 1.2) to pass
    in CI, so that conformance is checked on every change.

### SPARQL: recognising documents

17. As a query author, I want `.rq` and `.sparql` files recognised as SPARQL
    queries and `.ru` files as SPARQL updates, so that they open with
    highlighting.
18. As a query author, I want a file that holds an update in a `.rq` file
    (or a query in a `.ru` file) checked as what it is, so that the extension
    does not decide validity.
19. As a user of another editor, I want the server to treat a document as
    SPARQL when the client sends `sparql` or the name ends in `.rq`,
    `.sparql` or `.ru`, so that it works without editor-specific setup.

### SPARQL: diagnostics

20. As a query author, I want syntax errors reported at the token where the
    query stops making sense, with what was expected, so that I can fix them
    without reading the whole query.
21. As a query author, I want syntax errors reported as I type, on the
    unsaved text, so that I never have to run the query to find them.
22. As a query author, I want an undeclared prefix reported on the prefixed
    name, so that I find a missing `PREFIX` line before the endpoint does.
23. As a query author, I want unused `PREFIX` lines reported as warnings
    with a quick fix to remove them, so that queries stay tidy.
24. As a query author, I want a prefix declared twice with different IRIs
    reported, so that I notice the shadowing.
25. As a query author, I want unknown or deprecated terms of well-known
    vocabularies (`rdfs:lable`, `owl:DeprecatedClass` misuse, `schema:` typos)
    reported with "did you mean" fixes, matched by namespace IRI as in
    Turtle, so that a typo does not silently return no results.
26. As a query author, I want terms in my own namespaces that no file in the
    workspace describes reported (when `turtle.lint.undescribedTerms` is on),
    with "did you mean" suggestions from the workspace, so that I find
    misspelled project terms.
27. As a query author, I want a projected variable that the `WHERE` clause
    never binds reported as a warning, so that I notice an always-empty
    column.
28. As a query author, I want SPARQL 1.2 syntax (triple terms, reifiers,
    annotations, `VERSION`) accepted, so that RDF 1.2 queries are not
    flagged.
29. As a query author, I want prefixes from the project's `prefixes.ttl` and
    the `turtle.prefixes` setting to count as declared in queries, as they
    do in Turtle files, so that a shared header works everywhere.

### SPARQL: completion

30. As a query author, I want keywords completed by context (query forms at
    the top, `OPTIONAL`, `FILTER`, `BIND`, `VALUES`, `MINUS`, `GRAPH`,
    `SERVICE`, `UNION` inside a group, `ORDER BY`, `GROUP BY`, `LIMIT`
    after it), so that I type less.
31. As a query author, I want the variables already used in the query
    offered when I type `?` or `$`, so that I do not misspell them.
32. As a query author, I want prefixes completed, and accepting a prefixed
    term whose prefix is not declared to add the `PREFIX` line for me, so
    that I never write namespace IRIs by hand.
33. As a query author, I want classes first after `a` and properties first in
    predicate position, from the workspace and the vocabularies, so that the
    right terms are at the top.
34. As a query author, I want properties whose `rdfs:domain` fits the class
    the subject variable is typed with (`?p a foaf:Person ; foaf:`) offered
    first, as in Turtle, so that completion follows my ontology.
35. As a query author, I want built-in functions (`STR`, `LANG`, `REGEX`,
    `CONCAT`, `COALESCE`, aggregates, ...) completed in expressions, so that
    I remember their names.

### SPARQL: understanding a query

36. As a query author, I want hover on a term to show its label, comment,
    types and where it is described, exactly as in Turtle, so that I know
    what `wdt:P31` means.
37. As a query author, I want label inlay hints on opaque IRIs in queries
    (same setting as Turtle), so that Wikidata-style queries are readable.
38. As a query author, I want go to definition on a term to open where the
    workspace describes it, so that I can check its definition.
39. As a query author, I want go to definition on a prefixed name's prefix
    to jump to its `PREFIX` line, and on a variable to its first
    occurrence in its scope, so that I can navigate a long query.
40. As a query author, I want document highlight on a variable to mark every
    occurrence in its scope (a subquery's own variables are a separate
    scope unless projected), so that I see where it is bound and used.
41. As a query author, I want find references on a variable to list its
    occurrences in scope, and on a term to list its uses in the query and in
    the workspace, so that I can trace a term from its query to its data.
42. As a query author, I want semantic highlighting of variables, keywords,
    functions, prefixes, and of classes, properties and individuals as in
    Turtle, so that the role of each name is visible.
43. As a query author, I want the outline to show the query form or each
    update operation with its clauses, so that I can jump around a long
    update script.
44. As a query author, I want folding on `{ }` groups, the `PREFIX` block
    and comment blocks, so that I can collapse parts of a long query.

### SPARQL: changing a query

45. As a query author, I want to rename a variable across its scope, keeping
    each occurrence's `?` or `$`, so that I can give it a better name.
46. As a query author, I want renaming a variable to a name already used in
    the same scope refused, so that the rename never merges two variables.
47. As a query author, I want to rename a prefix in a query and every
    prefixed name that uses it, so that I can adopt the project's prefix.
48. As an ontology author, I want renaming a term in a Turtle file to also
    rename it in the SPARQL files of the workspace (open or not), and
    renaming it from a query to rename it everywhere, so that queries do not
    break after a refactoring.
49. As a query author, I want formatting that indents groups, puts one
    triple pattern per line, aligns the `PREFIX` block, keeps comments, and
    never changes what the query means (a formatted query parses to the same
    algebra, or nothing is changed), so that I can format without fear.
50. As a query author, I want formatting to respect `turtle.format.lineWidth`
    and to give the same output when run twice, so that it is predictable.
51. As a query author, I want formatting refused on a query with syntax
    errors, so that a half-written query is not mangled.
52. As a query author, I want quick fixes to add a missing `PREFIX` (the
    project's binding first, then the vocabularies'), add all missing
    prefixes, remove an unused one, and fix a misspelled term, so that
    diagnostics come with their fix.
53. As a query author, I want "organise prefixes" (sort, remove unused,
    deduplicate) and "shorten / expand IRI" on queries, as in Turtle, so that
    the refactorings I know work here too.

### Everyone

54. As a user, I want SPARQL and N-Quads files to change nothing about the
    workspace's knowledge except what they describe (N-Quads) or nothing at
    all (SPARQL), so that a query file never makes a typo look described.
55. As a user, I want the server to stay idle after opening, editing and
    closing `.nq` and `.rq` files, so that the new formats cost nothing when
    I stop typing.
56. As a user, I want a keystroke in a SPARQL file of a few hundred lines to
    get fresh diagnostics in a few milliseconds, as in Turtle, so that the
    editor never lags.
57. As a VS Code user, I want the extension to register N-Quads and SPARQL
    (languages, file extensions, highlighting grammars) and start the server
    for them, so that it works out of the box.
58. As a Neovim user, I want the README to show how to attach the server to
    `sparql` and `nquads` buffers, so that I can set it up.
59. As a maintainer, I want the W3C SPARQL 1.1 and 1.2 syntax suites (query,
    update, federation) to pass in CI, with known deviations listed in the
    harness so that the list cannot go stale.
60. As a maintainer, I want `npm run corpus` to check N-Quads against oxttl
    and SPARQL against spargebra on public real-world files, so that the new
    parsers and formatters are tested beyond the suites.

## Seams

Tests sit at the highest seams that already exist, and add one new library
seam for the W3C SPARQL harness:

1. **The LSP protocol** (the Node end-to-end tests driving the release
   binary): every user-facing behaviour of both formats.
2. **The RDF parse API** (`syntax::parse_with_base` with a `Syntax`): the
   W3C N-Quads suites, through the existing conformance harness.
3. **A SPARQL document API** (new): parse a SPARQL text with a prefix map
   into a document whose diagnostics and formatting can be asked for. The
   W3C SPARQL harness and the corpus use it; the server's SPARQL features are
   built on it.
4. **The corpus harness** (`npm run corpus`): real-world files, compared with
   oxttl (N-Quads) and spargebra (SPARQL).

## Implementation Decisions

### N-Quads

- A fourth `Syntax` variant, N-Quads, with language ids `nquads` / `n-quads`
  and the `.nq` extension; `has_prefixes` is false. Detection follows the
  existing rule: a specific language id wins, else the extension.
- The parser reads it in N-Triples line mode, plus an optional graph label
  (absolute IRI or blank node) before the final `.`. Quads carry the graph
  name the way TriG quads do, so everything downstream of the parse (the
  analysis, knowledge, outline, SHACL graph) needs no new concept.
- Everything that special-cases N-Triples (no prefix diagnostics or
  completions, line-oriented formatting, no prefix quick fixes) applies to
  N-Quads too; a helper on `Syntax` ("line-based") replaces equality checks
  against N-Triples so the two cannot drift.
- The formatter writes one quad per line: subject, predicate, object, graph
  (when present), single spaces, ` .`; comments and order kept; the same
  safety check (the output parses to the same dataset) as other syntaxes.
- The outline groups statements under their graph, as for TriG.
- Workspace indexing picks up `.nq` files through the shared extension
  check.
- The W3C suites: `rdf11/rdf-n-quads` and `rdf12/rdf-n-quads` vendored from
  the same rdf-tests commit as the existing suites; the conformance harness
  maps their test types to N-Quads (evaluation tests compare datasets).

### SPARQL

- A new `sparql` module, independent of the RDF parser:
  - **Lexer** over the whole SPARQL 1.2 token set with byte spans (IRIs,
    prefixed names, variables, literals incl. long strings, numbers,
    keywords case-insensitively, punctuation, comments), never failing: an
    unrecognised character becomes an error token.
  - **Structure**: the prologue (`BASE`, `PREFIX`, `VERSION` with spans),
    whether the document is a query or an update (by its first keyword after
    the prologue, not its extension), the groups (`{ }` nesting, for folding,
    variable scopes and indentation), subqueries and their projections, and
    each name's position in a triple pattern (subject, predicate, object) for
    completion. It is built from tokens so it exists for broken queries too.
  - **Validation** by `spargebra` (`parse_query` or `parse_update`, with the
    document's prefixes plus project and setting prefixes and the document's
    base). The error positioning the SHACL-SPARQL lint does today (`error at
    L:C: expected ...` mapped back to a span) moves into the `sparql`
    module and both callers use it.
  - **Lints** from the structure: undeclared, unused and duplicate prefixes,
    vocabulary terms via the same namespace-matched vocabulary check Turtle
    uses, undescribed own terms (same setting), projected-but-unbound
    variables.
- The new library seam: a SPARQL document is parsed from text plus a prefix
  map and a base; it gives diagnostics (with stable codes, reusing the Turtle
  codes where the meaning is the same: `undeclared-prefix`, `unused-prefix`,
  `duplicate-prefix`, `unknown-term`, `deprecated-term`,
  `undescribed-term`; new: `sparql-syntax`, `unbound-projection`) and
  formatting.
- **Formatter**: token-based and comment-keeping. Keywords upper case,
  `PREFIX` block aligned, two-space indentation per group, one triple
  pattern per line with `;` / `,` continuations indented, solution modifiers
  on their own lines, `turtle.format.lineWidth` for long expressions. Safety
  check: the output must parse to the same algebra as the input with
  `spargebra` (same prefixes and base), and formatting the output again
  must change nothing; otherwise the formatter returns no edits. Refused
  when the document has syntax errors.
- **Documents**: the workspace keeps a document kind (RDF with a `Syntax`,
  or SPARQL). SPARQL documents never feed the knowledge index. SPARQL files
  on disk are not analysed at startup; references and rename of a term scan
  the workspace's SPARQL files then (open documents from memory, the rest
  from disk).
- **Features** reuse the RDF features' knowledge lookups and presentation
  (hover content, completion ranking by `rdfs:domain` / SHACL, label inlay
  hints, semantic token types and modifiers, "did you mean"), on positions
  from the SPARQL structure. New semantic token types: `variable`,
  `keyword`, `function` (added to the legend; existing indices unchanged).
- **Variable scope**: a group pattern is one scope; a subquery is its own
  scope whose projected variables are shared with the enclosing scope. Rename
  keeps `?` or `$`, accepts the new name with or without a sigil, and refuses
  a name already used in the scope or an invalid variable name.
- **Prefix rename** in a query edits the `PREFIX` line and every prefixed
  name using it; a prefix from `prefixes.ttl` or the setting is refused with
  a pointer to where it is declared (as in Turtle).
- **Term rename** across files: the existing term rename gathers edits from
  SPARQL documents too (a prefixed name is renamed only when the new IRI
  keeps the same namespace, otherwise it becomes a full IRI).
- **Server**: language ids `sparql` and `nquads`/`n-quads`; capabilities
  unchanged except the semantic token legend. Every request handler routes
  by document kind. SHACL validation ignores SPARQL documents; N-Quads data
  takes part like TriG data.
- **Extension**: languages `nquads` (`.nq`) and `sparql` (`.rq`, `.sparql`,
  `.ru`), TextMate grammars for both, language configuration (comments,
  brackets), the document selector, and the Marketplace README. The
  extension activates on these languages.
- **README**: format table, Neovim setup for `sparql` / `nquads` filetypes,
  badges: "W3C Turtle, TriG, N-Triples, N-Quads x/x" and
  "W3C SPARQL syntax x/x".

### Corpus

- `tools/corpus/sources.tsv` gains formats:
  - `nq`: an N-Quads file;
  - `nq-head`: the first megabytes of a large gzipped N-Quads dump (a Web
    Data Commons RDFa extraction), cut at the last complete line; real-world
    data with real-world errors;
  - `sparql-examples`: the SIB `sparql-examples` repository at a pinned
    commit (UniProt, Rhea, Bgee, neXtProt, ... about a thousand real
    queries), from which every `sh:select`, `sh:ask`, `sh:construct`,
    `sh:describe` and `sh:update` literal becomes one SPARQL case.
- The corpus harness:
  - N-Quads cases compare with `oxttl`'s N-Quads parser (same quads, same
    validity), plus a generated `NAME.gen.nq` from every Turtle file under
    5 MB (as `.gen.trig` is generated today);
  - SPARQL cases check that our diagnostics agree with `spargebra` on
    validity (no `sparql-syntax` error exactly when spargebra parses it,
    given the examples' shared prefixes), the lexer has no error tokens on
    valid queries, the formatter keeps the algebra, every comment, and is
    idempotent, nothing panics, and each case stays under the time limit;
  - the report gains the new rows.
- `lsp-cost.mjs` measures open-to-diagnostics on the largest N-Quads case and
  on a long SPARQL query.

## Testing Decisions

- Tests are written first (TDD) and fail until the slice that implements
  them lands. A good test drives the server or library from outside, the
  way a client or a user would, and asserts on what they see: diagnostics
  (code, range, message substance), completion items, edits, locations,
  hovers, tokens. No test reaches into private structures.
- **End-to-end (Node, LSP protocol)**, prior art `test/trig.test.mjs`,
  `test/lsp.test.mjs`, `test/format.test.mjs`, `test/prefix-rename.test.mjs`,
  `test/protocol.test.mjs`:
  - `test/nquads.test.mjs`: detection by id and extension, diagnostics
    (bad graph label, quint, bad literal, relative IRI, recovery), no prefix
    diagnostics, outline by graph, hover/definition/references across
    a `.nq` and a `.ttl`, knowledge contribution (an undescribed-term warning
    in Turtle disappears when a `.nq` describes the term), formatting.
  - `test/sparql.test.mjs`: detection, query vs update, syntax errors and
    their ranges, prefix diagnostics, project prefixes, vocabulary and
    undescribed terms, unbound projection, SPARQL 1.2 syntax.
  - `test/sparql-completion.test.mjs`: keywords by context, variables,
    prefixes with auto-`PREFIX`, classes after `a`, properties in predicate
    position, domain ranking, functions.
  - `test/sparql-navigation.test.mjs`: hover, definition (term, prefix,
    variable), highlight and references with subquery scoping, semantic
    tokens, folding, outline, inlay hints.
  - `test/sparql-rename.test.mjs`: variable rename (sigils, scope, refusal),
    prefix rename, term rename across Turtle and SPARQL files in both
    directions.
  - `test/sparql-format.test.mjs`: layout cases, comments kept, idempotent,
    refusal on errors, line width.
  - `test/sparql-actions.test.mjs`: add / add all / remove prefix, fix
    misspelled term, organise prefixes, shorten / expand IRI.
  - `test/protocol.test.mjs`: semantic token legend has the new types after
    the existing ones.
  - `test/resources.test.mjs`: idle CPU after opening, editing, formatting
    and closing `.nq` and `.rq` files; no stray processes or files.
- **Conformance (Rust)**, prior art `tests/w3c_conformance.rs`,
  `tests/w3c_shacl.rs`: N-Quads in the existing harness; a new
  `tests/w3c_sparql.rs` running the vendored SPARQL 1.1 (`syntax-query`,
  `syntax-update-1`, `syntax-update-2`, `syntax-fed`) and 1.2 syntax
  suites through the SPARQL document API: positive tests have no error
  diagnostics, no error tokens, and format to the same algebra idempotently;
  negative tests have at least one error. Known deviations are listed with a
  reason, and a listed test that passes fails the run.
- **Fuzz**: the chaos test (`tests/chaos.rs`) also drives `.nq` and `.rq`
  documents.
- **VS Code**: the extension test opens a `.nq` and a `.rq` file and sees
  diagnostics from the server.
- **Corpus**: as above; run by hand (`npm run corpus`), not in CI.
- Each slice runs its targeted tests while working; the full verification
  (cargo tests, clippy, fmt, `npm test`, `npm run bench`, VS Code tests,
  corpus, resource hygiene) runs once after every slice has landed.

## Out of Scope

- Running queries against an endpoint, showing results, or any network use
  from SPARQL documents.
- Endpoint-specific prefixes or functions (Wikidata's predefined prefixes,
  `wikibase:label` service semantics); users can put such prefixes in
  `turtle.prefixes` or a `prefixes.ttl`.
- SPARQL embedded in other files (SHACL `sh:select` literals keep today's
  lint; Python or Java strings get nothing).
- Hover documentation for SPARQL keywords and built-in functions.
- Signature help for functions.
- N-Quads specific SHACL behaviour beyond what TriG data already gets.
- Other formats (JSON-LD, RDF/XML editing, N3).

## Further Notes

- Suites: W3C rdf-tests commit `d5d930b03ee6b63b388d0bb28fdb29ee78a59ec0`
  (the commit already vendored), directories `rdf/rdf11/rdf-n-quads`,
  `rdf/rdf12/rdf-n-quads`, `sparql/sparql11/{syntax-query,syntax-update-1,syntax-update-2,syntax-fed}`,
  `sparql/sparql12/{syntax,syntax-triple-terms-positive,syntax-triple-terms-negative,codepoint-escapes,version}`
  (syntax tests only).
- Corpus: `sib-swiss/sparql-examples` commit
  `45e5b53a2d59a121221cd66a8596ef868cc58021`.
- `spargebra` already passes the SPARQL syntax suites; what the harness tests
  is our layer on top of it (lexer, structure, error positions, formatter),
  and that our diagnostics never disagree with it.

## Slices

Slices are cut by files so that slices without an edge can run together. The
server and workspace wiring is the lock: one slice owns those files.

| Slice | Owns | Blocked by | Acceptance |
|---|---|---|---|
| T. Tests first | `test/*.test.mjs` (new and edited), `tests/w3c_sparql.rs`, vendored suites, harness N-Quads mapping, `sources.tsv`, fetch script, chaos inputs, SPARQL API stubs | none | New tests compile and fail for the right reason; existing tests still pass |
| A. N-Quads | `syntax/*`, `features/format.rs`, N-Triples special cases in `features/*` | T | W3C N-Quads suites pass; `nquads.test.mjs` passes |
| B. SPARQL core | `sparql/` lexer, structure, validation, lints; error positioning moved out of the SHACL-SPARQL lint | T | `w3c_sparql.rs` diagnostics part passes; SHACL-SPARQL tests still pass |
| C. Wiring (lock) | `server.rs`, `workspace.rs`, `analysis.rs` kind plumbing | A, B | `sparql.test.mjs`, protocol and resources tests pass |
| D. SPARQL formatter | `sparql/format` | B | `w3c_sparql.rs` formatting part and `sparql-format.test.mjs` pass |
| E. SPARQL features | `sparql/features` (completion, hover, navigation, rename, semantic tokens, folding, symbols, inlay hints, code actions); term-rename hook in `features/navigation.rs` | B, C | completion, navigation, rename and actions tests pass |
| F. Editors | `editors/vscode/*`, README(s) | C | VS Code test passes with `.nq` and `.rq` |
| G. Corpus | `examples/corpus.rs`, `tools/corpus/*` | A, B, D | `npm run corpus` has no findings on the new cases |

D, E and G run alongside each other once their blockers land.
