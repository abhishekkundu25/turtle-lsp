# Reference lenses count the uses in SPARQL files

Status: done (unreleased). Fixes issue #46. Builds on docs/specs/lsp-capability.md
(reference lenses) and docs/specs/workspace-query.md (an RDF term's
references include its uses in SPARQL files).

## Problem Statement

The "N references" lens above a subject counts fewer places than Find
References lists for the same term. In the playground, `id:acme` shows
"3 references", but Find References also lists its use in
`queries/move-to-acme.ru`. A lens that disagrees with Find References and
gives no hint of its rule looks broken.

The cause is that the lens and Find References take different paths:

- Find References (`ide::navigation`, `Session::references`) adds the term's
  uses in the workspace's SPARQL documents, open or closed.
- The lens (`features::lens::resolve`) only gets the knowledge index, which
  never holds SPARQL. The module comment says SPARQL was left out because
  reading every query file once per lens was thought too slow.

Every RDF format already counts: Turtle, TriG, N-Triples, N-Quads and
JSON-LD. Only `.rq`, `.sparql` and `.ru` files are missing.

## Solution

The lens lists what Find References lists without its declarations: one
search of the workspace, used by both. A resolved lens's count and
locations equal `textDocument/references` with `includeDeclaration: false`
on the same term.

## User Stories

1. As someone reading a vocabulary, I want the lens count to include the term's uses in SPARQL queries, so that the count tells me how much the term is used.
2. As someone reading a vocabulary, I want the lens count to include uses in SPARQL updates (`.ru`), so that a term an update writes is not shown as unused.
3. As a user, I want clicking a lens to list the same places as Find References without declarations, so that the two never disagree.
4. As a user, I want a query file I have not opened to count, so that the count does not depend on which tabs are open.
5. As a user, I want a query file I have open with unsaved edits to count as I see it, so that the count follows what I type.
6. As a user, I want a term's other descriptions (the same IRI as a subject in another file) still left out, so that the count stays a count of uses, as in other editors' reference lenses.
7. As a playground user, I want `id:acme` to show "4 references" in the published sample workspace, so that the example the issue names is right.
8. As a user of a large project, I want lenses to stay as fast to show as today, so that scrolling a vocabulary does not stall.
9. As a maintainer, I want one function that finds a term's uses across RDF and SPARQL, so that the next feature that needs them cannot drift from the other two.

## Implementation Decisions

- **One search.** The RDF-plus-SPARQL search in `Session::references` (the
  RDF term branch) becomes a `Session` helper that both
  `Session::references` and `Session::resolve_code_lens` call.
  `features::lens::resolve` takes the locations instead of the knowledge
  index, so the lens module no longer decides what counts.
- **What it deletes.** The module comment in `src/features/lens.rs` that
  documents the SPARQL exclusion, and the assertion in
  `code_lenses_count_references_in_turtle_and_run_a_query_file` that
  expects "1 reference" for a term `q.rq` uses (it becomes 2).
- **Cost.** `Workspace::sparql_documents` reads the SPARQL files once and
  afterwards only those written since. The first lens after startup pays
  the read; the rest pay a check per file. No new cache.
- **Declarations.** Unchanged: a description of the IRI in any file is left
  out, as Find References leaves it out when `includeDeclaration` is false.

## Testing Decisions

- Behaviour at the session seam, as in `src/ide/display.rs` tests:
  `a_reference_lens_counts_the_uses_in_sparql_files` (already written,
  failing) checks a closed `.rq` and a closed `.ru` are listed, the lens
  lists exactly what Find References finds, and the title reads
  "3 references".
- The existing lens test's expectation moves from "1 reference" to
  "2 references".
- The playground: a check in `web/test/lsp.test.mjs` (prior art: its
  `id:acme` sample) that the lens on `id:acme` in the published workspace
  resolves to "4 references".
- Performance: lens resolve timed before and after on a workspace of many
  query files, with nothing else running, with run count, spread and what
  limits the number.

## Principles

- fix-root-causes: the lens and Find References share one search, instead
  of the lens adding its own SPARQL lookup.
- subtract-before-you-add: the lens module stops taking the index and
  stops documenting an exclusion; the old assertion goes.
- explain-the-number: the lens resolve timing in the close-out.

## Non-functional budgets

- The keystroke rows of `bench/perf.mjs` do not move (nothing on the
  keystroke path changes).
- Resolving a lens in a workspace of 200 query files stays under 5 ms per
  lens after the first.

## Out of Scope

- Lenses in SPARQL files (only the "▶ Run query" lens, unchanged).
- Counting uses in RDF/XML (`.rdf`, `.owl`), which are import sources only;
  Find References does not list them either.
- Counting declarations.
- The workspace diagnostics work and the 31 October chaos failure.
