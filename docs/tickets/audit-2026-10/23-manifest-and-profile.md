# 23: Cargo manifest and release profile

**What to fix:** `Cargo.toml` and its siblings (a lock file: runs alone).

- No `[profile.release]`: defaults (no LTO, 16 codegen units, unstripped, 17.4 MB,
  29,943 symbols) in a project whose goal is speed; the wasm build already uses `lto =
  true, codegen-units = 1, strip = true`. `panic = "abort"` is not an option: `guard()`
  and the query `Stop` unwinding rely on `unwind`.
- `spareval = "0.2"` with a `[patch.crates-io]` path at 0.2.7: once 0.2.8 is published,
  `cargo update` resolves to it, the patch is ignored with a warning, and both builds
  fail on `with_check` (`Cargo.toml:22`, `web/engine/Cargo.toml:27-29`).
- The `regex` feature trim (`default-features = false`) has no effect: spareval enables
  the defaults and Cargo unifies them (`cargo metadata` shows every `perf-*`/`unicode-*`
  on). `docs/research/shacl-engine.md:18` credits the trim.
- No `rust-version`: a distro cargo without rustup gets let-chain syntax errors instead
  of an MSRV message (`build.lua` fallback, README "From source").
- `deny.toml` ignores for RUSTSEC-2026-0194/0195 cite only `imports.rs`; quick-xml
  0.37.5 is also reached through `sparesults` (never used to parse); no review date.
- `editors/vscode` `@types/node` 26.x against an engine whose host runs Node 20.
- Two `oxiri` versions (0.3.1 direct, 0.2.11 via Oxigraph): note the reason in the
  manifest.

**Files owned:** `Cargo.toml`, `web/engine/Cargo.toml`, `Cargo.lock` /
`web/engine/Cargo.lock` (only as `--locked` requires after the pin), `deny.toml`,
`editors/vscode/package.json` (`@types/node` pin and its Dependabot ignore in
`.github/dependabot.yml`: coordinate with ticket 08/24, which own that file; or leave the
ignore to them).
**Hands off:** everything else.

**Blocked by:** wave 1. Runs alone (lock).

**Status:** ready-for-agent

- [ ] `[profile.release] lto = "fat", codegen-units = 1, strip = "symbols"` (keep
      `panic = "unwind"`); `npm run bench` before and after, numbers recorded in the
      commit message; keep the change only if keystroke p50/p95 do not regress.
- [ ] `spareval = "=0.2.7"` in both manifests; `cargo build --locked` in both.
- [ ] Regex trim removed (or justified with a measurement) and the research doc line
      corrected (ticket 22 owns the doc: leave a note for it).
- [ ] `rust-version` set to what the code needs (let-chains on edition 2024 need 1.88;
      verify with `cargo msrv` if available or by reasoning from the pinned toolchain).
- [ ] `deny.toml` ignore reasons extended (sparesults path, "CLI runs the cap inline,
      ~0.3 s/MB") with a re-check date; `cargo deny check` green.
- [ ] `@types/node` pinned to the Node major of the minimum VS Code (20);
      `npm run typecheck` in `editors/vscode` green.
- [ ] `cargo test --release --locked`, `cargo clippy`, `web/engine` `cargo check
      --target wasm32-unknown-unknown` green.
