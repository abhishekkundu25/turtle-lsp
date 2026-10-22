# 38: Syntax error cap: 1,000 in the editor, none in `check`

**What to change (owner decision, replaces assumed decision 9):** ticket 34a capped
Turtle, TriG, N-Triples and N-Quads syntax errors at 100 per document inside the parser
(`src/syntax/parser.rs`: `MAX_ERRORS`, `ERRORS_HELD`, `report`, `keep_first_errors`,
`cap_errors`). The owner decided: the editor shows at most 1,000 syntax diagnostics per
document; `turtle-lsp check` (and `format`, `query`) report every error, as 0.7.1 did.

- The parser keeps every error again (no cap, no `ERRORS_HELD` dropping). Keep 34a's
  borrowed fixed messages (`Cow`), so each error stays cheap.
- The cap moves to where syntax errors become editor diagnostics: a new field on
  `features::diagnostics::Options`, e.g. `max_syntax_errors: Option<usize>`, default
  `Some(1000)` for every editor path (the server through `diagnose`, the browser editor
  through `Embedded`), `None` for the CLI. Past the cap: the first 1,000 by position
  (ties by report order, as 34a did) plus one note, "More than 1000 syntax errors; the
  rest are not shown", at the end of the last error shown, as 34a's note was.
- JSON-LD keeps its own cap (`jsonld::json::MAX_ERRORS`, 100) unchanged.
- SPARQL is unchanged.

**Files owned:** `src/syntax/parser.rs` (the cap code, its doc, its tests),
`src/features/diagnostics.rs` (`Options` and the syntax-error conversion), `src/batch.rs`
(only where `Options` is built for the CLI), `src/cli/check.rs` and the other CLI
commands only where they build `Options`, `src/diagnose.rs` only if `Options` flows
through it, `test/cli.test.mjs` (new cases), `test/lsp.test.mjs` (new case).
**Hands off:** everything else; `src/jsonld/**`.

**Blocked by:** 34a, 32b.

**Status:** done. The browser toolbox check (batch::analyse) keeps the editor cap. A file with 32,000 errors now peaks at about 42 B per input byte in check and format (was 34-35 with the parser cap); the error-free bench file is unchanged at 35.4.

- [ ] Parser: a document with 1,500 broken statements gives 1,500 errors from
      `syntax::parse`, in today's order; no note.
- [ ] Editor: the same document over LSP (Node test) publishes exactly 1,001 syntax
      diagnostics: the first 1,000 by position and the note. 1,000 errors give 1,000 and no
      note.
- [ ] CLI: `turtle-lsp check` on the same file reports all 1,500 (text and `--format
      json`), and the summary counts 1,500.
- [ ] Memory: peak RSS of `check --no-shacl` and `format --check` on the bench's big file
      (`node bench/perf.mjs --only big-file-peak`) stays under the D9 budget of 40 B per
      input byte. Measured under load; rerun quietly in 36.
- [ ] Green: `cargo test --release --locked`, clippy, fmt, `test/cli`, `test/lsp`,
      `test/format`, `test/invalid-literal`, `test/jsonld-syntax`, web/test engine and lsp.

Rules: test runs under 2 minutes (timeout 120); never point TMPDIR inside the repo; no
timing benchmarks; no new dependency; changes left uncommitted.
