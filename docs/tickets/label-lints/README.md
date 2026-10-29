# Label lints tickets

Spec: docs/specs/label-lints.md. Built with parallel slices on the
integration branch `feat/label-lints`.

| # | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|
| 01 | Settings seam: `lint.ignore`, `lint.enable`, composer filter, codes | — | src/settings.rs; src/diagnose.rs; **src/features/diagnostics.rs**; **src/cli/output.rs**; new src/features/curation.rs (stub) |
| 02 | Knowledge: term rule, label and definition index, own deprecations | 01 | src/knowledge.rs |
| 03 | `lint.ignore`/`lint.enable` end to end: VS Code settings, LSP and CLI tests | 01 | editors/vscode/package.json; test/settings-file.test.mjs; test/cli.test.mjs |
| 04 | `label-whitespace` and its trim fix | 01 | **src/features/curation.rs** (whitespace part); src/features/quickfix.rs; src/batch.rs; new test/label-lints.test.mjs |
| 05 | `duplicate-label`, `multiple-labels` and the eight house-style checks | 02, 04 | **src/features/curation.rs**; test/label-lints.test.mjs |
| 06 | Own `deprecated-term`: warning, strike-through, hover, SPARQL, JSON-LD | 02 | **src/features/diagnostics.rs** (`vocabulary_terms` only); src/features/hover.rs; src/features/semantic.rs; src/sparql/features/diagnostics.rs; test/lint.test.mjs |
| 07 | Close-out: docs, agreement and browser tests, full validation, bench | 01–06 | README.md; CHANGELOG.md; editors/vscode/README.md; web/test/engine.test.mjs; agreement test; full validation |

Lock files: `src/features/curation.rs` is written by 04, then 05 (05 starts
after 04 lands). `src/features/diagnostics.rs`: 01 added the `Options`
field and the call to `curation::diagnostics`; 06 edits only
`vocabulary_terms`.

Waves: 01 (done by the orchestrator); then 02, 03 and 04 together; then 05
and 06 together; then 07.

## Departures from the plan

- 01 was written by the orchestrator, not a slice: it is the seam every
  other ticket names.
- `multiple-labels` is on by default (Part A), not opt-in: two labels in one
  language is a mistake tools trip over, not a house style.
- 04 extended the chaos tests' unsafe-fix check: a string token may become
  the same string with the whitespace at its value's ends removed (same
  quotes). The fix itself is checked exactly, with no exemption. At 20,000
  iterations the old check failed on the new fix.
- 04: the `label-whitespace` range is the quoted string only, not the
  language tag or datatype.
- 05: `duplicate-label` compares terms of the same kind only (class,
  property, concept; a term of unknown or mixed kind collides with any).
  On the corpus, class/property pairs that differ only in case (dbpedia
  `Judge`/`judge`) made half the warnings; per-file corpus count went from
  2,399 to 1,134 (schema.org 118 to 0).
- 05: the five `missing-*` codes wait for the initial index, like
  `undescribed-term`.
- 05: cross-file `missing-label`/`missing-definition` read other documents'
  label and comment lookups, which also count `sh:name` and
  `dcterms:description`: lenient (may stay quiet), never a wrong warning.
