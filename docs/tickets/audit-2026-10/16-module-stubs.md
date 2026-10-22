# 16: Module stubs

**What to build:** the empty module declarations later slices need, so that no two
slices edit `src/lib.rs` or `src/syntax/mod.rs` at the same time. Also a module index in
the crate doc: one line per module saying what it owns and who calls it (the only
in-repo module map today is a local-only file).

**Files owned:** `src/lib.rs`, `src/syntax/mod.rs`, new empty `src/fetch.rs`,
`src/diagnose.rs`, `src/ide/mod.rs`, `src/layout/mod.rs`, `src/syntax/names.rs`
(each with a `//!` doc stating its purpose and "filled by ticket NN").
**Hands off:** everything else.

**Blocked by:** wave 1 landed. **Run first in wave 2.**

**Status:** ready-for-agent

- [ ] `pub mod fetch; pub mod diagnose; pub mod ide; pub mod layout;` in `src/lib.rs`
      (wasm: `fetch` behind `cfg(not(target_arch = "wasm32"))` like `cli`), `pub mod
      names;` in `src/syntax/mod.rs`.
- [ ] `src/lib.rs` crate doc gains a module index table.
- [ ] `cargo build`, `cargo clippy --all-targets -- -D warnings` (no dead-code warnings:
      use `#![allow(dead_code)]` with a comment only if an empty module warns), the
      web engine builds (`cd web/engine && cargo check --target wasm32-unknown-unknown`
      if the target is installed).
