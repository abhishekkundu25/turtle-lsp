# 11: TriG one-line graph block parses in linear time

**What to build:** Statements in one TriG graph block on one line cost time quadratic in their number (the same statements on separate lines, or outside a block, are linear): 100,000 tokens take over a second, 200,000 about five. The ignored test `trig_graph_block_on_one_line_parses_in_linear_time` in tests/chaos.rs states the bound. Find the quadratic step in src/syntax/parser.rs (the review's reading: error recovery or a statement-start search that rescans from the block's start for each statement when there is no line break to anchor on), make it linear, and remove the `#[ignore]`. The squeeze mutation of ticket 12 makes one-line blocks common, so this must land before it. This is the second grandfathered exemption.

**Blocked by:** nothing.

**Issue:** #28. **Wave:** 1. **Files (lock in bold):** src/syntax/parser.rs; **tests/chaos.rs** (the ignored test only)

**Status:** proposed

- [ ] `trig_graph_block_on_one_line_parses_in_linear_time` passes and is no longer ignored (it is a wall-clock test: keep it in tests/chaos.rs's pathological section where the existing bound helper runs, or rename to `timing_` if moved into the lib).
- [ ] A unit test in src/syntax/parser.rs tests with the minimal shape (a block with N statements on one line) that asserts the parser's step count (or the recovery call count) is linear, independent of the clock.
- [ ] `bench/perf.mjs` keystroke rows for TriG within the spec's noise definition, measured by ticket 13.
- [ ] W3C TriG suite green; `tests/chaos.rs` turtle shards pass a push-sized run under `timeout 120`.
