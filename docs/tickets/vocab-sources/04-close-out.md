# 04: Close-out: docs and the full validation

**What to build:** the documentation says what turtle-lsp now ships and how
to refresh it, and the whole project is validated once, on a machine that
runs nothing else. This is the only ticket that edits the README, the
changelog and the other documents, and the only one that runs the full
validation.

**Blocked by:** 03.

**Status:** done

Documents:

- [ ] `README.md`: the "Embedded vocabularies" item gives the new counts of namespaces and terms, and no longer mentions `geof` and `geor`; the Development section's `npm run vocab` line and the credits line name the manifest and the publishers, not the package; a short passage says where each vocabulary comes from and how to refresh one.
- [ ] `CHANGELOG.md`, under `## Unreleased`: Fixed (the false warnings of issue #106, `rdf:_n`, EARL's namespace, the hover line, the `dbo` junk), Changed (the refreshed vocabularies with their versions, GS1's and DPV's namespaces, the new deprecation hints, the comment rule, terms their publishers removed are now reported).
- [ ] `THIRD_PARTY_NOTICES.md`: the Vocabularies section lists the sources and licences from the manifest. The package and its copyright line are removed.
- [ ] `ARCHITECTURE.md`: the `vocab` row names the manifest, the corrections file and the script. `AGENTS.md`: one release step for refreshing a vocabulary (edit the row, run `tools/check-vocab.sh --write`, copy the printed hash, read the removed terms), beside the step for the vendored crates.
- [ ] `web/build.mjs`: the engine's size in the budget's comment is the measured one. `WASM_GZIP_BUDGET` is not changed.
- [ ] The spec's status line says done. This README's Departures hold every difference from the spec, the chosen comment cap and the measured numbers below.

Full validation, each run under `timeout 120`, one file at a time, after `cargo build --release --locked` and `cargo test --release --locked --lib --tests --no-run`:

- [ ] `cargo fmt --check`, `cargo fmt --check --manifest-path web/engine/Cargo.toml`, `cargo clippy --release --locked --all-targets -- -D warnings`, and `cargo clippy --locked -- -D warnings` in `web/engine`.
- [ ] `timeout 120 cargo test --release --locked --lib -- --skip timing_`
- [ ] `timeout 120 cargo test --release --locked --lib -- timing_ --test-threads=1`
- [ ] Every integration test file alone: `timeout 120 cargo test --release --locked --test NAME` for each file in `tests/` (`vocabulary` and `jsonld_lints` among them), with the public corpus downloaded (`tools/corpus/fetch.sh`) so that none prints `skipped`.
- [ ] `timeout 120 cargo test --release --locked --doc`
- [ ] Every Node test file alone, with the reporters `npm run test:node` uses, so that a pending test that starts to pass still fails: each `test/*.test.mjs` except `latency`, then `timeout 120 npm run test:latency`.
- [ ] `(cd editors/vscode && npm run typecheck && timeout 120 node --test test/*.test.mjs)`, and `npm run test:vscode` for the checks inside VS Code.
- [ ] `node web/build.mjs` passes its size budget. The engine's gzipped size before this work and after it are both recorded.
- [ ] Every web test file alone: `timeout 120 node --test --test-force-exit --test-timeout=60000 web/test/NAME.test.mjs`, the browser ones with Google Chrome.
- [ ] `(cd editors/vscode && npm run build && timeout 120 npm run test:web)` and `(cd editors/vscode && TURTLE_REQUIRE_ENGINE=1 timeout 120 node --test --test-timeout=60000 test/web-worker.test.mjs)`.
- [ ] `tools/check-vocab.sh` and `tools/check-vendor.sh` pass. `npm audit --audit-level=moderate` passes.
- [ ] `node bench/perf.mjs --only big-file-peak` passes.
- [ ] `node bench/perf.mjs`, at a load average under 1, against the same run on `main`: the keystroke-latency rows do not move beyond the spread of repeated runs. The runs, their spread and the comparison are recorded.
- [ ] The spec's six budgets are each marked pass or fail under Departures, with the number measured.
- [ ] A timing test that fails beside other work is run again alone before it counts. Any test that fails is fixed, never ignored.
