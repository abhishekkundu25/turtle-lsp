# 01: The oracle: incremental equals a fresh workspace; chaos edit mode

**What to build:** the one test seam of the spec ("The seam is the
workspace"), before anything incremental exists, so every later ticket is
checked by it.

- `PartialEq` on `Parsed` and every type it holds (`Statement`,
  `GraphBlock`, `Ref`, `PrefixDecl`, `PrefixUse`, `Triple`, `SyntaxError`,
  `lexer::Token`, and whatever else the derive needs). Derives only; no
  other edit to `src/syntax/mod.rs` (04 adds a `mod` line there).
- `Knowledge::snapshot` loses `#[cfg(test)]` and becomes `pub(crate)`; no
  other edit to `src/knowledge.rs` (02 owns it).
- A new module `src/oracle.rs`, `#[doc(hidden)] pub mod oracle` in
  `src/lib.rs`, listed in the "Public interface" doctest. Its entry point
  takes a syntax, a start text and a list of batches of LSP content changes
  (ranged or full), applies them through `Workspace::open_doc` /
  `Workspace::change_doc` with analysis between some batches (as a client
  that asks for diagnostics after some keystrokes), and compares with a
  fresh workspace opened on the final text: the parse (`Parsed`, errors
  included), the analysis maps, `Knowledge::snapshot`, and the composed
  diagnostics (`diagnose::document`). It returns `Ok(())` or the first
  difference, named by field.
- `tests/chaos.rs` gains an edit mode: per iteration, take a corpus or
  fixture text, turn each mutation it already makes into a ranged change on
  the previous text (not a fresh text), chain 1 to 4 changes per batch, and
  run the oracle. New mutations for the splice hazards (spec, Testing
  Decisions): insert or delete `"""`, `"`, `#`, `<`, `@prefix ex: <x> .`,
  `_:b`, `{`, `}`, a `.` after a number, a newline before a column-0
  subject. Replay line as today.

Today the incremental path is a full rebuild, so the oracle must pass
everywhere; a failure now is a bug in the oracle (or a real existing bug:
fix it with a unit test on the reduced input).

**Blocked by:** none.

**Status:** done

- [x] Unit tests in `src/oracle.rs`: a ranged edit, a two-change batch, a
  full-text change, a CRLF text and a multi-byte text all pass; a deliberate
  difference (compare against a different final text) is reported with the
  field name.
- [x] `tests/chaos.rs` edit mode runs in the default chaos run, within the
  per-iteration watchdog, and passes at `CHAOS_ITERS=20000`.
- [x] `cargo test --release --locked --doc` passes with the new public path.
