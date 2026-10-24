# 14: Bounded edit distance costs O(max x length) (class E sibling)

**What to build:** `EditBuf::within` (src/vocab.rs, used by `NameTable::search` for the undescribed-term lint's "did you mean" in src/features/lint.rs) fills the full |a| x |b| table even though `max` is at most 3. Two long local names that share a long run (the chaos test's default seed, iteration 275: `0{25000}InvalidResource` and `0{25000}ValidResource1`) keep the diagonal at 0, so the early exit never fires: 625 million cells per pair, about 1.1 s per `diagnostics` call, quadratic in name length. The watchdog of ticket 09 found it. Fix inside `EditBuf::within`, no signature change: strip the common prefix and suffix (exact under the optimal-string-alignment distance it computes), then compute only the band of cells with |i - j| <= max, clamping to max + 1 and keeping the early exit. A prototype that agrees with the current function on 1.6 million random pairs is in the root-cause report (scratchpad chaos10/F-long-iri.md and F/bench/src/bin/band.rs).

**Blocked by:** nothing.

**Issue:** #29. **Wave:** 2. **Files (lock in bold):** src/vocab.rs

**Status:** proposed

- [ ] A clock-independent test in src/vocab.rs: a `#[cfg(test)]` cell counter asserts cells <= (2 max + 1)(L + 20), and cells at 2L at most 2.1 x cells at L, for the 275 shape and for `ab0{L}c` / `ba0{L}d`; it fails on the current code.
- [ ] The existing equivalence tests (src/vocab.rs, src/features/lint.rs) pass, plus a few new pairs that swap characters at the band's edge and at the stripped prefix and suffix boundary.
- [ ] `CHAOS_ONLY=275` at the default seed (once ticket 09 lands) runs in well under a second; until then, diagnostics on the saved input target/tmp/chaos-turtle-1592590343-275.trig in under 50 ms through a scratch test.
- [ ] Push-sized turtle chaos run (`CHAOS_ITERS=30000`) under `timeout 120`.
