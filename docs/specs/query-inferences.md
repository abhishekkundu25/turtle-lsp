# SPARQL queries over inferred triples

Status: done. Tickets:
[../tickets/query-inferences/](../tickets/query-inferences/README.md). Builds on [workspace-query.md](workspace-query.md) (the
query feature and its four ways in) and
[turbowl-reasoning.md](turbowl-reasoning.md) (TurbOWL, the base, the rule
sets). One of that spec's phase 2 directions ("SHACL with inference" is
the other); this spec covers queries only.

## Problem Statement

A query runs over the triples written in the workspace's files and nothing
else. With reasoning on, hover says that `id:alice` is an
`ex:Organisation` (Bob `ex:worksFor` her, and the range of `ex:worksFor`
is `ex:Organisation`), yet `SELECT ?x WHERE { ?x a ex:Organisation }`
does not return her. The editor and the query disagree about the same
workspace. A user who wants the answer the model implies has to export the
data to GraphDB, Stardog or Jena and turn their reasoning on there.

## Solution

Every way a query runs gets one choice, **inferred triples**: `off` (the
default, today's results), `default` or `complete`, the same rule sets as
`turtle.reasoning.rules`. With `default` or `complete`, the query reasons
over its own snapshot of the workspace with TurbOWL, then runs over the
written triples plus the inferred ones, in the one default graph. The
answer says that it includes inferred triples, how many, and with which
rules, and says so too when they were left out (a workspace over the
reasoning size limit). This is what GraphDB's "Include inferred data in
results" and Stardog's per-query reasoning do; turtle-lsp offers it on the
command line, in the editors' "▶ Run query", in `turtle/query` and in the
browser editor's query pane.

## User Stories

1. As an ontology author, I want a query to return the resources my model implies are of a class, not only those typed so, so that the query agrees with hover.
2. As an ontology author, I want inferred property values (through `owl:inverseOf`, a transitive property, a property chain, a subproperty) to match a query's triple patterns, so that a query follows the model as a reasoner would.
3. As an ontology author, I want `owl:sameAs` merges to apply in queries, so that facts written about one name are found under the other.
4. As a user, I want inferred triples off by default, so that existing queries return exactly what they return today.
5. As a user, I want to choose `default` or `complete` per query run, so that I take the inference I need; `complete` is labelled slower.
6. As a user, I want a setting (`query.reasoning`) for the editors' default, so that I do not choose on every run.
7. As a command-line user, I want `turtle-lsp query --reasoning default q.rq`, so that scripts and CI can query the implied data.
8. As a tool author using `turtle/query`, I want a `reasoning` parameter, so that my extension gets the implied data without its own reasoner.
9. As a browser-editor user, I want an "Inferred" choice in the query pane's Run bar, on by default as reasoning is in the browser editor, so that queries agree with hover from the first visit and I can flip between written and implied results.
10. As a user, I want the results to say they include inferred triples, with the count and the rules, so that I never mistake implied results for written ones.
11. As a user, I want the query to reason over the same snapshot it queries, unsaved edits included, so that its inferences always match its data, whatever the editor's background reasoning is doing.
12. As a user, I want the editor's own reasoning setting (`turtle.reasoning.rules`) to stay independent of the query's choice, so that I can query with inferences without hover reasoning on, and the other way round.
13. As an editor user, I want axioms from imported ontologies to count in a query's inferences, as they count in hover, so that a class from an imported vocabulary infers the same in both.
14. As a user, I want the imported ontologies' own triples to stay out of the query's data, as today, so that turning inferences on adds only what the model implies, not the whole imported vocabulary.
15. As a user with a large workspace, I want the query to run without inferences above `turtle.reasoning.maxTriples`, saying so in its answer, so that a query never waits on an oversized reasoning run.
16. As a user, I want reasoning's time to count against the query's timeout, so that `query.timeoutSeconds` still bounds the whole run.
17. As a user, I want a query cancelled during reasoning to stop as soon as reasoning ends, without running the query, so that cancelling is still worth doing.
18. As a user of a workspace with contradictions, I want the query to still run over the closure, and the answer to say how many findings TurbOWL reported, so that I am told the model is inconsistent without losing the results.
19. As a user, I want blank nodes in inferred triples to be the same blank nodes as in the written data, so that a pattern joining written and inferred triples through a blank node matches.
20. As a user, I want CONSTRUCT and DESCRIBE to include inferred triples too, so that I can export the implied graph as Turtle.
21. As a user, I want ASK to answer over the implied data, so that a script can check "does the model imply this".
22. As a user of TriG, N-Quads and JSON-LD, I want inferred triples in the same single default graph as every written triple, so that nothing about named graphs changes.
23. As a user running a SPARQL UPDATE, I want updates to keep working on written triples only, so that an update never writes an inferred triple into my files.
24. As a user, I want the same answers in the browser editor as natively, so that both agree.
25. As a user of a repository's `turtle-lsp.json`, I want `query.reasoning` settable there, so that a project can make implied answers its default.
26. As a maintainer, I want the query feature unchanged and no TurbOWL work done when inferred triples are off, so that the default costs nothing.
27. As a maintainer, I want the inferred triples to come from TurbOWL as they are, internal ones included (types of unnamed restriction classes, `x owl:sameAs x`), as GraphDB and Stardog show them, so that turtle-lsp does not decide which inferences are worth seeing.

## Implementation Decisions

- **One seam: the query feature's entry point.** The core query function every path calls (`query::run`) gains one input, the rule set for inferred triples (`off`, `default`, `complete`), and its `Answer` gains a description of what was added: none, added (inferred count, rules, findings count), or skipped (base size against the limit). The command line, "▶ Run query", `turtle/query` and the browser's query worker pass the choice through; none of them reasons on its own.
- **The query reasons over its own snapshot.** It does not read the editor's background reasoning state. That state can be stale while the user types, does not exist with `turtle.reasoning.rules` off, and does not exist at all on the command line or in the browser's separate query worker. Reasoning the snapshot costs one TurbOWL run per query (about 18 ms for 180,000 triples, the base included, as measured in the base memory work), bounded by the size limit, and makes the inferences match the queried data by construction.
- **The base is the reasoning feature's.** The query builds the `reasoning` module's base from its documents plus the imports its path has, so blank-node names are the base's. A test from the base work already checks that the base names blank nodes exactly as the query graph does (`rdf::Graph` and its dataset), so inferred triples join written ones through blank nodes. The editors' paths ("▶ Run query", `turtle/query`) pass the workspace's imports, as background reasoning reads them, so imported axioms count. The command line and the browser query worker load no imports for a query today (the batch query reads the files given), so their inferences use those files only.
- **What is added.** Only TurbOWL's inferred triples (`Reasoner::inferred`) are added to the query graph, never the imports' own triples: the query's data stays the documents it reads, plus what reasoning implies. The query graph gains a way to take extra triples that are not from a document's parse; it holds them in the one default graph.
- **Rules and limits.** The rule sets map as for `turtle.reasoning.rules` (`Switches::default()`, `Switches::complete()`). Above `turtle.reasoning.maxTriples` the query runs without inferences and the answer says skipped. Reasoning time counts against the query's timeout: the clock is checked before reasoning and right after it, and during the run a thread beside it trips TurbOWL's cancel handle (`Reasoner::reason_cancellable`, checked before each round) when the query's timeout passes or its cancel flag is set, so a long run stops partway (added after TurbOWL's cancellation reached its `main`). The browser has no threads, so its runs are not stopped partway. A cancelled or timed-out query does not evaluate.
- **Findings do not block.** The query never searches proofs. It counts TurbOWL's findings and reports the count; the closure of an inconsistent workspace is queried as it is, as OWL RL engines do.
- **Settings and parameters.** A new setting `query.reasoning` (`off` default, `default`, `complete`), settable in `turtle-lsp.json`, read by "▶ Run query" and the browser pane. The browser editor sets it to `default` in its own settings, as it sets `turtle.reasoning.rules`: it is a showcase, and its queries should agree with its hover from the first visit. `turtle/query` gets a `reasoning` parameter, its default the setting. The command line gets `--reasoning <off|default|complete>`, default `off`. The setting is independent of `turtle.reasoning.rules`.
- **What the user sees.** "▶ Run query"'s results document and the browser pane's results header gain one line: "Includes N inferred triples (TurbOWL, default rules)", plus "TurbOWL reported K findings" when K > 0, or "Inferred triples skipped: the workspace has N triples, over the reasoning limit of M". The command line prints the same line to stderr, so stdout stays the results. `turtle/query`'s JSON answer gains an `inferred` member (`{"rules", "triples", "findings"}`, or `{"skipped": {"triples", "limit"}}`), absent when off.
- **Updates.** SPARQL UPDATE paths are unchanged: no inferred triples, no new parameter.
- **Off is free.** With `off`, no base is built, TurbOWL is not called, and every answer is byte-identical to today's (the answer has no `inferred` member and no extra line).

## Testing Decisions

- Test what the user sees at the highest seam, the query feature's entry point, then once per way in over its own protocol. Do not test the base or TurbOWL again: the base has its tests, and TurbOWL's correctness is its oracles'.
- Core (prior art: the query module's tests): with `off`, answers are identical to today's on the existing query tests; with `default`, a class inferred through a range, a subclass chain, an inverse, a transitive property, a property chain and an `owl:sameAs` merge each match; a pattern joining written and inferred triples through a blank node matches; imported axioms infer but imported triples are not in the results; CONSTRUCT includes inferred triples; ASK answers over them; over the size limit the answer is skipped and the results are today's; a timeout during reasoning stops before evaluation; a workspace with a contradiction still answers and reports its findings count.
- Command line (prior art: `test/query-cli.test.mjs`): `--reasoning default` returns the inferred row and prints the note to stderr; no flag is unchanged.
- `turtle/query` (prior art: `test/query-request.test.mjs`): the `reasoning` parameter, the `inferred` member, the skipped form, and the default taken from `query.reasoning`.
- "▶ Run query" (prior art: `test/query-lsp.test.mjs`): the results document includes the inferred row and the note when `query.reasoning` is on.
- Browser (prior art: `web/test/lsp.test.mjs`, `web/test/editor-query.test.mjs`): the query worker's answer equals the native one with inferences; the pane's Inferred choice changes the results and shows the note.
- Settings: the manifest and settings tests cover `query.reasoning`, as for the reasoning keys.

## Principles

- **redesign-from-first-principles:** the query's dataset is redesigned as "documents plus what they imply", with the choice an input of the one query entry point, not a flag bolted onto each of the four paths.
- **foundational-thinking, model-the-domain:** the answer's note is one sum type (off, added, skipped), defined in the core ticket before any path uses it.
- **type-system-discipline:** the rule set is the existing `Reasoning` enum, never a string past the boundary; the note's variants are matched exhaustively in each renderer.
- **boundary-discipline:** `--reasoning`, the `reasoning` request parameter and `query.reasoning` are parsed once where they enter; inside, the typed choice is trusted.
- **separate-before-serializing-shared-state:** the core ticket adds the settings field and the batch plumbing, so the three path tickets edit disjoint files and run together.

## Out of Scope

- Marking which result rows depend on inferred triples, or a proof per row.
- Filtering inferred triples (hiding restriction-class types, reflexive `owl:sameAs`): TurbOWL's output is shown as it is.
- Reusing the editor's background reasoning result for queries.
- Inferences in SPARQL UPDATE, and writing inferred triples to files.
- Hover with inferred types in SPARQL files (`.rq`, `.ru`): not what other SPARQL tools do; the query results are where reasoning shows.
- SHACL validation with inference: the other phase 2 direction, a separate spec.
- Named graphs of inferred triples (an inferred graph apart from the default graph).

## Further Notes

- Verification is light on timing (owner's choice, 2026-11-08): the budget is only that `off` changes no answer and builds nothing; no benchmark run is required.
- Prior art in other tools: GraphDB's "Include inferred data in results" toggle, Stardog's reasoning flag per query, Jena and RDF4J querying an inferencing model or store. None shows inferred types in a query editor's hover; all put reasoning in the results.
- The base now costs about 5 MB and 3 ms per 180,000 triples (the base memory work), so a reasoning run per query is cheap. If a workspace near the size limit makes repeated queries slow, caching the last query snapshot's reasoner by workspace version is the next step, measured first.
