# 04: Browser and close-out

**What to build:** the browser engine builds with TurbOWL (one thread, inline) and shows the same inferred type in hover; the docs say what phase 1 shipped.

**Blocked by:** 02, 03.

**Status:** todo

- [ ] `web/engine` depends on `turbowl` (no default features, `rdf-12`); `node web/build.mjs` passes within the engine's size budget, or the budget is raised on purpose with the reason in the commit.
- [ ] One case in `web/test/lsp.test.mjs`: inferred type in hover, native and wasm agree.
- [ ] `CHANGELOG.md` (Unreleased, Added), `README.md` feature list, `ARCHITECTURE.md` (the `reasoning` module and `server::reasoning`), `reasoning.md`'s status line pointing at the new spec, this README's statuses and departures.
- [ ] The handover file `docs/handover-turbowl.md` is removed, its content now in the spec.
