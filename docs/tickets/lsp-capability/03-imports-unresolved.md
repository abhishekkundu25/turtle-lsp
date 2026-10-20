# 03: `imports.unresolved` setting

**What to build:** setting `imports.unresolved`, `"soften"` (default, today) or `"ignore"`. With `"ignore"` SHACL results keep full severity and no "may change once these imports resolve" note; the unresolved-import information diagnostic and its fetch quick fix stay. Works through initialization options, didChangeConfiguration (revalidates), `turtle-lsp.json` and the CLI. VS Code contributes `turtle.imports.unresolved` with a description.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Rust test in the SHACL validation feature: same input, both values.
- [ ] Node LSP test: switching the setting changes severity and note; the info diagnostic stays.
- [ ] CLI test with `turtle-lsp.json`.
