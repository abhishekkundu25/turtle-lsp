# Browser-based RDF tools: a survey for the turtle-lsp web toolbox

Date: 2026-10-19. Purpose: decide what a browser-only RDF toolbox built on
turtle-lsp (compiled to WebAssembly, run in a Web Worker) must do to beat the
online tools people use today.

> **Historical.** Written on 2026-10-19, this research describes the code and the
> tools as they were then; file names, figures and decisions may have
> changed since. README.md, ARCHITECTURE.md and the module docs describe
> the code as it is now.

Method: the tools' own pages, READMEs and announcements, fetched on the date
above. Where a page could not be fetched, the row says so and the cell is
marked "(not verified)".

## Comparison

"Client-side" means the data is processed in the browser tab, not sent to a
server.

| Tool | What it does | Client-side | Formats | Strengths | Gaps |
|---|---|---|---|---|---|
| IDLab Turtle Validator ([ttl.summerofcode.be](https://ttl.summerofcode.be), [source](https://github.com/IDLabResearch/TurtleValidator)) | Syntax and XSD datatype check | Yes (browserified N3.js) | Turtle, N-Triples | The tool people reach for first; fast; also a CLI | Validation only, one error stream, no fixes, no formatting, no prefixes or vocabulary checks, no TriG/N-Quads/SPARQL. Users on the W3C list point out it "only does validation, only for turtle" ([thread](https://lists.w3.org/Archives/Public/semantic-web/2023Nov/0034.html)) |
| EasyRDF Converter ([easyrdf.org/converter](https://www.easyrdf.org/converter)) | Format conversion | No (PHP server) | RDF/XML, Turtle, N-Triples, JSON, RDFa in; Graphviz out (not verified: page returned 403) | Many formats, permalinkable | Uploads your data; parse errors are a one-line message; no validation |
| RDF Play ([rdf-play.rubensworks.net](https://rdf-play.rubensworks.net), [source](https://github.com/rubensworks/rdf-play.js)) | Parse, serialize, dereference, convert | Yes (Comunica parsers) | Turtle/TriG/N-Triples/N-Quads 1.2, N3, JSON-LD 1.1, RDF/XML, RDFa, Microdata, SHACL Compact | Widest format list, RDF 1.2 | Conversion only: no diagnostics beyond the first parse error, no SHACL, no editor features |
| SoWasm ([page](https://perso.liris.cnrs.fr/pierre-antoine.champin/2023/sowasm/)) | Parse, convert, canonicalize | Yes (Rust Sophia library, WebAssembly) | N-Triples, N-Quads, Turtle, TriG, JSON-LD, YAML-LD, RDF/XML | Only tool here with RDFC-1.0 canonicalization; same Rust-to-Wasm approach as turtle-lsp | Plain textarea, no diagnostics, no validation |
| Zazuko RDF Converter, Sketch, prefix.zazuko.com ([zazuko.com/tools](https://zazuko.com/tools/)) | Convert; draw a model diagram; look up prefixes | Converter and Sketch in the browser | Common RDF syntaxes | Polished single-purpose tools; diagramming | Each tool does one thing; data must be copied between them |
| Zazuko SHACL Playground ([zazuko.com/tools/shacl-playground](https://zazuko.com/tools/shacl-playground/), [source](https://github.com/zazuko/shacl-playground)) | Data and shapes side by side, validation report below | Yes ("100% client-side", rdf-validate-shacl) | "A wide variety of RDF serializations" | The best-known client-side SHACL tool | Report is detached from the source text; SHACL Core only; no editor help while writing shapes |
| SHACL Playground, shacl.org (TopQuadrant, [announcement](https://lists.w3.org/Archives/Public/public-rdf-shapes/2017Mar/0003.html)) | The original in-browser SHACL demo | Yes | Turtle, JSON-LD | Historic reference | Self-described partial implementation, "mostly a demonstrator" ([thread](https://lists.w3.org/Archives/Public/public-shacl/2020Feb/0004.html)), unmaintained |
| SHACL Play! (Sparna, [announcement](https://lists.w3.org/Archives/Public/semantic-web/2020Nov/0027.html)) | Validate by URL, upload or paste; shapes catalogue; shape diagrams; badges | No (TopBraid SHACL API on a server) | Common RDF syntaxes | Readable reports with a summary, permalinks to reports, README badges, PlantUML shape diagrams | Uploads data; no editing; report is a separate page |
| ITB SHACL validator (European Commission, [web form](https://www.itb.ec.europa.eu/shacl/any/upload), [service](https://interoperable-europe.ec.europa.eu/collection/interoperability-test-bed-repository/solution/interoperability-test-bed/news/shacl-validation-service)) | SHACL validation against configured or uploaded shapes; owl:imports | No (server, REST/SOAP API, CLI, Docker) | JSON-LD, N-Triples, RDF/XML, Turtle | Official, configurable per profile (DCAT-AP and similar), machine APIs | Form-and-report workflow; no editor; uploads data |
| RDF Playground ([rdfplayground.dcc.uchile.cl](https://rdfplayground.dcc.uchile.cl/), [paper](https://www.aidanhogan.com/docs/rdf-playground.pdf)) | Teaching: Turtle in, graph drawing, SPARQL, OWL 2 RL reasoning, SHACL and ShEx | Mostly server (not verified) | Turtle | Only tool combining query, reasoning, shapes and a graph view | Turtle only; aimed at teaching; light diagnostics |
| YASGUI / YASQE / YASR ([Triply](https://triply.cc), [YASQE](https://cdn.jsdelivr.net/npm/yasgui-yasqe@2.11.22/index.html)) | SPARQL editor plus result viewer against endpoints | Yes (editor); queries go to the endpoint | SPARQL 1.1 | The de-facto SPARQL editor: highlighting, prefix completion from prefix.cc, class/property completion from LOV, tabs, table/chart/graph results ([FAO note](https://aims.fao.org/news/two-yasgui-new-libraries-yasque-yasr)) | Error checking is parse-only; completion calls third-party services; no SPARQL 1.2; formatter is cosmetic |
| Comunica web client ([comunica.dev](https://comunica.dev)) | Run SPARQL in the browser over files, endpoints, TPF | Yes | SPARQL 1.1 over most RDF syntaxes | Real federated query in the tab | Bare query box; no editor intelligence |
| Oxigraph JS ([npm](https://npmjs.com/package/oxigraph)) | In-memory SPARQL store compiled to WebAssembly | Yes | Turtle, TriG, N-Triples, N-Quads, RDF/XML; SPARQL 1.1 query and update | A Wasm query engine turtle-lsp could sit next to | Library, no hosted playground found |
| JSON-LD Playground ([json-ld.org](https://json-ld.org/)) | Expand, compact, flatten, frame, to N-Quads | Yes | JSON-LD | The reference for JSON-LD; permalinks | JSON-LD only |
| W3C RDF Validation Service ([w3.org/RDF/Validator](https://www.w3.org/RDF/Validator/)) | RDF/XML to triples and a graph image | No (servlet) | RDF/XML | Still linked from courses | Last modified 2006; RDF/XML only |
| WebProtégé ([webprotege.stanford.edu](https://webprotege.stanford.edu/)) | Collaborative ontology editing | No (server, account) | OWL/RDF imports and exports | Collaboration, forms | Not a text tool; account and server required (page content not retrievable) |
| LinkedDataHub ([source](https://github.com/AtomGraph/LinkedDataHub)) | Low-code knowledge-graph app platform | No (Docker stack with a triplestore, 8 GB suggested) | RDF via HTTP API | RDFa-aware document editing, apps | A platform you host, not a paste-and-check tool |

There is no browser tool that formats SPARQL with a real parser: searching
found only general SQL beautifiers and a command-line SPARQL formatter
([fichu](https://pypi.org/project/fichu)).

## What a state-of-the-art tool must have

### (a) Table stakes: every serious tool has these

- Paste, upload or drop a file; pick or detect the format.
- A syntax error with a line number.
- Syntax highlighting in the input.
- SHACL validation with a results list (data graph plus shapes graph).
- Copy and download of the output.

### (b) What the best ones have

- Everything in the tab, nothing uploaded (IDLab, Zazuko, RDF Play, SoWasm,
  JSON-LD Playground). Privacy is the main reason people pick these.
- Many formats, including RDF 1.2 (RDF Play) and canonical N-Quads (SoWasm).
- Completion of prefixes, classes and properties (YASQE, via prefix.cc and
  LOV, both network calls).
- Readable SHACL reports with a summary, permalinks and badges (SHACL Play).
- A graph or diagram view of the data or shapes (RDF Playground, Zazuko
  Sketch, SHACL Play).
- Result tables and charts for queries (YASR).
- Shareable state in the URL (JSON-LD Playground, SHACL Play).

### (c) What nobody has and turtle-lsp can offer

1. **Every problem, not the first.** Error recovery gives all syntax errors
   with exact ranges in one pass; every other tool stops at the first.
2. **Fixes, not only findings.** Quick fixes (declare `foaf:`, `lable` ->
   `label`, remove an unused prefix) applied with one click, previewed as a
   diff.
3. **Vocabulary checks offline.** Unknown and deprecated terms in 80+
   vocabularies with "did you mean", with no call to LOV or prefix.cc.
4. **SHACL results on the source.** Violations are diagnostics with ranges, so
   they can sit on the exact literal (`"forty"` on line 5), not in a detached
   report. SHACL Core and SHACL-SPARQL, passing 98/98 and 23/23 of the W3C
   suites; the JavaScript engines in the browser tools are Core only.
5. **Formatting you can trust.** The formatter provably keeps the graph (and
   the SPARQL algebra); the tool can say "same graph" next to the diff. No
   other browser tool formats SPARQL with a real parser.
6. **One tool for data, shapes and queries.** Turtle, TriG, N-Triples,
   N-Quads, SPARQL 1.1 and 1.2 query and update, with cross-file definition,
   references and rename between the data, shapes and query files.
7. **Editor intelligence in a web page.** Ontology-ranked completion, hover
   labels, label inlay hints and semantic highlighting, which YASQE has only
   for SPARQL and only with network calls.
8. **Conformance in public.** 1137/1137 W3C RDF syntax tests and 425/426
   SPARQL syntax tests, a claim no surveyed tool publishes.

Gaps to state honestly on the page: no RDF/XML or JSON-LD input yet, no format
conversion yet (planned), no query execution (pair with Oxigraph or Comunica
later), no network imports.
