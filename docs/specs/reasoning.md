# Lightweight reasoning (RDFS, then a part of OWL RL)

Status: spec only, not scheduled.

## Problem Statement

An RDF model says much more than its triples spell out. A Manager is an
Employee because of `rdfs:subClassOf`; anything that `ex:worksFor` something
is a Person because of `rdfs:domain`. Today the editor knows only the types
that are written down (and the subclass closure for the type hierarchy). So
hover shows only the asserted type, completion ranks properties only by
asserted types, and data that states just its most specific type looks
poorer than it is. Modelling mistakes that only show through inference
(using `ex:worksFor` the wrong way round makes an organisation a Person) go
unnoticed. Users who validate with inference elsewhere (pySHACL's
`inference="rdfs"`) cannot reproduce that in the editor.

## Solution

An optional reasoning setting. When it is on, the language server derives
the facts that follow from a small, fixed set of rules (RDFS first: subclass,
subproperty, domain and range; later a part of OWL RL) and uses them where
types matter: hover lists inferred types with the reason for each,
completion ranks by them, the type hierarchy and label hints can mark them,
and SHACL can (as a separate opt-in) validate against them. Every inferred
fact can say why it holds, as a link to the line that caused it. Nothing
changes when the setting is off, which is the default.

## User Stories

1. As an ontology author, I want hover on a resource to list its inferred types, so that I see what my model implies about it.
2. As an ontology author, I want each inferred type to say which rule and which line produced it, so that I can check the reasoning.
3. As an ontology author, I want the line behind an inference to be a link, so that I can jump to the axiom that caused it.
4. As a data author, I want completion to rank properties by inferred types too, so that data typed only as `ex:Manager` gets the properties of `ex:Person`.
5. As a data author, I want an inferred type that surprises me (an organisation inferred to be a Person) to be visible, so that I notice a property used the wrong way round.
6. As an ontology author, I want a warning when a resource is inferred to belong to two classes declared disjoint, so that I catch contradictions early (OWL RL stage).
7. As a user, I want reasoning off by default, so that nothing changes unless I ask for it.
8. As a user, I want to choose the level (off, RDFS, RDFS plus the OWL RL part), so that I take only the inference I understand.
9. As a SHACL user, I want a separate switch to validate against inferred facts, so that I can match pySHACL's RDFS inference.
10. As a SHACL user, I want results that depend on an inference to say so, so that I can tell them from results on asserted data.
11. As a SHACL user, I want SHACL without inference to stay exactly as today when that switch is off, so that results match other validators.
12. As a user, I want the type hierarchy to show inferred subclass and subproperty links marked as inferred, so that I can tell them from declared ones.
13. As a user, I want label hints and semantic highlighting to keep showing asserted facts, so that the text view does not change under me.
14. As a user, I want inference to follow my edits (after a pause in typing, as SHACL does), so that what I see stays current.
15. As a user with a large workspace, I want reasoning skipped above a size limit with a note saying so, so that the editor stays fast.
16. As a user, I want inference to include loaded `owl:imports`, so that axioms from imported ontologies count.
17. As a user, I want rules that multiply facts without bound kept out (no `owl:sameAs` merging), so that reasoning cannot stall the editor.
18. As a user of the browser editor, I want the same reasoning there, so that both behave alike.
19. As a VS Code and Neovim user, I want it through the standard features (hover, completion, type hierarchy), so that no client changes are needed.
20. As a maintainer, I want the rules checked against the W3C entailment tests for the rules covered, so that the inference is correct.
21. As a maintainer, I want the server's speed with reasoning off unchanged, so that nobody pays for a feature they do not use.

## Implementation Decisions

- **Setting.** `turtle.reasoning`: `off` (default), `rdfs`, or `owl-rl-lite`.
  A separate `turtle.shacl.inference` (bool, default false) makes SHACL
  validate against asserted plus inferred facts. Both are sent with the rest
  of the `turtle` configuration and shown in the browser editor's Settings.
- **Rules, stage 1 (RDFS).** Subclass (types follow `rdfs:subClassOf`,
  transitively), subproperty (statements and `rdfs:subPropertyOf`
  transitively), domain (the subject of a statement gets the property's
  domain as a type) and range (an IRI or blank-node object gets the range as
  a type; literals are not typed by range). No axiomatic triples, no
  container membership, no `rdfs:Resource` typing of everything.
- **Rules, stage 2 (OWL RL part).** `owl:inverseOf`,
  `owl:SymmetricProperty`, `owl:TransitiveProperty`, `owl:equivalentClass`
  and `owl:equivalentProperty` (as subclass and subproperty both ways), and
  `owl:disjointWith` as a check (a diagnostic, never an inference). Not
  `owl:sameAs`, not property chains, not class expressions (restrictions,
  unions, intersections).
- **Engine.** A forward-chaining step over the knowledge index: rules apply
  until nothing new follows (semi-naive, so each round looks only at what
  the last one added). It runs off the request path, after a pause in
  typing and on open and save, like SHACL; inline in the browser build. Its
  result is a set of inferred facts kept beside the knowledge index, never
  merged into it, so every feature chooses asserted, inferred or both.
- **Provenance.** Each inferred fact keeps the rule and the facts it came
  from, back to source locations (file and line). Hover renders the first
  reason per inferred type as a link to that line, using the same
  `file#L<line>` links as "Described in".
- **Limits.** The same triple cap as SHACL (`turtle.shacl.maxTriples`) or a
  `turtle.reasoning.maxTriples` of its own; above it, reasoning is skipped
  and a status note says why. A cap on inferred facts per run stops a
  runaway model.
- **Consumers.** Hover (inferred types with reasons), completion ranking
  (inferred types join asserted ones), type hierarchy (inferred links
  marked), SHACL focus nodes and value types when `turtle.shacl.inference`
  is on (results caused by an inference say so in their message), and the
  disjointness diagnostic at stage 2. Semantic tokens, label hints,
  formatting and the lints for undescribed terms keep using asserted facts
  only.
- **Imports.** Loaded `owl:imports` take part like workspace files;
  unresolved imports contribute nothing.
- No change to the protocol or to clients.

## Testing Decisions

- A good test drives the server through the protocol, as the editor does,
  and checks what the user sees (hover text, completion order, type
  hierarchy items, SHACL diagnostics), not the engine's internals.
- **Conformance:** the W3C rdf-tests entailment cases for the RDFS rules
  covered (positive and negative), run as Rust tests; stage 2 adds cases
  per OWL RL rule implemented.
- **Protocol tests** (prior art: the Node end-to-end suite over stdio, and
  the hover and SHACL tests in it): hover lists inferred types with reasons
  and links; completion ranks by inferred types; a reasoning-off server
  answers exactly as today; SHACL with inference finds the violations pySHACL
  finds with RDFS inference on a shared sample, and SHACL without it is
  unchanged; edits update inferences after the pause; the size limit skips
  with a note; the disjointness diagnostic (stage 2).
- **Browser parity** (prior art: the web LSP parity tests): the same answers
  from the WebAssembly build.
- **Performance:** the benchmark is unchanged with reasoning off; with it on,
  the cost is measured and reported on the benchmark workspace.

## Out of Scope

- Full OWL (DL) reasoning: consistency of class expressions, classification,
  restrictions, property chains, `owl:sameAs`.
- Writing inferred facts into files ("materialise"), and a separate view
  listing all inferred triples (possible later).
- SHACL Advanced Features rules (a separate spec).
- Changing what other features show by default (semantic tokens, label
  hints, lints).

## Further Notes

- The main risk is surprise: `rdfs:domain` and `rdfs:range` infer, they do
  not check, and many users expect a check. Explaining every inferred fact,
  and keeping reasoning off by default, are what make that acceptable.
- Stage 1 alone (about a week) covers most of the value; stage 2 is worth it
  only if users ask.
- SHACL leaves entailment to the validator (`sh:entailment`); validating
  with inference must stay a visible opt-in so that results remain
  comparable with other validators.
