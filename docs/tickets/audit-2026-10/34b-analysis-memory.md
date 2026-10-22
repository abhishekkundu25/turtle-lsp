# 34b: Analysis memory and the 40-bytes-per-byte budget

**What to build:** after 34a the tokens are small, but `Analysis` and the formatter
still hold more than they need (audit X5-04, X5-13, X5-18). Features read tokens
straight from `a.parsed.tokens`: `completion.rs:104` and `:799`,
`completion/ranking.rs:294`, `semantic.rs:113`, `refactor.rs:556`, `symbols.rs:338`,
`syntax/names.rs:225`, and `features/format.rs:90-95`, which re-lexes when the vector
was dropped. A document opened in a workspace always keeps its tokens
(`src/workspace.rs:698`, `:740`), and that includes every file `batch` opens for
`check`, `format` and the browser. The formatter's safety net re-parses its own output
with a full parse (`features/format.rs:107-113`), so peak memory holds two parses with
tokens, refs and comments, plus the output. `change_doc` builds a new `LineIndex` for
each range change (`src/workspace.rs:608-624`), and `Analysis::new` builds another
(`src/analysis.rs:220`). Make `Analysis::tokens()` (and `comments()`) the only door,
filled lazily, at most once per analysis, when the parse did not keep them. Move the
readers to it. Let `batch` open its documents without keeping tokens, through a
workspace option. Give the parser a triples-only mode (no tokens, refs or comment
spans kept) for the formatter's verification parse. Apply a notification's range
changes against one line index, adjusting offsets as LSP's sequential order requires,
and hand the final index to the analysis. Then measure the budget. If it is still
over, the next levers in order are: one text copy per open document (`Doc.text` and
`Analysis.text` are two), and literal values as spans into the text where nothing
needs unescaping. **Behaviour:** none visible; the memory budget below is the
acceptance.

**Budget (pass/fail):** peak resident memory of `turtle-lsp format --check FILE` and of
`turtle-lsp check --no-shacl FILE`, each **at most 40 bytes per input byte**, on the
bench's big-file document. That document is `bigDoc` in `bench/perf.mjs`, grown until
it is at least 2,400,000 bytes, as the `big-file` scenario builds it: 2,492,901 bytes
today. Measure it with `/usr/bin/time -l` on macOS ("maximum resident set size",
bytes) or `/usr/bin/time -v` on Linux ("Maximum resident set size", KiB), on the
release binary, three runs, taking the median. Baseline on 2026-10-22 at 8e3f8d7,
macOS arm64: `format --check` 152.9 to 155.9 MB (61.3 to 62.5 B per byte), `check
--no-shacl` 156.0 MB (62.6 B per byte). The budget is 99.7 MB. Add the measurement to
the bench as a scenario `big-file-peak`, with rows `peak-format-big` and
`peak-check-big` (`rssMb` = the peak, notes = bytes per input byte). Where
`/usr/bin/time` is missing (Windows), the rows are `null` with a note. Ticket 36 reruns
the budget on a quiet machine.

**Files owned:** `src/analysis.rs`, `src/workspace.rs` (`change_doc`, the `analyze`
call sites and the keep-tokens option only), `src/batch.rs` (the option),
`src/syntax/parser.rs` (the triples-only mode), `src/syntax/names.rs` (line 225),
`src/features/{format,completion,semantic,refactor,symbols}.rs` and
`src/features/completion/ranking.rs` (the token reads), `bench/perf.mjs` (the new
scenario only), `tests/chaos.rs` (token reads).
**Hands off:** `src/server.rs`, `src/server/**` and `src/ide/**` (28c holds the server
in this wave), `src/syntax/lexer.rs` (34a's representation stays), the other bench
scenarios.

**Blocked by:** 34a. Also 27b and 31 (`src/workspace.rs` lock). Runs in the same wave
as 28c, with disjoint files.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`, and the measurement writes its file under the default temp
directory. The only performance measurement allowed is the budget above (and
`node bench/perf.mjs --only big-file-peak`), and it is rerun later on a quiet machine.
No new dependencies. Leave the changes uncommitted.

- [ ] `rg -n "parsed\.tokens|parsed\.comments" src --glob '!src/syntax/**' --glob '!src/analysis.rs'`
      finds nothing. Analyses that `batch` builds keep no token vector (a unit test).
      `Analysis::tokens()` on an analysis built without tokens equals the tokens of
      one built with them.
- [ ] The verification parse keeps no tokens or refs, and `tests/format.rs` still
      catches a formatter that changes the graph (an injected bad output under
      `cfg(test)`, refused with the "formatter bug" message).
- [ ] `change_doc` builds at most one `LineIndex` per notification. A unit test applies
      50 range changes in one notification and gets the same text as 50 notifications
      of one change; `tests/chaos.rs` ranged edits hold.
- [ ] **Budget:** `peak-format-big` and `peak-check-big` are each at most 40 B per input
      byte. Report the three runs of each, the median, and the baseline above.
      `rss-big-file` (LSP open plus 20 keystrokes) is reported, not gated.
- [ ] Green: `--test oxttl_oracle`, `--test w3c_conformance`, `--test chaos`,
      `--test format`, `--test jsonld_budget`, `cargo test --release --locked --lib`;
      `test/display.test.mjs`, `test/completion-ranking.test.mjs`,
      `test/refactor.test.mjs`, `test/format.test.mjs`, `test/cli.test.mjs`,
      `test/lsp.test.mjs`, `timeout 120 npm run test:latency`;
      `web/test/engine.test.mjs`; `cargo fmt --check`, clippy.
