# 19: Diagnostics composer and quick fixes

**What to fix:** "the diagnostics of one RDF document" is assembled in three places:
open documents in the server (`diagnostics_with` + imports + `needs_index` filter +
cached SHACL, `src/server.rs:1225-1263`), closed files in workspace diagnostics
(`batch::rdf_diagnostics` + SHACL, `src/server/workspace_diagnostics.rs:283-287`), and
the CLI/web (`batch::Input::diagnostics`, `src/batch.rs:420-442, 539-587`). README
promises `check` reports "what the editor would show"; nothing forces the three to agree.
`batch.rs` also has its own position conversion (`byte_offset`, `Lines`,
`:767-857`): O(lines) per edit, CRLF clamps between `\r` and `\n`, a line past the end
drops the fix, unlike `analysis::offset_in`.

Quick fixes: `analyse_in` computes fixes for **every** diagnostic through the LSP
`code_actions`, and each call rebuilds the file-wide "Add all missing prefixes" action
(`batch.rs:246, 589-640`; `features/diagnostics.rs:542-560`), so `check` is quadratic in
undeclared-prefix uses (5k lines 0.26 s, 20k lines 4.2 s) even when the output never
shows fixes. `diagnostics.rs` is three modules in one file (assembly, quick fixes, prefix
insertion helpers); the fix contract is an implicit JSON shape in `Diagnostic.data`.
Several features take a `text: &str` that must equal `a.text`.

**Files owned:** new `src/diagnose.rs` (move `rdf_diagnostics`/`sparql_diagnostics`
from `batch.rs`; `DiagnosticInputs { settings, import_status, shacl, indexed }`),
`src/batch.rs`, `src/server/workspace_diagnostics.rs`, `src/server.rs` (`diagnostics_for`
and `publish_dirty` only), `src/features/diagnostics.rs` → `diagnostics.rs` +
new `src/features/quickfix.rs` (typed `FixData`; per-file "add all" built once),
`src/cli/check.rs` (ask for fixes only for `json`/`sarif`/`--fix`),
`src/features/{completion,symbols}.rs` (drop redundant `text` params), `tests/chaos.rs`
(call sites only), `test/workspace-diagnostics.test.mjs`, `test/cli.test.mjs` (new cases).
**Hands off:** the rest of `src/server.rs` (ticket 13 landed first), `src/features/lint.rs`.

**Blocked by:** tickets 13 (`server.rs` lock #1), 15 (`diagnostics.rs` prefix helpers
moved), 16 (`diagnose` stub), 12 (`check.rs`).

**Status:** ready-for-agent

- [ ] One composer `diagnose::document(doc, k, enc, &DiagnosticInputs) -> Vec<Diagnostic>`
      used by the server's `diagnostics_for`, workspace diagnostics and `batch::Input`;
      the `needs_index` filter, the cached-import exemption and SHACL ordering live
      there once. Rust test: open-document output == batch output for the same input.
- [ ] `batch::byte_offset` and `Lines` deleted in favour of `analysis::offset_in` /
      `position_in`. Tests: `fix_all` on CRLF text; a fix past the last line.
- [ ] Fixes computed lazily (`Options { fixes: bool }`), only for `json`, `sarif` and
      `--fix`; `code_actions` split so a per-diagnostic call does not rebuild the
      file-wide action. Release test: 20k undeclared uses `check --statistics` under 1 s.
- [ ] `quickfix.rs` with a typed `FixData` enum serialised in one place; `text`
      parameters dropped from `complete`, `document_symbols`, `code_actions`.
- [ ] `cargo test batch::`, `tests/format.rs`, `test/cli.test.mjs`,
      `test/workspace-diagnostics.test.mjs`, `test/lsp.test.mjs`,
      `test/prefix-import.test.mjs`, `test/unknown-term.test.mjs`, `web/test/engine.test.mjs`
      green.
