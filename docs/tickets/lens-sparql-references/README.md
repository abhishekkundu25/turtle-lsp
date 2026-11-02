# Tickets: reference lenses count SPARQL uses

Spec: [docs/specs/lens-sparql-references.md](../../specs/lens-sparql-references.md)

| # | Ticket | Blocked by | Files |
|---|---|---|---|
| 01 | [One search for a term's uses; the lens uses it](01-lens-uses-find-references.md) | | `src/ide/navigation.rs`, `src/ide/display.rs`, `src/features/lens.rs`, `web/test/lsp.test.mjs` |
| 02 | [Close-out](02-close-out.md) | 01 | **`CHANGELOG.md`**, the spec's status |

Waves: 01, then 02. One slice: the fix touches one seam.

## Departures from the plan

- `test/lens.test.mjs` pinned the old count: ex:Person goes from 4 to 5
  references (its fixture's `q.rq` now counts).
- Find References keeps its own code path; only its SPARQL step is shared
  with the lens (`Session::sparql_term_references`), so a JSON-LD term's
  references are unchanged.
- The keystroke benchmark was not rerun (see 02).
