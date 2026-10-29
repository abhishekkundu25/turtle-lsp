# 03: Keep the edits and the line starts; a per-line line table

**What to build:** Part 2 of the spec, in `src/workspace.rs` and
`src/analysis.rs`.

- The open document (`Doc`) keeps, until its next analysis: the collapsed
  edit (one byte range in the last analysed text, and the new length; a
  later batch widens it) and the `LineStarts` that `change_doc` already
  moves. A full-text change, or a change it cannot collapse, clears the
  edit (the next analysis is full). The analysis clears both once it ran.
  Model it as a type with the cases it has (no edit since the analysis,
  one collapsed edit, a full change), not as optional fields.
- `LineIndex` counts UTF-16 code units and code points per line (running
  totals at each line start), with fixed-stride marks only inside lines
  over about 4 KB. Conversions answer exactly as today.
- A pub(crate) way to build an `Analysis` from the kept line starts
  (`Analysis::with_loader` stays as it is for its other callers), and a way
  to update the previous `LineIndex` from the collapsed edit, touching only
  the changed lines plus a shift of the later running totals. The
  workspace uses it for open documents.
- No change to parsing or the index here.

**Blocked by:** none.

**Status:** open

- [ ] `LineIndex` unit tests: the existing conversion tests pass; a new
  property test compares the updated index with `LineIndex::new` on the
  edited text for random edits over ASCII, multi-byte, CRLF and lone CR
  text, and a line over 4 KB.
- [ ] Workspace tests: a batch of ranged changes, then a full change, then
  ranged again, each followed by an analysis, give the same positions as a
  fresh open (`change_doc_applies_ranged_changes_in_order` and its
  neighbours pass).
- [ ] The existing counting test on position conversion (issue #32) still
  passes.
- [ ] `tests/chaos.rs` and `timeout 120 npm run test:latency` pass.
