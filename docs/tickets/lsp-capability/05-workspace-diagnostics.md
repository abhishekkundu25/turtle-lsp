# 05: Workspace diagnostics

**What to build:** setting `diagnostics.workspace` (default false). When on, after indexing, diagnostics for every workspace file that isn't open are computed on a background thread at low priority and published; recomputed (debounced) when a file changes on disk or a change affects other files; yields to edits. Opening a file switches to live diagnostics; closing hands it back. Turning the setting off clears every closed file it reported. Same diagnostics as `turtle-lsp check` gives for that file.

**Blocked by:** None (can start immediately).

**Status:** ready-for-agent

- [ ] Node LSP tests: closed files match `check`; open/edit/close hand-over; a file changed on disk; setting off clears; with the setting off nothing is published for closed files.
- [ ] A keystroke is answered within the usual budget while the background pass runs (test with a few hundred files).
- [ ] Keystroke rows of bench/perf.mjs unchanged with the setting off.
