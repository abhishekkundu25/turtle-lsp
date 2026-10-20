# Workspace query tickets

Spec: docs/specs/workspace-query.md. Built with parallel slices on the same
integration branch, after the lsp-capability tickets it builds on.

| # | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|
| 01 | Core query module and renderers | lsp-capability 01 | new src/query/ (mod.rs, render.rs); **src/lib.rs** |
| 02 | `turtle-lsp query` and the batch entry point | 01 | src/cli/*; **src/batch.rs**; new test/query-cli.test.mjs |
| 03 | Run query lens and command | 01, lsp-capability 02 and 04 | src/features/lens.rs; src/commands.rs; **src/server.rs**; **src/settings.rs**; editors/vscode/package.json; new test/query-lsp.test.mjs |
| 04 | Browser Run button and results pane | 02 | web/engine/src/api.rs; web/app/editor*.js, editor.css; web/test/ |
| 05 | Close-out | 01–04 | README, CHANGELOG, VS Code README, bench row; full validation |

Waves: 01 (may start during lsp-capability wave 1 once its 01 lands); then
02 and 03 together; then 04; then 05.
