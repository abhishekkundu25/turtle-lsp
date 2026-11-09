# 03: A `query.*` change re-runs nothing

**What to build:** spec item 4. A settings change whose only differences are
`query.*` keys stores the settings and schedules no reasoning, SHACL
validation or re-check.

**Blocked by:** none. **Status:** done (3ca2f33)

- [ ] A test (server unit test or Node over the protocol) that a `query.reasoning` change does not start a reasoning run or re-validate, while a `reasoning.rules` change still does.
- [ ] fmt, clippy, lib unit tests, `test/reasoning.test.mjs`, `test/shacl-validate.test.mjs`.
