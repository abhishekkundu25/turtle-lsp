# Fix the corpus-validation findings

Status: done in 0.3.0.

## Problem Statement

The public-corpus run found three things
a user opening well-known ontologies would hit:

1. Editing `qudt-units.ttl` (2.3 MB) takes 4.6 s per keystroke to re-check, and
   every request (formatting, hover) waits behind it.
2. The unknown-term warning fires on terms the user's own files define (1,099
   of 9,609 warnings in `qudt-units.ttl`) and on terms the bundled vocabulary
   snapshot is too old to know.
3. Brick shows 24 red SHACL errors that only exist because the QUDT
   vocabularies it imports cannot be resolved without network imports.

## Solution

1. Unknown-term diagnostics cost milliseconds on any file: the "did you mean"
   suggestion is computed once per distinct unknown term, and cheaply.
2. A term the workspace or a resolved import describes is never reported as
   unknown, whatever the bundled snapshot says.
3. When the workspace has unresolved imports, a SHACL violation whose result
   could change once they load is shown as a warning, with a note naming the
   unresolved imports. Other violations stay errors.

## User Stories

1. As an ontology author editing QUDT, I want typing to stay instant, so that
   the editor does not freeze for seconds on each key.
2. As an ontology author, I want formatting and hover to answer at once even in
   a file with thousands of warnings, so that one slow check does not block the
   editor.
3. As a vocabulary maintainer, I want terms my own file defines not to be
   reported as unknown in their namespace, so that I can edit the vocabulary
   itself without thousands of false warnings.
4. As a user of a vocabulary newer than the bundled snapshot, I want terms
   described by a file in my workspace or by a resolved `owl:imports` to count
   as defined, so that I can silence stale-snapshot warnings by providing the
   real vocabulary.
5. As a user with a genuine typo (`foaf:namee`), I want the warning and its
   "did you mean" fix to stay exactly as before.
6. As a Brick user without network imports, I want violations that depend on
   missing imports shown as warnings that name the missing imports, so that red
   errors mean my data is wrong, not that the server cannot see QUDT.
7. As a user with no unresolved imports, I want SHACL severities unchanged.
8. As a user whose shapes say `sh:severity sh:Warning` or `sh:Info`, I want
   those severities kept (never raised).

## Implementation Decisions

- **Unknown-term check (diagnostics, vocabulary module).** The suggestion
  search is memoised per (vocabulary, local name) for one diagnostics run;
  lowercase term names are computed once per vocabulary; candidates whose
  length differs from the unknown name by more than the allowed distance are
  skipped before the edit-distance computation. Message text, the quick fix
  data and the choice of suggestion stay identical (same tie-break: smallest
  distance, then shortest name).
- **Described terms are known.** Before reporting an unknown term, the check
  asks the knowledge index whether the IRI is described (by any workspace
  document or resolved import). If it is, no unknown-term diagnostic. The
  deprecated-term hint is unaffected.
- **SHACL and unresolved imports.** The SHACL job carries the IRIs of the
  workspace's unresolved imports. When that list is non-empty, a violation
  reported as error whose constraint is one that missing data can change
  (class, node, minCount, qualified counts, and any constraint whose focus or
  value is an IRI in a namespace described by none of the loaded documents) is
  downgraded to warning, and its message ends with a note naming up to three
  unresolved imports. Simplest acceptable rule if the precise one proves
  costly: downgrade every error-level violation while any import is
  unresolved; record which rule was built.
- No new settings.

## Testing Decisions

- Test external behaviour through the existing end-to-end LSP client
  (`test/*.test.mjs`) and the Rust unit tests already beside each module.
- Unknown-term: a generated document with 10,000 unknown-term uses of 2,000
  distinct locals in a bundled vocabulary must get its diagnostics within a
  budget (well under 1 s); a document defining `ex`-style terms in a bundled
  namespace gets no unknown-term warnings for them; a typo keeps its message and
  fix. Prior art: `test/lint.test.mjs`, unit tests in the diagnostics module.
- SHACL: a workspace with an unresolved import and a `sh:class` violation gets a
  warning naming the import; the same without the import gets an error. Prior
  art: `test/shacl-validate.test.mjs`.
- Corpus re-run (`npm run corpus`) before closing: `qudt-units.ttl` under
  100 ms after an edit, parser/formatter findings still zero.

## Out of Scope

- Refreshing the bundled vocabulary snapshot (dependency change).
- N-Triples memory use (needs profiling first).
- Resolving imports differently or enabling the network by default.

## Further Notes

- The formatter's safety check re-parsing without the document's base IRI
  (latent, unreachable from the server) is fixed alongside, as a one-line
  change.

## Departures from the plan

- The typo example was `foaf:nmae`, which never got a suggestion (a swap is two
  edits; the limit for four letters is one); the tests use `foaf:namee`.
- SHACL: the precise rule was built. The server also re-validates when import
  resolution changes. The first validation after indexing can run before
  imports resolve, so strict errors may show briefly before they soften.
