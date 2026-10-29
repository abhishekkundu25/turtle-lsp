## Problem Statement

People who write ontologies make small curation mistakes that the file's syntax does not catch, and turtle-lsp says nothing about them today:

- Two classes or properties get the same label, usually from copying a block and forgetting to rename it. Anyone who searches by name, and every tool that shows labels (hover, completion, inlay hints, Protégé), can't tell them apart.
- A label has a stray space at the start or end. Search, matching and sorting treat `"Pump "` and `"Pump"` as different.
- A team retires a term with `owl:deprecated true`, but other files keep using it. turtle-lsp flags deprecated terms only from its bundled vocabularies (for example `vcard:Email`), never the project's own. It flags them only as a hint, which never shows in the Problems count or in `turtle-lsp check`.

Some teams also hold their ontologies to a house style: every term has a label and a definition, and the ontology header has a title, a description and a licence. Today they have to run a separate tool (ROBOT `report`) in CI to enforce it. That tool reports 33 checks, most of them specific to biomedical (OBO) projects, and it can't show results while you type.

A project has no way to configure any of this. It can't silence a check it doesn't want in the editor; only `turtle-lsp check --ignore` can, for one run. It also can't switch on a check it does want.

## Solution

Two parts, each shippable on its own. Checking `turtle-lsp.json` itself is a separate spec, `settings-file-checks.md`.

**Part A: four checks on by default, and `lint.ignore`.** Four checks that catch real mistakes in any ontology, shown as warnings in the editor and in `turtle-lsp check`:

- `duplicate-label`: a warning on the label of each term that shares its label with another term in the workspace, in the same language. The message names the other term.
- `label-whitespace`: a warning on a label that starts or ends with whitespace, with a quick fix that trims it.
- `multiple-labels`: a warning on each label of a term that has more than one `rdfs:label`, or more than one `skos:prefLabel`, in the same language. Tools then pick one at random, and SKOS forbids two prefLabels in one language. (Moved from Part B: it is a mistake, not a house style.)
- `deprecated-term` for the project's own terms: a use of a term that a workspace file marks `owl:deprecated true` is a warning with the deprecated tag. It is struck through and counted, and hover says the term is deprecated. Deprecations from the bundled vocabularies and from imports stay hints, as today.

`lint.ignore` in `turtle-lsp.json` (and in the editor's `turtle.lint.ignore`) lists diagnostic codes the project never wants reported. It works like `--ignore`, and the lists from the file, the editor setting and the flag add up.

**Part B: eight house-style checks, off until switched on with `lint.enable`.** The checks are:

- `missing-label`
- `missing-definition`, `multiple-definitions`, `duplicate-definition`, `lowercase-definition`
- `missing-ontology-title`, `missing-ontology-description`, `missing-ontology-license`

A project switches them on by listing codes: `"lint": { "enable": ["missing-label", "missing-ontology-license"] }`. They are warnings once enabled.

Warnings do not fail `turtle-lsp check` unless `--max-warnings` is given, so existing CI keeps passing.

## User Stories

### Part A: default checks and lint.ignore

1. As an ontologist, I want a warning when two classes have the same `rdfs:label`, so that I catch the copy-paste slip before anyone searches for that name.
2. As an ontologist, I want the duplicate-label warning to name the other term, so that I can jump to it and decide which one to rename.
3. As an ontologist, I want the warning on both terms' labels, so that I see it whichever file I have open.
4. As an ontologist, I want terms in different files of the workspace compared, so that a duplicate split across `core.ttl` and `labels.ttl` is still found.
5. As an ontologist, I want `"Pump"@en` and `"Pump"@de` treated as different labels, so that translations that happen to match don't warn.
6. As an ontologist, I want `"Pump"` and `"pump"` treated as the same label, so that a case-only difference is still caught.
7. As an ontologist, I want `skos:prefLabel` checked as well as `rdfs:label`, so that SKOS concept schemes get the same check.
8. As a data engineer, I want instance data left alone by the duplicate check, so that a file of 10,000 people with common names doesn't flood me with warnings.
9. As an ontologist, I want only declared terms checked (classes, properties, SKOS concepts), so that the warning means what it says.
10. As an ontologist, I want a term that repeats its own label not reported as its own duplicate, so that the check stays about different terms.
11. As an ontologist, I want terms from imported ontologies and bundled vocabularies left out, so that I'm only warned about labels I can change.
12. As an ontologist, I want a warning on a label with a leading or trailing space, so that I find it before it breaks a search.
13. As an ontologist, I want a quick fix that trims the label, so that fixing it takes one click.
14. As an ontologist, I want the trim fix to keep the literal's language tag, datatype, quote style and escapes, so that only the whitespace changes.
15. As an ontologist, I want tabs and newlines at the ends counted as whitespace too, so that a label pasted from a spreadsheet is caught.
16. As an ontologist, I want `label-whitespace` checked on `rdfs:label`, `skos:prefLabel` and `skos:altLabel` of any subject, so that the check covers every label I write.
17. As an ontologist, I want uses of a term I marked `owl:deprecated true` to be warnings, so that I can find every leftover use after retiring it.
18. As an ontologist, I want my deprecated term struck through where it's used, so that I see it while reading.
19. As an ontologist, I want hover on my deprecated term to say it's deprecated, so that I know why it's struck through.
20. As an ontologist, I want the deprecated term's own description (its triples in subject position) not flagged, so that marking a term deprecated doesn't warn on its own definition.
21. As an ontologist, I want `owl:deprecated true` recognised whether written as `true`, `"true"^^xsd:boolean` or a JSON-LD boolean, so that the way I write it doesn't matter.
22. As an ontologist, I want `owl:deprecated false` to mean not deprecated, so that un-deprecating a term clears its warnings.
23. As a SPARQL author, I want my own deprecated terms flagged in `.rq` files too, so that queries stop using retired terms.
24. As a JSON-LD author, I want uses of a deprecated term through my context flagged as well, so that every format behaves the same.
25. As a user of vCard or other bundled vocabularies, I want their deprecated terms to stay hints, so that widely used terms like `vcard:Email` don't become warnings I can't act on.
26. As a team lead, I want to switch off any check for the whole project in `turtle-lsp.json`, so that a deliberate pattern doesn't warn for everyone.
27. As a team lead, I want `lint.ignore` to take the same codes as `--ignore`, so that there's one list of names to learn.
28. As a developer, I want my editor's `turtle.lint.ignore` to add to the project's list, so that I can hide more for myself without undoing the team's choice.
29. As a CI maintainer, I want `--ignore` on the command line to add to the file's list, so that one run can hide more without editing the file.
30. As a CI maintainer, I want `turtle-lsp check` to report exactly what the editor shows, so that the build and the editor never disagree.
31. As a user who clones someone else's repository, I want `lint.ignore` to only hide diagnostics, so that a shared file can't make the tool do anything riskier.
32. As an existing user, I want `lint.undescribedTerms: false` to keep working, so that my config doesn't break.
33. As an existing user, I want my CI to keep passing after upgrading, so that the new warnings are information, not a breaking change.
34. As a user of `--fix`, I want the trim fix applied only with `--unsafe-fixes`, so that `check --fix` never changes data values without my say-so.
35. As a user on a large workspace, I want typing to stay as fast as before, so that the workspace-wide label comparison doesn't slow every keystroke.
36. As a user of the browser editor, I want the same checks and settings, so that the browser behaves like the desktop server.
37. As a Neovim user, I want to set `lint = { ignore = { ... } }` in `init_options`, so that I configure it the way I configure the rest.

### Part B: opt-in house-style checks

38. As a team lead, I want to switch on `missing-label` for the project, so that every class and property we publish has a name.
39. As a team lead, I want to switch on `missing-definition`, so that every term says what it means.
40. As a team lead, I want `multiple-labels` to flag a term with two labels in the same language, so that tools don't pick one at random.
41. As a team lead, I want `multiple-definitions` and `duplicate-definition`, so that each term has exactly one definition and no two terms share one.
42. As a team lead, I want `lowercase-definition`, so that definitions read as sentences.
43. As a publisher of an ontology, I want `missing-ontology-title`, `missing-ontology-description` and `missing-ontology-license` on the `owl:Ontology` header, so that others can find, understand and legally reuse it.
44. As a team lead, I want these checks off unless I list them, so that projects that don't follow this house style see nothing new.
45. As a team lead, I want `lint.enable` to take codes in the same form as `lint.ignore`, so that switching checks on and off works the same way.
46. As a team lead, I want a code in both `lint.enable` and `lint.ignore` to stay ignored, so that hiding always wins and the outcome is predictable.
47. As a team lead, I want the house-style checks to look only at declared terms, so that instance data doesn't get "missing definition" warnings.
48. As a team lead, I want `rdfs:comment` accepted as a definition when `skos:definition` is absent, so that ontologies that document terms with comments don't fail `missing-definition`.
49. As a CI maintainer, I want enabled checks reported by `turtle-lsp check` from the same file, so that CI enforces the house style without extra flags.
50. As a publisher of an ontology, I want the header checks to accept `dcterms:` and `dc:` title, description and licence properties, so that common header styles pass.

## Implementation Decisions

### Part A

- **Codes.** Three new diagnostic codes, `duplicate-label`, `label-whitespace` and `multiple-labels`, all warnings. They join the known-code list, so `--select` and `--ignore` accept them and an unknown code still exits 2. `deprecated-term` keeps its code; its severity depends on where the deprecation comes from: a workspace document gives a warning, a bundled vocabulary or an import gives a hint.
- **Which subjects are terms.** The workspace may type a subject `owl:Class`, `rdfs:Class`, `owl:ObjectProperty`, `owl:DatatypeProperty`, `owl:AnnotationProperty`, `rdf:Property` or `skos:Concept`, or the subject may be the subject of `rdfs:subClassOf` or `rdfs:subPropertyOf`. Such a subject is a term. Everything else is instance data, and the label checks that apply to terms do not compare it. Part B uses the same rule.
- **Label properties.** `duplicate-label` compares `rdfs:label` and `skos:prefLabel`. `label-whitespace` checks `rdfs:label`, `skos:prefLabel` and `skos:altLabel` on any subject. The lists are fixed in this version.
- **Comparing labels.** Two labels collide when their trimmed text is equal ignoring letter case and their language tags are equal ignoring case. An untagged label is its own group and does not collide with a tagged one. A term never collides with itself.
- **Workspace label index.** Each document's analysis today keeps only the preferred label per term, which isn't enough. The knowledge index gains a map from (folded label, language) to the terms that carry it, built from each workspace document's label triples. It is updated per document as that document changes, like the existing indexes, so a keystroke re-reads only the edited document's labels. External sources (imports, bundled vocabularies) don't feed it.
- **Where the diagnostics go.** `duplicate-label` goes on each colliding label literal in the document being checked. The message names the other term or terms by the prefixed name the document can write. The check sits in the lint module next to `undescribed-term` and reads the analysis and the knowledge index. The editor, workspace diagnostics, the browser engine and `check` all call that one function.
- **Trim fix.** `label-whitespace` carries a quick fix that rewrites only the literal's lexical value without the surrounding whitespace. Quote style, escapes, the language tag and the datatype are kept. In JSON-LD it edits the string value. The fix is in the unsafe class: the editor offers it, but `check --fix` applies it only with `--unsafe-fixes`.
- **Own deprecations.** The knowledge index records the terms that a workspace document gives `owl:deprecated` with a boolean true value (`true`, `"true"^^xsd:boolean`, or a JSON-LD `true`). The deprecated-term check, the deprecated semantic-token modifier and hover consult this set as well as the bundled flag. Refs in subject position are never flagged, the same rule `undescribed-term` uses.
- **`lint.ignore` setting.** A new settings key holds a list of known diagnostic codes. Unlike other keys, its layers add up rather than replace one another: `turtle-lsp.json`, `init_options`, the editor's `turtle` section and `--ignore` are combined. The file may set it, because it can only hide diagnostics. The server filters ignored codes out before publishing, for open documents and workspace diagnostics alike, and `check` applies the same filter. `lint.undescribedTerms: false` stays and has the same effect as listing `undescribed-term`.
- **VS Code.** The extension declares `turtle.lint.ignore` as an array of strings and passes it on like the other server settings.

### Part B

- **Codes and default.** There are eight new codes, all warnings, all off by default: `missing-label`, `missing-definition`, `multiple-definitions`, `duplicate-definition`, `lowercase-definition`, `missing-ontology-title`, `missing-ontology-description` and `missing-ontology-license`. They are known codes from the start, so `--select`, `--ignore` and `lint.ignore` accept them.
- **`lint.enable` setting.** This list of codes switches off-by-default checks on. It adds up across layers like `lint.ignore`. `lint.ignore` wins over `lint.enable`. A code that is already on is accepted in `lint.enable` and changes nothing. There is no command-line flag; `check` reads the file.
- **What they read.** The label checks use the Part A label properties. A term's definition is its `skos:definition`, or its `rdfs:comment` when it has no `skos:definition`. The header checks look at each subject typed `owl:Ontology` in a workspace document. A title is `dcterms:title`, `dc:title` or `rdfs:label`. A description is `dcterms:description`, `dc:description` or `rdfs:comment`. A licence is `dcterms:license` or `cc:license`. Only declared terms are checked.
- **Where they go.** `multiple-labels` (Part A) and a multiple-thing diagnostic count within one language and one property. A missing-thing diagnostic goes on the term's or the ontology's subject in the document that describes it. A multiple-thing or duplicate-thing diagnostic goes on the offending literals. Each multiple-thing check counts within one language.
- **Index.** The duplicate-definition check reuses the label index's design with definitions as the key. The others read the triples of the document being checked and need no new index.

## Testing Decisions

- A good test opens text or a small workspace and asserts what a user would see: diagnostic codes, ranges, severities and messages, the text after applying a fix, and hovers. Tests don't reach into the index's internals.
- **Highest seam: the per-document diagnostics function, given a workspace's knowledge.** Its unit tests cover Parts A and B:
  - duplicate pairs, the language and case rules, instance data skipped, self-collision, imports skipped;
  - whitespace variants;
  - the deprecated true and false forms, and the subject-position rule;
  - each house-style check on and off, `lint.ignore` winning over `lint.enable`, and the header property alternatives.

  Prior art: the existing `undescribed-term` and `deprecated-term` unit tests, which build a small workspace and assert its diagnostics.
- **Editor and command line agree.** Extend the agreement test that runs the same files through the server and `check`. It gets a workspace that triggers all three default checks, and a `turtle-lsp.json` with `lint.ignore` and `lint.enable`.
- **LSP end to end.** The Node lint tests cover:
  - a duplicate across two files, and an edit that resolves it (the other file's warning clears);
  - the trim quick fix applied;
  - `lint.ignore` and `lint.enable` from the file and from the editor settings.

  Prior art: the lint and settings-file Node tests.
- **CLI.** CLI tests cover:
  - `lint.ignore` and `lint.enable` in the file;
  - adding `--ignore`;
  - exit codes with and without `--max-warnings`;
  - the trim fix under `--fix` (not applied) and `--unsafe-fixes` (applied).

  Prior art: the CLI test file's `--select` and `--ignore` cases.
- **Browser.** The engine test asserts that the default checks appear in the WebAssembly build.
- **Chaos.** The chaos tests already check every quick fix against the tokens and the graph it should leave. The trim fix must pass them unchanged, with no exemption.
- **Performance budget, pass or fail.** On a quiet machine, the benchmark's keystroke-latency rows move by no more than 5% against main, and the initial index of the 200-file workspace by no more than 10%.

## Out of Scope

- Presets, per-rule severity levels, and the `--rule`, `--preset`, `--enable` and `--list-rules` flags.
- Suppression comments in the source (`turtle-lsp-disable-next-line`).
- The OBO-specific ROBOT checks (synonyms, xrefs, subsets, obsolete-label conventions, OBO ID format, the deprecated boolean datatype) and running ROBOT's SPARQL files.
- ROBOT's OWL structure checks (equivalent pairs, multiple equivalent classes or definitions, genus-less equivalent class axioms, missing or multiple superclasses).
- `annotation_whitespace` and `label_formatting`.
- Configurable label, definition and header property lists.
- Per-folder settings.
- SKOS integrity conditions beyond what `duplicate-label` and `multiple-labels` give for `skos:prefLabel`.
- Checking `turtle-lsp.json` itself, which is the settings-file-checks spec.

## Seam (as built)

- `Settings::lint_on(code)` decides whether a code is reported: not in `lint.ignore`, not `undescribed-term` with `lint.undescribedTerms: false`, and, for a code in `curation::OPT_IN`, listed in `lint.enable`. `lint.ignore` and `lint.enable` add up across layers; the settings file may set both.
- `diagnose::document`, the one composer the server, workspace diagnostics, the browser and `check` call, drops every diagnostic whose code is not on. `check --select/--ignore` filter on top.
- The checks live in `features::curation` (one function, `diagnostics`, called last by the RDF diagnostics). `diagnostics::Options::house_style` says whether any opt-in check is on, so the house-style pass is skipped otherwise.
- Cross-file updates reuse `Knowledge::described_generation`: a change to the label index, definition index or own-deprecated set that can change another document's diagnostics bumps it, and the server re-diagnoses open documents after the usual pause.

## Further Notes

- Do A first, then B, which reuses A's term rule and index design. The settings-file-checks spec can run alongside either. Its check reports unknown codes in `lint.ignore` and `lint.enable` once both land.
- The checks are informed by ROBOT `report`'s list (BSD-3-Clause), but no text or queries are copied, so no third-party notice is needed. If a later change copies ROBOT's level table or queries, add it to the third-party notices in the same change.
- Open question for review: should the duplicate checks also compare against imported ontologies' labels? They are left out here because those labels can't be fixed in the workspace.
