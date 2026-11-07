# Tickets: reasoning and proofs through TurbOWL (phase 1)

Spec: [docs/specs/turbowl-reasoning.md](../../specs/turbowl-reasoning.md).
Lock files are in **bold**. TurbOWL's own ticket is
`~/repos/turtle-reasoner`'s `docs/tickets/rdf12/01` (branch `rdf12-terms`).

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 00 | TurbOWL: a triple term is an opaque term (other repository) | in progress | — | `crates/turbowl/` there |
| 01 | [The reasoning core and the setting](01-core.md) | todo | 00 | **`Cargo.toml`**, **`Cargo.lock`**, **`src/lib.rs`** (one `mod` line, public-interface doctest), new `src/reasoning/`, `src/settings.rs`, **`src/ide/mod.rs`** (one `Session` field), **`src/server.rs`** (one field and its init, the `Session::new` argument), `editors/vscode/package.json`, `editors/vscode/src/client.ts`, `editors/vscode/test/manifest.test.mjs`, `web/app/settings.js` |
| 02 | [Background runs and diagnostics](02-server.md) | todo | 01 | **`src/server.rs`**, new `src/server/reasoning.rs`, `src/server/{timers,progress,workspace_diagnostics}.rs`, `src/diagnose.rs`, `src/workspace.rs` (`Effects`), new `test/reasoning.test.mjs` (diagnostic cases) |
| 03 | [Hover with proofs](03-hover.md) | todo | 01 | `src/features/hover.rs`, `src/ide/display.rs`, `src/reasoning/proof.rs`, `test/reasoning-hover.test.mjs` |
| 04 | [Browser and close-out](04-close-out.md) | todo | 02, 03 | `web/engine/`, `web/test/lsp.test.mjs`, `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/specs/reasoning.md` (status line), this README |

## How the lock files stay apart

- **`src/server.rs`**: 01 adds the state field, its init and the `Session::new` argument; 02 owns every other hunk; 03 does not touch it.
- **`src/ide/mod.rs`**: only 01 (the field and its accessor). 03 reads it.
- **`Cargo.toml` / `Cargo.lock`**: only 01 (and 04 for `web/engine`'s own manifest and lock).
- 02 and 03 share no file: 03 writes its Node tests in their own file.

## Waves

1. **00** (running), then **01**.
2. **02, 03** together.
3. **04**.

## Departures

- 2026-11-07: phase 2 (completion ranking, type hierarchy, SHACL with inference) left for a later spec; owner asked to finish quickly with light verification.
