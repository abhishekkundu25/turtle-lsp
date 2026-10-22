# Editing feedback: SHACL while typing, import-aware prefix fix, progress

Status: done in 0.3.0.

## Problem Statement

SHACL results only change on save, so a violation typed into a file stays
invisible until the user saves, and a fixed one stays red. Adding a prefix for
a namespace that a workspace ontology defines needs a second manual edit to
import that ontology. Long work (indexing, import downloads, a large
validation) happens with no visible sign.

## Solution

1. SHACL validation also runs about a second after the user stops typing, on
   the unsaved text. Saving still validates at once.
2. The undeclared-prefix fix offers a second action that also adds the
   `owl:imports` of the ontology defining that namespace.
3. The server reports progress for indexing, import resolution and SHACL runs
   that take noticeable time, ending with a one-line summary.

## User Stories

1. As a shape author, I want a violation I just typed to appear without saving,
   so that I see mistakes as I make them.
2. As a data author, I want a fixed violation to disappear without saving, so
   that the editor reflects my buffer.
3. As a user on a slow machine or a large workspace, I want typing never to
   wait for validation, so that editing stays instant.
4. As a user who prefers save-time checks, I want a setting to turn typing-time
   validation off.
5. As an ontology author, I want "add prefix" to also import the ontology that
   defines the namespace when my file is an ontology that does not import it
   yet, so that one action makes the reference resolve.
6. As a user of a well-known vocabulary (FOAF, Dublin Core), I want no import
   offered, since bundled vocabularies are not imports.
7. As a user, I want to see when the server is indexing, downloading imports or
   validating for a while, so that I know why results are not there yet.
8. As a user, I want a short summary when validation finishes ("SHACL: 3
   violations in 2 files"), so that I know the result at a glance.
9. As a user whose editor does not support progress, I want nothing to change.

## Implementation Decisions

- **Typing-time validation.** After a document change, the main loop arms a
  validation deadline (1 s after the last change; each change pushes it back),
  using the same timer pattern as the existing described-terms debounce.
  When it fires, the existing validation entry point runs; the worker already
  supersedes older runs by generation. Save keeps validating immediately and
  clears the deadline. New setting `turtle.shacl.onType` (boolean, default
  true), read like the other SHACL settings and declared in the VS Code
  extension manifest.
- **Import-aware prefix fix.** For an undeclared prefix whose namespace is
  described by a workspace or catalog ontology (a document with an
  `owl:Ontology` subject whose terms are in that namespace), when the current
  document has an `owl:Ontology` subject that does not already import that
  ontology IRI, add a second quick fix "Add prefix … and import <ontology>"
  whose edit declares the prefix (same text as the existing fix) and adds an
  `owl:imports <ontology>` statement for the document's ontology subject. The
  existing fix is unchanged and stays first. No import fix for bundled
  vocabularies.
- **Progress.** Only when the client declares `window.workDoneProgress`: create
  a token (`window/workDoneProgress/create`), then `begin`/`report`/`end`
  through `$/progress`. Indexing: begin at start, end when merged. Imports:
  while a resolution pass with network or file loads is running. SHACL: begin
  only when a run has lasted 200 ms (no flicker for fast runs); end with
  "N violations in M files" or "no violations". No `window/showMessage`.

## Testing Decisions

- End-to-end tests through the existing LSP test client.
- Typing-time: editing a file into violation shows the SHACL diagnostic
  without `didSave`; editing back clears it; a burst of 20 edits 50 ms apart
  causes one validation (observable through one result publication after the
  burst); `onType: false` restores save-only behaviour.
- Prefix fix: a workspace with an ontology defining a namespace and an
  ontology file using an undeclared prefix for it gets both fixes; applying
  the second yields the prefix and the import; a file that already imports
  it, a file without `owl:Ontology`, and a bundled vocabulary get only the
  plain fix.
- Progress: a client declaring `window.workDoneProgress` receives create,
  begin and end for indexing; a slow SHACL run (large generated chain, as in
  existing tests) reports begin and an end message with the count; a client
  without the capability receives none.
- The benchmark (`npm run bench`) keystroke latency must not regress.

## Out of Scope

- SHACL rules (SHACL-AF).
- Downloading imports by default.
- `window/showMessage` notifications.
