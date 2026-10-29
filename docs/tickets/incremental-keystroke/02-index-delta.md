# 02: Index delta

**What to build:** Part 1 of the spec, in `src/knowledge.rs` only.

- `DocFacts` keeps a count per IRI for each kind of contribution it
  toggles in `apply` (subject, object, predicate, refs, defs, bindings) as a
  sorted vector of `(Iri, count)` by [`Iri::id`], instead of unique lists
  plus reads of `refs_by_iri` / `defs` at apply time.
- `Knowledge::put`, when it replaces a source, walks the old and new facts
  by a sorted merge and changes the index only where a count moves between
  zero and non-zero (or the curation summary of an IRI differs). Facts
  equal to the old ones skip the swap's index work entirely (typing inside
  a comment or a literal that no check reads). Removal (`take`) and first
  add stay as today.
- `Keyed` (labels, definitions, comments) holds positions relative to its
  statement (statement index plus offset), so an edit before it changes
  nothing in the facts.
- The change set of a swap (IRIs and text keys whose entries changed) is
  returned from `put` / `set` / `set_prepared` to the caller, or kept on
  `Knowledge` and read once; it replaces the scratch state the label checks'
  refresh uses today. 06 reads it.
- Leave `Knowledge::snapshot` alone (01 changes its attribute); if a new
  field needs dumping, say so in the report.

**Blocked by:** none.

**Status:** done

- [x] `incremental_updates_match_a_rebuild` and every existing knowledge
  test pass unchanged; new tests: a keystroke in a comment leaves the
  index and generations untouched; a label edit changes only that key; a
  ref added and removed in one swap nets to nothing; the change set names
  exactly the changed IRIs and keys.
- [x] A `timing_` test: replacing a 17,000-class vocabulary's facts after
  a one-label edit is under a fixed budget well below a full
  release-and-apply (measure both in the test; assert the ratio, not only
  a wall time).
- [x] `tests/chaos.rs` passes.
