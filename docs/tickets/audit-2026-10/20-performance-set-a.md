# 20: Performance set A

**What to fix:** measured costs hidden from callers (release binary, Apple Silicon,
load < 3.5).

- SHACL validates the shared Turtle graph once, then for every TriG/N-Triples/N-Quads
  dataset (and JSON-LD with named graphs) builds a new `Graph` of that dataset plus
  **all** shared triples and validates it; `shacl.maxTriples` applies per unit
  (`src/features/shacl_validate.rs:256-268, 371-372, 383`). 75k shared triples: 0.07 s
  with 0 datasets, 0.26 s with 50, 0.82 s with 200; runs on every save (and typing pause
  with `shacl.onType`).
- Find References / Rename on any RDF IRI walks the whole workspace tree and re-reads
  and re-parses every `.rq/.sparql/.ru` file per request; `MAX_FILES` caps matches, not
  entries visited (`src/server.rs:1518, 1605`; `src/workspace.rs:655-686, 868-897`).
  9.6 ms on a 20k-entry tree with no hits vs 0.1 ms for a prefix target.
- `didChangeWatchedFiles` re-indexes one file at a time with its own rayon job
  (`server.rs:873-902`): 2,000 changed files 108 ms vs 73 ms for the parallel initial
  index.
- Each `sh:select` literal is re-parsed into a `SparqlAnalysis` per feature call
  (`src/features/embedded.rs:217-223`): 500 queries → keystroke 1.1 → 2.9 ms.
- `navigation::uri()` re-parses the document URI per `Location`; references on
  `foaf:Person` in a 100k-triple file: 75k locations, 16.8 MB, 349 ms
  (`navigation.rs:51-65, 237-255`).
- Typed completion walks every described IRI, calls `shorten` per candidate and sorts
  with two `to_lowercase` per comparison before the 200 cap: `ex:p` over 40k subjects
  31 ms vs 1.3 ms untyped (`completion.rs:315-340, 407-417`); the ranking's 20,000-ref cap
  resets per document (`ranking.rs:603-618`).

**Files owned:** `src/features/shacl_validate.rs`, `src/rdf/graph.rs` (an overlay or a
shared read-only base), `src/workspace.rs` (a SPARQL file index kept current by the
watcher; `index_paths` batched), `src/server.rs` (the watched-files branch and the two
`sparql_documents()` call sites only), `src/features/embedded.rs`,
`src/features/navigation.rs`, `src/features/completion.rs`,
`src/features/completion/ranking.rs`, `test/completion-ranking.test.mjs`,
`test/sparql-navigation.test.mjs`.
**Hands off:** `src/shacl/**` semantics (ticket 06), `src/query/**`, the rest of
`src/server.rs`.

**Blocked by:** ticket 15 (`completion.rs`), ticket 14 (`workspace.rs` lock #2),
ticket 19 (`server.rs` lock #2). Third holder of both locks.

**Status:** ready-for-agent

- [ ] Shared `Graph` and indexes built once per run; each dataset validated as an
      overlay (dataset triples + shared base) or batched with per-origin focus filtering;
      `maxTriples` bounds the run's total work. Test: 200 datasets ≤ 2× the 0-dataset
      time (release, counted work preferred over wall time);
      `a_dataset_shows_only_the_results_its_data_changes` and W3C SHACL unchanged.
- [ ] SPARQL files indexed once (paths + `SparqlAnalysis` or a per-file IRI set),
      updated on watched events; `sparql_documents()` becomes a lookup. Test: references
      on an IRI with no SPARQL hits on a 20k-entry tree < 1 ms; `sparql_documents_are_
      recognised_and_never_feed_the_index` green.
- [ ] Watched-file changes of one notification collected and indexed in one parallel
      `index_paths`; deletions separate. Test: 2,000 changed files ≤ initial-index time.
- [ ] Embedded analyses cached in a `OnceLock` on the `Analysis`. Document URI parsed
      once per document in `loc`.
- [ ] Completion: sort keys computed once per candidate; `select_nth_unstable_by` for
      the top 200, then sort those; shared scan budget across documents; the predicate
      from the triple, not a backward ref scan. Tests: item order identical on existing
      completion tests; typed `ex:p` over 40k described < 5 ms (release).
- [ ] All `test/*completion*.test.mjs`, `test/lsp.test.mjs`, `test/refactor.test.mjs`,
      `test/shacl-validate.test.mjs`, `test/shacl-sparql-editor.test.mjs` green.
