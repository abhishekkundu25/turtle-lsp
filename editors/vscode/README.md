<p align="center"><img src="https://raw.githubusercontent.com/abhishekkundu25/turtle-lsp/main/assets/logo.png" alt="turtle-lsp" width="640"></p>

# Turtle RDF Language Server for VS Code

Language support for **RDF Turtle (`.ttl`)** files: ontologies (OWL, RDFS,
SKOS), SHACL shapes and linked data. Powered by
[turtle-lsp](https://github.com/abhishekkundu25/turtle-lsp), a language server
written in Rust: a keystroke in a 30,000-line file gets fresh diagnostics in
about 9 ms.

## Features

- **Syntax highlighting** for Turtle and RDF 1.2 (triple terms `<<( )>>`, reifiers `~`, annotations `{| |}`).
- **Error checking as you type**: syntax errors with recovery (one mistake does not hide the rest of the file), undeclared, unused and duplicate prefixes, invalid IRIs and language tags.
- **Vocabulary checks**: unknown or deprecated terms in 80+ well-known vocabularies (rdf, rdfs, owl, skos, sh, dcterms, foaf, schema.org, prov, sosa, qudt and more), with "did you mean" fixes. Matched by namespace IRI, so your own `ex:` or `sd:` prefixes are never checked against the wrong vocabulary.
- **Completion** of prefixes and terms; type `Pers` to get `foaf:Person`. Classes come first after `a`, properties in predicate position. Accepting a term adds its `@prefix` line for you.
- **Hover** with label, comment, types and where a resource is described.
- **Navigation**: go to definition, type definition (a resource's class), implementations (a class's instances and subclasses), find references, rename across files, document highlight.
- **Outline, workspace symbol search and folding.**
- **Quick fixes**: add a missing prefix, add all missing prefixes, remove an unused prefix, fix a misspelled term.

## Project prefixes

If your Turtle files share a prefix header instead of declaring prefixes
themselves, put the header in a `prefixes.ttl`. Every Turtle file in that
folder or below can then use those prefixes. Or set `turtle.prefixes`.

## Settings

- `turtle.prefixes`: prefixes available in every file, e.g. `{ "ex": "http://example.org/" }`.
- `turtle.server.path`: use a different `turtle-lsp` binary (user settings only; ignored in untrusted workspaces).
- `turtle.trace.server`: log the language server traffic to the Output panel.
- `turtle.inlayHints.labels`: show labels as inlay hints for `opaque` IRIs (default), `all` terms, or `off`.
- `turtle.shacl.enabled`, `turtle.shacl.maxTriples`: SHACL validation and the size above which it is skipped (default 2,000,000 triples).
- `turtle.lint.undescribedTerms`: warn about your own terms that are described nowhere in the workspace.
- `turtle.imports.network`: fetch `owl:imports` over the network (default off; ignored in untrusted workspaces). With it off, the quick fix on an unresolved import fetches that one import and caches it.
- `turtle.format.lineWidth`: line width for formatting (default 100).

## Also for Neovim and other editors

The same server works in any editor that speaks the Language Server Protocol.
See the [project README](https://github.com/abhishekkundu25/turtle-lsp#readme)
for Neovim setup.
