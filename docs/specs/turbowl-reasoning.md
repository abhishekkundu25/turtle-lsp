# Reasoning and proofs through TurbOWL

Status: phase 1 in progress. Replaces the engine, provenance and
`owl:sameAs` decisions of [reasoning.md](reasoning.md); its user stories,
consumers, size limit, imports and browser parity stand. Handover:
[../handover-turbowl.md](../handover-turbowl.md). Tickets:
[../tickets/turbowl-reasoning/](../tickets/turbowl-reasoning/README.md).

## Problem Statement

An RDF model implies more than its triples spell out, and the editor shows
only what is written. A user cannot see that `ex:p1` is an `ex:Device`
because `ex:Pump rdfs:subClassOf ex:Device`, nor why, nor that two classes
declared disjoint now share a member. Users who reason elsewhere cannot do
it while they edit.

## Solution

With `turtle.reasoning` set to `default` or `complete`, the server hands the
workspace's triples (imports included) to TurbOWL, the owner's OWL 2 RL
reasoner, on a thread of its own after open, save and a pause in typing.
Hover on a resource lists its inferred types, each with its proof: the base
triples it rests on, one W3C rule per step, each base triple a link to its
file and line. TurbOWL's findings (contradictions, ill-typed literals)
become diagnostics on the lines of their terms, with the proof in the
diagnostic's message. The editor never waits on reasoning: a progress
indicator shows while it runs, a stale run's result is dropped, and a
workspace over the size limit is skipped with a note. `off`, the default,
changes nothing.

## User Stories

1. As an ontology author, I want hover on a resource to list its inferred types, so that I see what my model implies about it.
2. As an ontology author, I want each inferred type to come with a proof, so that I can check why it holds.
3. As an ontology author, I want each base triple of a proof to be a link to its file and line, so that I can jump to the axiom that caused it.
4. As an ontology author, I want each step of a proof to name its W3C rule (`cax-sco`, `prp-dom`), so that I can look the rule up.
5. As an ontology author, I want hover on a triple that holds through `owl:sameAs` to show the chain of sameAs links, so that I see which merge produced it.
6. As an ontology author, I want hover on an asserted triple's subject to still show its asserted facts as today, so that reasoning adds and never hides.
7. As an ontology author, I want a contradiction (a member of two disjoint classes) as a diagnostic on the lines of its terms, so that I catch it while editing.
8. As an ontology author, I want the contradiction's proof in the diagnostic, so that I see which lines caused it.
9. As a data author, I want ill-typed literals TurbOWL finds as diagnostics, so that I fix them where they are.
10. As a user, I want reasoning off by default, so that nothing changes unless I ask.
11. As a user, I want to choose `default` or `complete`, so that I take the inference I want; `complete` is labelled slower.
12. As a user, I want reasoning to run after a pause in typing, on open and on save, so that results follow my edits.
13. As a user, I want a progress indicator while reasoning runs, so that I know results are coming.
14. As a user, I want typing to stay as fast as before while reasoning runs, so that the editor never waits on it.
15. As a user, I want a stale run's results never shown, so that what I see matches my text.
16. As a user with a large workspace, I want reasoning skipped above a size limit with a note saying so, so that the editor stays responsive and I know why.
17. As a user, I want the size limit as a setting (`turtle.reasoning.maxTriples`), so that I can raise it.
18. As a user, I want imports included in reasoning, so that axioms from imported ontologies count.
19. As a user, I want proofs through imported files to link to where the import is cached, as "Described in" does, so that I can open them.
20. As a user, I want hover to answer within about 200 ms even when a proof is slow, saying it is still being worked out, so that hover never hangs; hovering again shows the proof.
21. As a browser-editor user, I want the same inferred types, proofs and diagnostics in the browser, so that both editors agree.
22. As a user of a repository's `turtle-lsp.json`, I want `turtle.reasoning` settable there, with the size limit clamped as SHACL's is, so that a cloned repository cannot make my editor slow.
23. As a maintainer, I want reasoning's correctness left to TurbOWL's oracles and proof checker, so that turtle-lsp tests only the wiring and what users see.
24. As a maintainer, I want the server's benchmark unchanged with reasoning off, so that the default costs nothing.

## Implementation Decisions

- **Dependency.** `turbowl` as a path dependency on `../turtle-reasoner/crates/turbowl`, `default-features = false`, with its `rdf-12` feature (TurbOWL ticket `rdf12/01`: a triple term is an opaque term). The browser engine takes the same dependency; TurbOWL runs on one thread there. This branch cannot build in CI until TurbOWL is published; light local checks stand in.
- **Setting.** `turtle.reasoning`: `off` (default), `default`, `complete`, mapped to `Switches::default()` and `Switches::complete()`. `turtle.reasoning.maxTriples`: the base size above which a run is skipped, default the same as SHACL's cap, clamped in an untrusted `turtle-lsp.json` like SHACL's.
- **A core module `reasoning`.** It owns:
  - the **base**: the workspace's documents (imports included) in a stable order, `(Source, uri)` as SHACL sorts them, made into oxrdf triples with the existing `rdf::Graph` conversion, so blank-node names are fixed for a run, and each base triple maps back to its document and triple (`Graph::origin`) and so to a file and line;
  - the **state** shared between the reasoning thread and hovers: the reasoner, the base's origin map, the generation it reflects, the explain cache; behind a read-write lock, written only by the reasoning thread;
  - the **worker**: one thread, newest job wins, generation counter, panics caught, as `features::shacl_validate::Worker`; `run_inline` for the embedded server;
  - **proof rendering**: a `Proof` to markdown, leaves as `[file:line](uri#Lline)` links as `features::hover::describe_iri` writes them.
- **Server wiring** follows SHACL's: a `server::reasoning` group (worker, feed, generation, diagnostics by URI), a typing timer and a progress timer, `Work::Reasoning` progress titled "TurbOWL: reasoning…", `Effects` flag on workspace changes, results feeding `DiagnosticInputs` and the closed-file workspace diagnostics. The skip note is an information diagnostic as `shacl-skipped` is, code `reasoning-skipped`.
- **Diagnostics.** A TurbOWL `Diagnostic` becomes one LSP diagnostic per document line its terms are the subject of, code `turbowl/<code>`, severity mapped, the proof as indented text in the message.
- **Hover.** On a resource: after today's hover, a section "Inferred" listing each inferred `rdf:type` with its proof, folded under the type. The explanation runs on a thread holding a read lock on the state, with the explain cache passed in and out; the hover waits at most 200 ms, then says the proof is still being worked out, and the search finishes in the background so the next hover is instant. While a run is writing the state, hover says reasoning is running. The browser runs it inline, without the time limit.
- **Off is free.** With `off`, no thread starts, no state exists, and every answer is as today.

## Testing Decisions

- Test what the user sees over the protocol, not the module's internals. Prior art: `test/shacl-validate.test.mjs` (debounce, size cap, progress, stale runs), `test/knowledge.test.mjs` (hover), `web/test/lsp.test.mjs` (browser parity).
- New `test/reasoning.test.mjs`: hover shows an inferred type with its proof and a working `#L` link; a type through `owl:sameAs` shows the chain; a disjointness contradiction is a diagnostic with its proof; an edit updates the result after the pause; the size limit skips with the note; `off` answers hover and diagnostics exactly as today.
- Unit tests in `reasoning`: the base is the same for the same workspace in any document order; a base triple maps back to its line; proof markdown.
- Settings: the manifest test (`editors/vscode/test/manifest.test.mjs`) and the settings unit tests cover the new keys.
- Browser: one parity case in `web/test/lsp.test.mjs` (inferred type in hover).
- Light verification (owner's choice): format, clippy, the touched unit tests and the new Node test file; no benchmark runs in this phase.

## Out of Scope

- Completion ranking by inferred types, inferred links in the type hierarchy, and SHACL with inference: phase 2, a separate spec.
- A "Show why" command, and hover on an arbitrary inferred non-type triple.
- Cancelling a run halfway (TurbOWL cannot today); a stale result is dropped instead.
- `complete`'s slow explanations on Brick (TurbOWL's ticket explain/06).
- Publishing TurbOWL or pushing either repository.

## Principles

- redesign-from-first-principles: `reasoning.md`'s stored-provenance engine is replaced, not patched; the spec and README say so.
- foundational-thinking, model-the-domain: the `reasoning` module's base, origin map and shared state come first (ticket 01) and every later ticket reads them.
- boundary-discipline: settings are parsed and clamped in `settings.rs`; TurbOWL's terms are converted at the `reasoning` module's edge, never past it.
- separate-before-serializing-shared-state: only the reasoning thread writes the state; hovers read under the lock and hand the cache back.
- make-operations-idempotent: a run on an unchanged base converges to the same state; a crashed run leaves the previous result and the next job retries.
