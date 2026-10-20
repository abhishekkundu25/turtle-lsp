# JSON-LD phase 2: editing inside JSON-LD

Status: ready-for-agent. Follows docs/specs/jsonld.md (phase 1, landed in 0.6.0).

## Problem Statement

Since 0.6.0, turtle-lsp reads JSON-LD: diagnostics, SHACL, and JSON-LD data
in completion, hover and references from Turtle and SPARQL. Writing JSON-LD
is still unassisted:
- typing a key offers nothing, although the context defines the terms;
- go to definition on a term doesn't reach its `@context` entry;
- references from a key miss the term's definition and its other uses;
- renaming a term or a class written in JSON-LD is either impossible or would
  write Turtle syntax (`<...>`, prefixed names) into JSON;
- `turtle-lsp format` and the editor leave JSON-LD unformatted;
- JSON-LD gets only the editor's JSON colours, which can't tell keywords,
  properties and classes apart;
- the outline lists node objects flat, even inside named graphs.

## Solution

Phase 2 makes the editing features work inside `.jsonld` documents, with the
same quality bar as Turtle: exact ranges, workspace-wide where the IRI is
shared, and never an edit that changes the graph by accident.

- **Completion** in keys and `@type` values, from the active context at the
  cursor, `@vocab` terms the workspace describes, and known prefixes.
  Completing a compact IRI whose prefix the context lacks also adds the prefix
  to the `@context` (the idea comes from SWLS, credited).
- **Go to definition** from a term to its `@context` entry, in the same
  document or in a workspace context file.
- **References** from a term: its definition, its uses, and the IRI's uses in
  other documents.
- **Rename:**
  - a term or keyword alias: its `@context` entry and every use, nothing else;
  - an IRI, from any syntax: every JSON-LD use rewritten in its own form
    (compact, `@vocab`-relative, absolute, or through a term's definition).
- **Formatting** in the editor, `turtle-lsp format` and the browser, with the
  same JSON value guaranteed.
- **Semantic tokens** for keywords and aliases, properties, and classes.
- **Outline** nesting a named graph's nodes under the graph.

## User Stories

1. As a JSON-LD author, I want completion in a key to offer the terms of the active context, so that I don't have to look the context up.
2. As a JSON-LD author, I want completion to offer the context in force at the cursor, including scoped and nested contexts, so that suggestions are valid where I type.
3. As a JSON-LD author, I want completion in a key to offer properties the workspace describes under `@vocab`, so that I can use my own vocabulary without defining terms.
4. As a JSON-LD author, I want completion in an `@type` value to offer classes, so that I pick an existing class rather than inventing one.
5. As a JSON-LD author, I want compact IRIs completed after a prefix (`schema:`), so that I can use a vocabulary through its prefix.
6. As a JSON-LD author, I want completing a prefix the context lacks to add it to my `@context`, so that the result is valid JSON-LD at once.
7. As a JSON-LD author, I want completion to keep working while my JSON is half typed, so that it helps exactly when I need it.
8. As a JSON-LD author, I want completion items labelled with the IRI and the label the workspace gives them, so that I choose the right one.
9. As a JSON-LD author, I want go to definition on a term to jump to its `@context` entry, so that I can see what it means.
10. As a JSON-LD author, I want go to definition on a term from a context file in my workspace to open that file at the entry, so that shared contexts are navigable.
11. As a JSON-LD author, I want go to definition on a term from a bundled or remote context to show where the IRI is described, so that I still land somewhere useful.
12. As a JSON-LD author, I want go to definition on a keyword alias to jump to its `@context` entry, so that aliases aren't magic.
13. As a JSON-LD author, I want references on a term to list its definition, every use in the document and the IRI's uses in other files, so that I see its full impact.
14. As a Turtle author, I want references on an IRI to include its JSON-LD uses, whatever form they are written in, so that mixed workspaces are one graph.
15. As a JSON-LD author, I want document highlights on a term to mark its definition and every use, so that I can scan a document.
16. As a JSON-LD author, I want to rename a term, changing its `@context` entry and every key that uses it, so that the data stays the same graph.
17. As a JSON-LD author, I want renaming a term to leave other contexts' terms with the same name alone, so that scoped contexts aren't broken.
18. As a JSON-LD author, I want to rename a keyword alias (`"id": "@id"`), so that aliases are refactorable too.
19. As a JSON-LD author, I want renaming a term from a bundled or remote context to be refused with a clear message, so that I'm not left with a half-applied edit.
20. As a Turtle author, I want renaming an IRI to rewrite its JSON-LD uses in their own form, so that `ex:Widget`, `Widget` under `@vocab` and the full IRI all stay valid JSON-LD.
21. As a Turtle author, I want renaming an IRI that a JSON-LD term maps to, to rewrite the term's definition, not every key, so that the document keeps its terms.
22. As a Turtle author, I want a rename that can no longer be written `@vocab`-relative to fall back to a compact or absolute IRI, so that the rename never changes the graph silently.
23. As a JSON-LD author, I want prepare-rename to show exactly the key or value I am renaming, so that I know what will change.
24. As a JSON-LD author, I want to format a document in the editor, so that compact or messy JSON-LD becomes readable.
25. As a CI maintainer, I want `turtle-lsp format --check` to cover `.jsonld`, so that formatting is enforced like Turtle's.
26. As a JSON-LD author, I want formatting to keep key order, number spelling and string escapes, so that only whitespace changes.
27. As a JSON-LD author, I want formatting to refuse a document with syntax errors, so that a broken file is never rewritten.
28. As a JSON-LD author, I want formatted text to stay as it is when formatted again, so that format-on-save is stable.
29. As a browser editor user, I want the same formatting in the browser, with its "same graph" guarantee, so that the toolbox and the editor agree.
30. As a JSON-LD author, I want keywords and aliases, properties, and classes coloured differently, so that I read structure at a glance.
31. As a JSON-LD author, I want the outline to nest a named graph's nodes under the graph, so that datasets are navigable.
32. As a JSON-LD author, I want every feature to work without network access, using bundled and cached contexts, so that offline editing is complete.
33. As a JSON-LD author on a large file, I want these features to keep keystrokes under the phase-1 budget, so that editing stays fast.
34. As a maintainer, I want every phase-2 pending test promoted, so that `npm test` reports zero pending JSON-LD work.

## Implementation Decisions

**One shared source index (the prefactor).** Phase 1's expansion already knows,
for each string in the document, the active context and what the string
expands to. It keeps only part of that (`refs`, `used_terms`). Phase 2 needs
the rest, so expansion records a JSON-LD source index on the parsed document:
- **Key and value uses:** every key, `@type` value and `@id` value, with its
  span (without quotes), its expanded IRI and how it is written:
  - a keyword, or an alias of one (with the alias's definition);
  - a term (with its definition);
  - a compact IRI (with the prefix length);
  - `@vocab`-relative;
  - absolute.
- **Term definitions:** each definition the document's contexts make, with the
  term's key span, the IRI's span and the document it is written in. That's
  this document, a workspace context file (its URL), or none for bundled and
  remote contexts.
- **Scopes:** the span of each JSON object with the active context that
  applies inside it, so features can ask "which context is in force here".

The index is built during the expansion that already runs. It must not add
measurable time to the phase-1 benchmark rows.

**Features read the index.** Each editor feature gets a JSON-LD branch that
reads the index, beside its Turtle code. The shared parse model and its `Form`
enum stay as they are, since many Turtle features match on them.

**Completion.**
- **Context:** the innermost scope containing the cursor. JSON being typed is
  usually broken, so the scope comes from phase 1's recovering parser, and
  from the nearest enclosing object when the cursor's object didn't parse.
- **Keys:** the context's terms, compact IRIs of its prefixes, and local names
  of workspace properties under `@vocab`.
- **`@type` values:** classes the same way.
- **Missing prefix:** completing `prefix:` for a prefix the context doesn't
  define adds the prefix to the `@context` object as an additional edit, using
  the same prefix catalogue as Turtle completion. Credit SWLS (MIT, IMEC –
  IDLab – UGent) for the idea in a comment and the CHANGELOG. No code is
  copied.
- **Ranking and labels:** the same ranking and resolve step as Turtle.

**Definition and references.**
- A term goes to its definition's key, in this document or in a workspace
  context file.
- A term from a bundled or remote context falls back to where the workspace
  describes its IRI, which is phase 1's behaviour.
- References on a term: its definition, its uses in this document, and the
  IRI's uses in the workspace.
- References on an IRI from any syntax already include JSON-LD uses; that
  stays as it is.

**Rename.**
- **Terms and keyword aliases:** rename the definition's key and every use
  whose active context resolves to that same definition. A term defined in a
  workspace context file is renamed there and in each workspace document using
  that file. A term from a bundled or remote context is refused, with a
  message naming the context.
- **The new name** must be a valid term: not a keyword, not empty, and no
  colon unless it's meant as a compact IRI. Anything else is refused.
- **IRIs:** the existing workspace rename asks each document for its edits. A
  JSON-LD document answers from the index, writing the new IRI per use:
  - **compact:** shortened with the document's context prefixes;
  - **`@vocab`-relative:** kept relative when the new IRI is still under
    `@vocab`, otherwise compact or absolute;
  - **absolute:** absolute, never in angle brackets;
  - **through a term:** the term's definition IRI is rewritten once (in its
    own form), and the keys are left alone.
- Every edited document must give the same graph apart from the renamed IRI.
  The tests check this through toRdf.

**Formatting.**
- **Layout:** a JSON pretty-printer over phase 1's parse tree. Two-space or
  tab indentation from the editor options, one member per line, and arrays and
  objects inline only when empty. This is the layout of `JSON.stringify` with
  an indent, which is what most JSON users expect.
- **What's kept:** the source text of every scalar, so number spellings and
  escapes stay, and key order.
- **Refusals:** a document with JSON syntax errors is not formatted.
- **Check:** the result must parse to the same JSON value, and so the same
  graph. Through the batch module this gives the guarantee "same graph", so
  the editor, the CLI and the browser engine share it.

**Semantic tokens.** These use the existing token types (no new legend
entries):
- `keyword` for keywords and their aliases, in keys and values;
- `property` for keys that expand to properties;
- `class` for `@type` values;
- the existing deprecated and readonly modifiers where Turtle uses them.

**Outline.** A node object with an `@id` and an `@graph` becomes the graph's
symbol. The node objects in its `@graph` are its children, as TriG named
graphs already appear.

**Editors.**
- **VS Code:** keeps the JSON grammar. Semantic tokens colour on top of it.
- **Neovim:** gets them through the LSP as it is.
- **Browser:** only formatting goes through the engine. The other features
  come with the language server and are out of scope there, as for Turtle.

## Testing Decisions

**What a good test is.** It asserts what a user sees through the LSP, the CLI
or the browser engine: ranges, edits and the resulting text. It never asserts
the shape of the index.

**Acceptance.** The 14 phase-2 pending tests (13 Node LSP and CLI tests, and 1
browser-engine test) are promoted one by one by the ticket that makes each
pass.

**Added tests:**
- completion in a half-typed key, and in a scoped context;
- rename refused for a bundled-context term and for an invalid new name;
- a term renamed from a workspace context file;
- an IRI rename that leaves `@vocab`, so the use becomes compact;
- formatting refused on a syntax error;
- semantic tokens on an alias used as a value.

**Graph checks.**
- Rename and formatting tests compare toRdf before and after (as N-Quads,
  with blank nodes canonicalised), using the oracle helper from phase 1.
- The real-world corpus gets a format round trip: every document that parses
  without errors formats to the same graph, and formats again to the same
  text.

**Unit tests** cover the index at the JSON-LD module seam: the expected uses
and definitions for a document with scoped contexts, aliases and a context
file.

**Budgets:** the phase-1 rows, with keystroke-to-diagnostics still under
10 ms, plus a completion and formatting timing on the 30,000-line file.

**Prior art:**
- the phase-1 LSP tests and fixtures, and the CLI helpers for `format`;
- the Turtle formatter's idempotence and same-graph tests;
- the corpus oracle.

## Out of Scope

- **Other JSON-LD algorithms:** compaction, flattening, framing, fromRdf, and
  converting between Turtle and JSON-LD.
- **Code actions specific to JSON-LD:** for example, extracting a term into the
  context, or turning compact IRIs into terms. That's a later phase.
- **Inlay hints** on JSON-LD keys (the expanded IRI shown inline).
- **Editing bundled or remote contexts.**
- **Keeping custom layout:** formatting does not preserve hand-aligned
  arrays.
- **Other editors:** JetBrains and Emacs packages.

## Further Notes

- **Build order:** the index is the one piece every feature needs. Formatting
  and the outline don't need it and can start at once.
- **Rename is the risky part.** Its tests check the graph, not only the
  edited text.
- **SWLS** (MIT) was reviewed for reusable code. Its JSON-LD rename replaces
  every occurrence of a term's text, which is the behaviour these tests reject.
  Only the missing-prefix completion idea is taken.
