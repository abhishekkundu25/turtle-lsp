# Tickets: reasoning and proofs through TurbOWL (phase 1)

Spec: [docs/specs/turbowl-reasoning.md](../../specs/turbowl-reasoning.md).
Lock files are in **bold**. TurbOWL's own ticket is
`~/repos/turtle-reasoner`'s `docs/tickets/rdf12/01` (branch `rdf12-terms`).

| # | Ticket | Status | Blocked by | Files |
|---|---|---|---|---|
| 00 | TurbOWL: a triple term is an opaque term (other repository) | done (TurbOWL 9248fc3) | — | `crates/turbowl/` there |
| 01 | [The reasoning core and the setting](01-core.md) | done (ef20c9b) | 00 | **`Cargo.toml`**, **`Cargo.lock`**, **`src/lib.rs`** (one `mod` line, public-interface doctest), new `src/reasoning/`, `src/settings.rs`, **`src/ide/mod.rs`** (one `Session` field), **`src/server.rs`** (one field and its init, the `Session::new` argument), `editors/vscode/package.json`, `editors/vscode/src/client.ts`, `editors/vscode/test/manifest.test.mjs`, `web/app/settings.js` |
| 02 | [Background runs and diagnostics](02-server.md) | done (5343681) | 01 | **`src/server.rs`**, new `src/server/reasoning.rs`, `src/server/{timers,progress,workspace_diagnostics}.rs`, `src/diagnose.rs`, `src/workspace.rs` (`Effects`), new `test/reasoning.test.mjs` (diagnostic cases) |
| 03 | [Hover with proofs](03-hover.md) | done (77b8503) | 01 | `src/features/hover.rs`, `src/ide/display.rs`, `src/reasoning/proof.rs`, `test/reasoning-hover.test.mjs` |
| 04 | [Browser and close-out](04-close-out.md) | done | 02, 03 | `web/engine/`, `web/test/lsp.test.mjs`, `README.md`, `CHANGELOG.md`, `ARCHITECTURE.md`, `docs/specs/reasoning.md` (status line), this README |

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
- 01: the level setting is `turtle.reasoning.rules` (`off`/`default`/`complete`), not `turtle.reasoning`: VS Code cannot hold a string setting beside `turtle.reasoning.maxTriples`.
- 01 built the TurbOWL-to-LSP diagnostic conversion (`reasoning::diagnostics`) and the worker's `Outcome` carries it; 02 wires it in. A finding shows once per statement whose subject is one of its terms.
- 01 creates the shared state from the settings at start only; 02 makes a settings change create or drop it.
- 02: `Effects.shacl` is renamed `Effects.data`, since a data change now schedules reasoning as well as SHACL. Reasoning also runs when a document opens or closes, not only on save and after a pause in typing.
- 04: the browser engine gets TurbOWL through turtle-lsp's dependency (no default features, `rdf-12`), not one of its own. With TurbOWL the engine weighs 3.34 MB gzipped, over the 3.1 MB budget; `WASM_GZIP_BUDGET` is raised to 3.7 MB. The proof wait no longer reads the clock when the search ran inline: `recv_timeout` panicked in the browser, which has none.
- 04: the run's generation is no longer kept in the shared state, nor `Proof::leaves`: only tests read them. The reasoning tests read the state and the proof's origins directly; the server's settings test checks the findings after each run.
