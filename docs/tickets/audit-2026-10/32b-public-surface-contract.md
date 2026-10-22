# 32b: Narrow the public surface, part 2 (contract): `pub(crate)` inside the public modules

**What to build:** the contract step of audit X4-17. After 32a, 16 modules stay `pub`
for the consumers listed in the crate doc of `src/lib.rs`. Inside them, most `pub`
items still have no outside user. Turn every `pub` item in those modules into
`pub(crate)` (or narrower), then make `pub` again exactly what a consumer fails to
compile without. The compiler gives the list: `cargo build --release --locked
--all-targets` and `node web/build.mjs`. Code that turns out dead (it was `pub` and
nothing in the crate uses it) is deleted and listed in the report. This sweeps every
file under `src/`, so it runs alone. A `pub` field on a type that stays `pub` is
narrowed only where no consumer reads it. A `pub use` re-export kept for an old path
inside the crate (`features::format::Options` from 26a) is replaced by the direct path
and removed. **No behaviour change.**

**Files owned:** every file under `src/` (visibility, dead-code deletion and the import
paths those changes force), and `src/lib.rs` (the consumer list, if a path changed).
**Hands off:** `web/engine/**`, `examples/**` and `tests/**`. Consumers do not change:
32b only narrows what they do not use. If a consumer really should move, stop and
report it instead.

**Blocked by:** 32a and 35. Runs alone, the last code ticket of wave 3.

**Status:** ready-for-agent

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. No performance benchmarks. No new dependencies. Leave the changes
uncommitted.

- [ ] Every `pub` item under `src/` is used by a consumer, or is a field or method of
      a type a consumer uses that the consumer reaches. Check: flip one such item to
      `pub(crate)` and a consumer fails to build. Spot-check ten, and list them.
- [ ] Report the `pub fn` and `pub` type counts before and after (the audit counted
      493 and 213 at d7c33eb; 32a reports its own figures), and every deleted item.
- [ ] `cargo build --release --locked --all-targets` and `node web/build.mjs` pass
      with no change under `web/engine/`, `examples/` or `tests/`.
- [ ] Green, no behaviour change: `cargo test --release --locked --lib` and every
      integration test binary one at a time; the Node suite file by file
      (`timeout 120 node --test --test-timeout=30000 test/<file>.test.mjs` for each, and
      `timeout 120 npm run test:latency`); `web/test` after `node web/build.mjs`;
      `(cd editors/vscode && timeout 120 node --test test/manifest.test.mjs)`;
      `cargo fmt --check`, clippy.
