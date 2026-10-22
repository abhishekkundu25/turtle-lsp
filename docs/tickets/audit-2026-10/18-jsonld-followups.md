# 18: JSON-LD follow-ups

**What to fix:** the JSON-LD modules that walk `@context` syntactically and the test
harness around them.

- `jsonld-undefined-prefix` looks inside `@json` literals, and its "safe" fix
  (`check --fix`) inserts the prefix into the literal's `@context`, changing the
  `rdf:JSON` value (`src/jsonld/lint.rs:154-169`, `context_edit.rs:32-56`). Reproduced.
- Term rename follows neither scoping nor `@json` literals: `Scan` rewrites any string
  with the term in every `@context` in the text (`rename.rs:600-628`); one rename
  re-parses the document up to three times (`:167, 405, 576`).
- `context_edit::string_end` skips the byte after a backslash even when it is a
  newline, unlike the parser (`context_edit.rs:8-19` vs `json.rs:504-510`).
- The 218 negative W3C tests pass on any error: `expectErrorCode` is never compared
  (`tests/w3c_jsonld.rs:612-620`).
- The oxjsonld oracle and the corpus lint/format tests `return` early when
  `test/corpus/jsonld/` is absent, so CI reports them `ok` without running them
  (`tests/jsonld_oracle.rs:262-266`, `jsonld_lints.rs:303-306`, `format.rs:497-501`).
- A lone surrogate escape is an error (RFC allows it; other processors accept); the
  formatter drops a leading BOM silently; duplicate keys are never reported; the
  "more than 100 syntax errors" cap lands mid-file; the stack test nests 127 objects,
  not 254; the budget generator exists three times; `test/jsonld.test.mjs` header and
  helpers are stale (`json.rs:684-694, 372-388`; `format.rs:28-30`; `mod.rs:675-712`;
  `tests/jsonld_budget.rs:46-108`; `bench/jsonld-gen.mjs`).

**Files owned:** `src/jsonld/{lint,rename,context_edit,source,json,format}.rs`,
`src/jsonld/expand.rs` (one hook: record context strings with scope into
`SourceIndex`), `tests/w3c_jsonld.rs`, `tests/w3c/jsonld/expected-status.tsv`,
`tests/jsonld_oracle.rs`, `tests/jsonld_lints.rs`, `tests/jsonld_budget.rs`,
`src/jsonld/mod.rs` (test module only), `bench/jsonld-gen.mjs`, `test/jsonld.test.mjs`,
`test/jsonld-syntax.test.mjs`.
**Hands off:** `src/jsonld/{context,to_rdf,loader}.rs` (tickets 05, 14),
`.github/workflows/ci.yml` (the corpus fetch job is ticket 08's), `README.md`.

**Blocked by:** ticket 05.

**Status:** ready-for-agent

- [ ] Expansion records each context entry string it processes, with its scope, into
      `SourceIndex`; `lint::context_iris` and `rename::Scan` read that instead of walking
      JSON; `@json` values are never visited; `context_edit` picks its target from
      `SourceIndex::scope_at` with a syntactic fallback only for unparsable text and
      shares `json::tokens` for strings. Tests: an `@json` literal holding `@context`
      gets no lint; `--fix` leaves it unchanged; rename skips it; rename respects scope.
- [ ] Rename helpers take the parsed `Json` tree; edits looked up through a map.
- [ ] Negative W3C tests compare the message prefix with `expectErrorCode`; mismatches
      listed as `known-fail` with reasons. Corpus tests print `skipped` and fail when
      `CI=true` and the corpus is missing.
- [ ] JSON policy decided and documented in the module doc: lone surrogate (warning or
      error), BOM kept by the formatter, `json-duplicate-key` warning at the earlier key,
      error cap placed at the end of the reported range. Stack test nests 254 objects.
      One generator shared by both budget tests and the bench (or a checked-in fixture).
      Stale header/helpers removed.
- [ ] `tests/w3c_jsonld.rs`, `tests/jsonld_*`, `test/jsonld.test.mjs`,
      `test/jsonld-syntax.test.mjs`, `test/cli.test.mjs` (fix cases) green.
