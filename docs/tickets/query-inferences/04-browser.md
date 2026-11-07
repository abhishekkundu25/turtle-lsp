# 04: Browser: the query pane's Inferred choice

**What to build:** the browser engine's query request takes `reasoning`
(default the settings' `query.reasoning`) and answers the note as an
`inferred` member, as `turtle/query` does. The query pane's Run bar gets an
"Inferred" choice (off, default rules, complete) bound to the
`query.reasoning` setting, and the results header shows the note's line. The
browser's settings list gets `query.reasoning` under Query with default
`default` (the showcase; owner's choice), as `turtle.reasoning.rules` has.

**Blocked by:** 01.

**Status:** open

- [ ] `web/test/engine.test.mjs`: the query request with `reasoning: "default"` returns an inferred row and the `inferred` member; `off` answers as before with no member.
- [ ] `web/test/editor-query.test.mjs`: on a fresh visit the sample's people query includes the inferred row and the note; turning Inferred off removes both.
- [ ] `web/test/editor-settings.test.mjs` counts the new setting.
- [ ] `node web/build.mjs` and every `web/test/*.test.mjs` file this ticket touches pass.
