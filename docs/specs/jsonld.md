# JSON-LD support

Status: done in 0.6.0 (phase 1). Tickets: `docs/tickets/jsonld/`.

## Problem Statement

turtle-lsp treats a project as one body of RDF knowledge, but only for
Turtle, TriG, N-Triples, N-Quads and SPARQL. Many projects keep part of
their data in JSON-LD:
- schema.org markup;
- ActivityStreams;
- Verifiable Credentials;
- IIIF manifests;
- DCAT catalogues;
- application payloads.

For those users:
- a `.jsonld` file is invisible to the workspace;
- terms it defines are reported as undescribed in Turtle files;
- SHACL shapes in Turtle cannot validate JSON-LD data;
- `turtle-lsp check` ignores the files.

The one extension that edits JSON-LD (SWLS) knows only open files. It has no
hover on keys and no go to definition into `@context`, and its rename breaks
the document by leaving the `@context` entry behind.

JSON-LD is harder than Turtle. What a key means depends on `@context`
processing:
- contexts can be remote, nested, scoped by type or property, and protected;
- they can alias keywords;
- they can set `@vocab` and `@base`.

Doing it right needs a correct expansion engine that remembers where every
triple came from. Correctness can only be claimed with a test suite behind
it.

## Solution

**Tests first.** Before any engine code:
- **The W3C JSON-LD 1.1 suites** that matter to an editor (toRdf, expand,
  remote-doc), with a runner and an expected-status manifest.
- **A real-world corpus** with a graph oracle.
- **Every planned editor feature as a pending test,** tagged by phase.

**Then phase 1: the engine and data features.**
- **The engine:** a position-tracking JSON parser and JSON-LD 1.1 context
  processing and expansion to triples, written in-house, where each triple
  keeps the source span of the key or value that produced it.
- **Workspace:** `.jsonld` files join the workspace. Their terms count in
  Turtle and SPARQL features, and their data is validated by the project's
  SHACL shapes, with results on the exact key.
- **Diagnostics:** JSON syntax, context errors, invalid IRIs, undefined keys.
- **Everywhere else:** `turtle-lsp check` covers them, and the browser editor
  works too.

Editing features inside JSON-LD files (completion, hover, go to definition,
rename, formatting) are phase 2. Their tests exist from the start as pending
tests.

## User Stories

1. As a maintainer, I want the W3C JSON-LD 1.1 toRdf, expand and remote-doc suites running in `cargo test`, with an expected-status manifest, so that conformance only goes up and regressions fail the build.
2. As a maintainer, I want a real-world JSON-LD corpus whose graphs are compared with a reference implementation, so that the engine is right on files people actually write.
3. As a maintainer, I want every planned JSON-LD editor feature as a pending test tagged with its phase, so that progress is visible and nothing planned is forgotten.
4. As a maintainer, I want pending tests to report when they start passing, so that I can promote them.
5. As a JSON-LD author, I want JSON syntax errors at the exact position, with recovery, so that one missing comma doesn't hide the rest of the file.
6. As a JSON-LD author, I want invalid `@context` definitions, context cycles and protected-term redefinitions reported where they occur.
7. As a JSON-LD author, I want a warning on a key the active context does not define (which JSON-LD silently drops), so that typos don't lose data.
8. As a JSON-LD author, I want invalid IRIs and malformed language tags reported at the value.
9. As a JSON-LD author, I want remote contexts loaded from a cache, with common contexts bundled (schema.org, ActivityStreams, Verifiable Credentials), so that my files work offline.
10. As a JSON-LD author, I want network fetching of contexts off by default and enabled by the same setting as imports, so that opening a file never goes online unexpectedly.
11. As a Turtle author, I want classes and properties defined in JSON-LD files to be known in completion, hover and references in my Turtle and SPARQL files.
12. As a Turtle author, I want "undescribed term" to stop flagging terms a JSON-LD file describes.
13. As a SHACL user, I want my Turtle shapes to validate JSON-LD data, with each result on the key or value that breaks the shape.
14. As a SHACL user, I want JSON-LD files that contain shapes to have those shapes used.
15. As a CI maintainer, I want `turtle-lsp check` to report JSON-LD problems with the same codes, positions and exit codes as for Turtle.
16. As an agent, I want the same JSON and SARIF output for JSON-LD files, so that one integration covers every format.
17. As a browser-editor user, I want `.jsonld` files in the sample workspace to behave as on the desktop.
18. As a user with a large JSON-LD file, I want opening and re-checking it to take about as long as a Turtle file of the same size.
19. As a user, I want named graphs in JSON-LD (`@graph` with `@id`) to behave as TriG named graphs do.
20. As a maintainer, I want JSON-LD documents to produce the same parse model as Turtle documents, so that downstream features need no JSON-LD-specific code.

## Implementation Decisions

**One parse model for every format.** A JSON-LD document produces the same
parsed structure Turtle and TriG produce:
- triples with the spans of subject, predicate and object;
- references (term uses with their roles);
- errors;
- tokens.

Knowledge, lints, SHACL, the CLI and the browser engine then work unchanged.
This is the seam; the ideal is that nothing downstream branches on JSON-LD.

**New syntax kind.** JSON-LD is a new syntax alongside Turtle, TriG,
N-Triples and N-Quads:
- detection by `.jsonld` and the `jsonld` language id;
- generic `.json` files are not detected in phase 1.

Adding it to the syntax enumeration is the one wide change. It lands first,
as a prefactor, with an empty frontend, so every exhaustive match is
updated once.

**An in-house engine.** The JSON parser and the JSON-LD algorithms are
written in the crate:
- an existing JSON-LD crate cannot report source spans per triple, and
  spans are what every feature needs;
- an existing crate (oxjsonld) is used only as a test oracle
  (dev-dependency), the same way oxttl is.

**Engine parts:**
- **JSON parser:** fault-tolerant, keeping spans for every key, value and
  structural token, and recovering at commas and brackets.
- **Context processing:** follows JSON-LD 1.1 (active context, term
  definitions, `@vocab`, `@base`, `@language`, `@direction`, `@protected`,
  `@propagate`, `@import`, scoped contexts, keyword aliases). Errors carry
  the span of the offending definition.
- **Expansion and toRdf:** follow JSON-LD 1.1, producing triples and named
  graphs. Each triple records the spans of the key that gave its predicate,
  the node that gave its subject (`@id`, or the enclosing object), and the
  value that gave its object. Lists, `@reverse`, `@nest`, `@included`,
  `@json` and value objects are covered.
- **Out of phase 1:** compaction, flattening, framing and fromRdf.

**Context loading.**
- **The loader is an interface,** so tests and the browser can inject
  documents.
- **The desktop loader** resolves, in order: file-relative contexts, the
  import cache, then bundled common contexts. The network comes last, only
  with `imports.network` or `--network`.
- **Caching:** processed contexts are cached by URL and content, so
  re-checking a document re-processes only its inline contexts.
- **Shared cache:** remote contexts share the existing imports cache
  directory and its offline semantics.

**Diagnostics.**
- **New codes:** `json-syntax`, `jsonld-context`, `jsonld-undefined-term`
  (warning), `invalid-iri`, `invalid-language-tag`.
- **Reused codes:** existing codes apply unchanged where the meaning is the
  same, such as `unknown-term` for misspelled vocabulary IRIs reached through
  the context, `undescribed-term`, and SHACL codes.

**Workspace, SHACL, CLI and web.**
- **Workspace:** JSON-LD files join the index like TriG.
- **SHACL:** named graphs in JSON-LD follow the TriG unit rules.
- **CLI:** collects `.jsonld`.
- **Browser engine:** gets the same frontend, with a loader backed by
  bundled contexts.

**Performance budgets** (release build, a generated 30,000-line JSON-LD
file):
- open to diagnostics within 3× the Turtle equivalent;
- keystroke to syntax and lint diagnostics under 10 ms with cached contexts;
- no network on open.

## Testing Decisions

**A good test** uses only public behaviour:
- for Rust, the parse model and engine API;
- for Node, the LSP protocol and the CLI;
- it asserts codes and ranges, never message wording.

**W3C suites** (Rust integration test, like the existing W3C runners):
- vendored manifests for toRdf, expand and remote-doc;
- expected statuses: pass, known-fail with reason, or out-of-scope;
- the test fails when a pass regresses or when a known-fail starts passing,
  which prompts an update of the manifest;
- remote-doc runs offline through an injected loader.

**Corpus:**
- public, licensed JSON-LD fetched by the existing corpus script into the
  gitignored corpus folder;
- each file's graph must be isomorphic to oxjsonld's;
- a list of sources with licences is committed; the files are not;
- synthetic large files are generated by the benchmark for the budgets.

**Pending feature tests** (Node, `todo` with a phase tag, such as
`jsonld:phase1` and `jsonld:phase2`):
- diagnostics, cross-file knowledge, SHACL positions, CLI and the browser
  engine (phase 1);
- completion, hover, go to definition, references, rename (including the
  `@context` entry, which SWLS gets wrong), formatting (idempotent and
  graph-preserving), semantic tokens, outline (phase 2);
- a pending test whose behaviour works is promoted in the same change.

**Performance:** the benchmark gains JSON-LD rows; budgets are asserted only
in release builds.

**Prior art:**
- the W3C runners;
- the oxttl oracle test;
- the CLI tests;
- the LSP tests in the Node suite.

## Out of Scope

- **Phase 2 editing features:** completion, hover, go to definition and
  rename inside JSON-LD, formatting, semantic tokens, outline.
- **Other JSON-LD algorithms:** compaction, flattening, framing, fromRdf, and
  converting between JSON-LD and Turtle.
- **Other file types:** `.json` files detected by `@context`, YAML-LD, and
  JSON-LD embedded in HTML.
- **RDF/XML editing.**

## Further Notes

- **Order:** the order above (tests, corpus, prefactor, engine, then
  integration) is deliberate. The engine is the risk, and the W3C suite is
  what keeps it honest.
- **The comparison with SWLS** (scratch report from 2026-10-20) found SWLS's
  JSON-LD engine skips `@nest` and has TODOs for `@protected`, `@import`,
  `@propagate` and `@direction`. Covering those is a concrete, checkable
  difference.
- **Claims:** keep public claims to what the suite and corpus prove.
