# SHACL-SPARQL constraints

## Problem Statement

Shapes that express business rules as SPARQL (`sh:sparql` with
`sh:SPARQLConstraint` and `sh:select`) are silently skipped: a project whose
checks are mostly SPARQL sees "no problems" in the editor while its own
validator reports violations. SPARQL-based targets (`sh:target` with
`sh:SPARQLTarget`) are skipped the same way.

## Solution

Validation on save and while typing runs SPARQL-based constraints with the
semantics of the SHACL-SPARQL specification, alongside SHACL Core, and shows
their results like any other SHACL result. Query mistakes in shapes are
reported as you type.

## User Stories

1. As a shape author, I want `sh:sparql` constraints on node and property shapes
   to produce results, so that my SPARQL business rules are checked in the
   editor.
2. As a shape author, I want `$this`, `$value`, `$PATH`, `$currentShape` and
   `$shapesGraph` pre-bound as the specification says, so that my queries mean
   what they mean in any conforming validator.
3. As a shape author, I want `sh:prefixes` (with `sh:declare`) to supply the
   query's prefixes, so that I do not repeat PREFIX lines.
4. As a shape author, I want `sh:message` templates such as `{?value}` and
   `{$this}` filled from the solution, so that messages name the offending
   values.
5. As a shape author, I want `sh:deactivated true` on a constraint or shape to
   switch it off.
6. As a shape author, I want `sh:severity` on the shape to apply to SPARQL
   results like any other.
7. As a shape author, I want SPARQL-based targets to select focus nodes.
8. As a shape author, I want a query that is malformed, uses an undeclared
   prefix, or uses constructs SHACL-SPARQL forbids (MINUS, VALUES, SERVICE,
   nested SELECT with the pre-bound variables, as listed by the specification)
   reported in the shapes file as I type, not as a silent skip.
9. As a user of a large dataset, I want SPARQL validation to stay in the
   background, so that typing never waits for it.
10. As a user, I want results identical to a reference validator's on the same
    data and shapes.

## Implementation Decisions

- SPARQL parsing with `spargebra` and evaluation with `spareval`, both with
  SPARQL 1.2 enabled. The validation graph is exposed to the evaluator through
  an adapter over the existing SHACL graph, so no second copy of the data.
- Pre-binding follows the specification: the query is evaluated with the
  pre-bound variables substituted (or provided as initial bindings), never by
  string concatenation.
- SPARQL constraints run in the existing validation worker with the existing
  cancellation; a cancelled run stops between queries.
- Result shape: focus node, path (property shapes), value (from `?value` or the
  focus node), source shape, source constraint, component
  `sh:SPARQLConstraintComponent`, severity, messages.
- Custom SPARQL-based constraint components (`sh:ConstraintComponent` with
  validators) are built after the constraints if time allows; otherwise listed
  as known deviations in the conformance test.
- As-you-type lint: parse each `sh:select` / `sh:ask` in shapes with its
  `sh:prefixes`; report syntax errors and forbidden constructs on the query
  literal.

## Testing Decisions

- Seam 1: the W3C SHACL-SPARQL suite (vendored) through the same conformance
  harness as SHACL Core; target 100%, every deviation listed with a reason.
- Seam 2: end-to-end through the LSP client: a SPARQL violation appears on the
  offending triple after save; a message template is filled; a malformed query
  is reported in the shapes file as you type.
- Validation outside the test suite: results compared with a reference
  validator's on the same data and shapes for real-world simulation graphs;
  keystroke latency unchanged (`npm run bench`).

## Out of Scope

- SHACL rules (`sh:rule`), SHACL functions, `sh:ask`-based validators beyond
  what custom components need.
- Running SPARQL against named graphs other than the data graph.
