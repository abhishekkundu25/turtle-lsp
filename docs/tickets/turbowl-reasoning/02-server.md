# 02: Background runs and diagnostics

**What to build:** the server runs reasoning as it runs SHACL: on open, on save and after a pause in typing (its own typing timer), on the reasoning thread, with work-done progress "TurbOWL: reasoning…" after a short delay, a stale run's result dropped by generation, and the size limit skipping with an information diagnostic `reasoning-skipped` naming `turtle.reasoning.maxTriples`. TurbOWL's diagnostics become LSP diagnostics (code `turbowl/<code>`, severity mapped, the proof as indented text in the message) on the lines where their terms are subjects, in open documents and in workspace diagnostics for closed ones. The embedded server (`Spawn::Inline`) runs it inline on the timer as SHACL does.

**Blocked by:** 01.

**Status:** todo

- [ ] New `test/reasoning.test.mjs`: a disjointness contradiction is a diagnostic with its proof; an edit updates diagnostics after the pause; the size limit gives the note; progress begins and ends; `off` publishes exactly today's diagnostics.
- [ ] The existing `test/shacl-validate.test.mjs` still passes.
- [ ] clippy and the lib unit tests pass.
