# 01: Core query module and renderers

**What to build:** a library module that runs a SPARQL query over a set of documents (the analyses SHACL validation reads): builds an `rdf::graph::Graph`, evaluates with `spareval` through `rdf::dataset`, and returns solutions, a boolean or triples, or an error (syntax with position, unsupported form, time limit). Default graph = union of every triple, named graphs included; no named graphs (GRAPH / FROM NAMED match nothing; FROM ignored). UPDATE rejected. Time limit and cancellation. Pure renderers: aligned text table, Markdown table with a row limit ("showing N of M"), SPARQL 1.1 JSON results, CSV, TSV, Turtle for triples via the existing formatter with given prefixes; compact IRIs in table and Markdown.

**Blocked by:** lsp-capability 01 (the `rdf` module).

**Status:** ready-for-agent

- [ ] Rust unit tests: SELECT, ASK, CONSTRUCT, DESCRIBE; union default graph across Turtle and TriG; GRAPH matching nothing; UPDATE rejected; syntax error position; time limit; each renderer (escaping included).
- [ ] No change to SHACL behaviour or the SHACL suites.
