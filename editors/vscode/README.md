<p align="center"><img src="https://raw.githubusercontent.com/abhishekkundu25/turtle-lsp/main/assets/logo.png" alt="turtle-lsp" width="640"></p>

# Turtle RDF Language Server for VS Code

Language support for **RDF Turtle (`.ttl`), TriG (`.trig`), N-Triples (`.nt`),
N-Quads (`.nq`) and SPARQL (`.rq`, `.sparql`, `.ru`)**: ontologies (OWL, RDFS,
SKOS), SHACL shapes, linked data, and the queries and updates you run on them. Powered
by [turtle-lsp](https://github.com/abhishekkundu25/turtle-lsp), a language
server written in Rust: a keystroke in a 30,000-line file gets fresh
diagnostics in about 9 ms.

Try it first in your browser, with nothing to install: [RDF Editor](https://abhishekkundu25.github.io/turtle-lsp/).

<p align="center"><img src="https://raw.githubusercontent.com/abhishekkundu25/turtle-lsp/main/assets/demo.gif" alt="Editing RDF in the browser: SHACL errors as you type, completion from the shapes, rename across files" width="900"></p>

Passes the W3C test suites: Turtle, TriG, N-Triples and N-Quads 1137/1137,
SPARQL 1.1 and 1.2 syntax 425/426, SHACL Core 98/98, SHACL-SPARQL 23/23.

## Features

- **Syntax highlighting** for Turtle, TriG, N-Triples, N-Quads, SPARQL and RDF 1.2 (triple terms `<<( )>>`, reifiers `~`, annotations `{| |}`), plus **semantic highlighting** of classes, properties, individuals, datatypes and deprecated terms.
- **Error checking as you type**: syntax errors with recovery (one mistake does not hide the rest of the file), undeclared, unused and duplicate prefixes, invalid IRIs and language tags.
- **Vocabulary checks**: unknown or deprecated terms in 80+ well-known vocabularies (rdf, rdfs, owl, skos, sh, dcterms, foaf, schema.org, prov, sosa, qudt and more), with "did you mean" fixes. Matched by namespace IRI, so your own `ex:` or `sd:` prefixes are never checked against the wrong vocabulary. Terms in your own namespaces that nothing describes (often typos) are flagged too.
- **SHACL validation**: data is checked against the shapes in your workspace about a second after you stop typing, and on save. SHACL Core and SHACL-SPARQL (`sh:sparql` constraints, SPARQL targets and components). SPARQL inside shapes is checked as you type, and mistakes in the shapes themselves are flagged.
- **`owl:imports`**: imported ontologies are resolved from the workspace, a `catalog-v001.xml` or the cache (Turtle, N-Triples, RDF/XML), so their terms complete, hover and validate. An unresolved web import has a quick fix that fetches that one import and caches it; go to definition opens the cached copy.
- **Completion** of prefixes and terms; type `Pers` to get `foaf:Person`. Classes come first after `a`, properties in predicate position; properties your SHACL shapes require or whose `rdfs:domain` fits come first, and `sh:in` values are offered. Accepting a term adds its `@prefix` line for you.
- **Hover** with label, comment, types and where a resource is described; **label inlay hints** for opaque IRIs.
- **Navigation**: go to definition, type definition (a resource's class), implementations (a class's instances and subclasses), type hierarchy, find references, document highlight.
- **Rename across files**, including a prefix shared through `prefixes.ttl`, which is renamed in every file that takes it from there.
- **Formatting** that keeps comments and statement order and never changes the graph.
- **Outline, workspace symbol search and folding.**
- **Quick fixes and refactorings**: add a missing prefix (or the `owl:imports` that defines it), add all missing prefixes, remove an unused prefix, fix a misspelled term, organise prefixes, shorten or expand an IRI, extract a prefix for a namespace.

## SPARQL

SPARQL queries and updates (SPARQL 1.1 and 1.2) get the same knowledge of your
workspace and the vocabularies:

- Syntax errors as you type, at the token where the query stops making sense; undeclared, unused and duplicate prefixes; unknown or deprecated vocabulary terms and undescribed terms of your own, with "did you mean"; a projected variable that the `WHERE` clause never binds.
- Completion of keywords, variables, prefixes, terms and built-in functions. Accepting a term adds its `PREFIX` line.
- Hover and label inlay hints on terms, as in Turtle.
- Go to definition, references and document highlight for terms, prefixes and variables.
- Rename of variables, prefixes and terms. Renaming a term in a Turtle file also renames it in your queries, and the other way round.
- Formatting that keeps comments and never changes what the query means.
- Quick fixes: add a missing `PREFIX` (or all of them), remove an unused one, fix a misspelled term; organise prefixes, shorten or expand an IRI.
- Semantic highlighting, folding and an outline of the query or update operations.

**▶ Run query** above a query runs it over your workspace's files as the editor has them, unsaved edits included, and opens the results beside it as a Markdown table (or `true`/`false`, or Turtle for `CONSTRUCT` and `DESCRIBE`). The query runs in the background with a time limit and a row limit. No endpoint is contacted, and updates are never run.

## N-Quads

`.nq` files get what N-Triples files get: syntax errors per line with recovery, hover, navigation, an outline grouped by graph, and formatting with one quad per line. Their quads join the workspace, so terms described in a dataset dump count as described in your Turtle files.

## JSON-LD

`.jsonld` files keep VS Code's JSON highlighting and get the server's checks: JSON syntax errors, `@context` errors, keys the context does not define, invalid IRIs and language tags, and SHACL results from your shapes on the key or value that breaks them. A compact IRI such as `rdfs:label` whose prefix the `@context` lacks gets a warning and a quick fix that adds the prefix.

Editing works as in Turtle:

- completion in keys and `@type` values from the context in force, including `@vocab` terms and `prefix:local` names; completing a known prefix the context lacks adds it to the `@context`;
- hover, go to definition (into the `@context` entry or a context file), references and highlights on terms;
- rename of terms, keyword aliases and IRIs, each use rewritten in its own form; unsafe renames are refused with the reason;
- Format Document, keeping the same JSON value;
- semantic highlighting of keywords, properties and classes, and an outline that nests named graphs.

Their terms count in your Turtle and SPARQL files. Common contexts (schema.org, ActivityStreams, Verifiable Credentials, DID) are bundled; other remote contexts are fetched only with `turtle.imports.network` on.

## Project prefixes

If your Turtle files share a prefix header instead of declaring prefixes
themselves, put the header in a `prefixes.ttl`. Every Turtle file and SPARQL
query in that folder or below can then use those prefixes. Or set
`turtle.prefixes`.

## Settings

- `turtle.prefixes`: prefixes available in every file, e.g. `{ "ex": "http://example.org/" }`.
- `turtle.server.path`: use a different `turtle-lsp` binary (user settings only; ignored in untrusted workspaces).
- `turtle.trace.server`: log the language server traffic to the Output panel.
- `turtle.inlayHints.labels`: show labels as inlay hints for `opaque` IRIs (default), `all` terms, or `off`.
- `turtle.shacl.enabled`, `turtle.shacl.maxTriples`: SHACL validation and the size above which it is skipped (default 2,000,000 triples).
- `turtle.shacl.onType`: also validate while typing, not only on save (default on).
- `turtle.lint.undescribedTerms`: warn about your own terms that are described nowhere in the workspace.
- `turtle.imports.network`: fetch `owl:imports` and JSON-LD contexts over the network (default off; ignored in untrusted workspaces). With it off, the quick fix on an unresolved import fetches that one import and caches it.
- `turtle.imports.unresolved`: while an import can't be loaded, SHACL results it could change are warnings with a note (`soften`, the default); `ignore` keeps their full severity.
- `turtle.format.lineWidth`: line width for formatting (default 100).
- `turtle.codeLens.references`: show "N references" above each subject a file describes; click it to list them (default on).
- `turtle.diagnostics.workspace`: also check the files that are not open, in the background, and show their problems (default off).
- `turtle.query.timeoutSeconds`, `turtle.query.maxRows`: stop a query run with **▶ Run query** after this many seconds (default 30), and show at most this many rows of its results (default 1000); `0` for no limit.
- `turtle.query.maxBufferedRows`: stop a query that keeps more than this many rows in memory to sort, group, deduplicate or join them (default 10 000 000; `0` for no limit). It counts rows, not bytes: ordinary rows take about 100 bytes each, long text values take more.

## Also for Neovim and other editors

The same server works in any editor that speaks the Language Server Protocol.
See the [project README](https://github.com/abhishekkundu25/turtle-lsp#readme)
for Neovim setup.
