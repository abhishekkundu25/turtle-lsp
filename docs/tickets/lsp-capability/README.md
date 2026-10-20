# Seams and LSP capability tickets

Spec: docs/specs/lsp-capability.md. Built with parallel slices on the
integration branch `lsp-capability`.

| # | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|
| 01 | Shared `rdf` module (graph and SPARQL dataset) | none | new src/rdf/; src/shacl/graph.rs (moves), src/shacl/sparql.rs, src/shacl/*, src/features/shacl_*.rs imports; **src/lib.rs** |
| 02 | Command module | none | new src/commands.rs; **src/server.rs** (executeCommand arm, advertised commands); **src/lib.rs** |
| 03 | `imports.unresolved` setting | none | **src/settings.rs**; src/features/shacl_validate.rs; editors/vscode/package.json |
| 04 | Reference lenses | none | new src/features/lens.rs; src/features/mod.rs; **src/server.rs** (capability, codeLens arms); **src/settings.rs**; editors/vscode/src/extension.ts; plugin/turtle_lsp.lua; new test/lens.test.mjs |
| 05 | Workspace diagnostics | none | **src/server.rs**; src/workspace.rs; **src/settings.rs**; editors/vscode/package.json; new test/workspace-diagnostics.test.mjs |
| 06 | Close-out | 01–05 | README, CHANGELOG, VS Code README; full validation |

Wave 1: 01–05 together. The locks are shared files whose edits are in
different places (new `pub mod` lines, separate match arms, separate settings
fields); conflicts are resolved at landing. Land order: 01, 02, 03, 04, 05.
Every slice keeps the keystroke rows of bench/perf.mjs unchanged; 02, 04 and
05 measure them. No slice edits README or CHANGELOG; 06 does.
