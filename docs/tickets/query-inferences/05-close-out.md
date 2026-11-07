# 05: Close-out

**What to build:** `CHANGELOG.md` (Unreleased: queries over inferred
triples, with the setting, flag and parameter), `README.md` (the query
section names the choice), `ARCHITECTURE.md` (the query core may reason),
the spec's status line, and this README's status column and departures.
Then the full validation, light on timing (owner's choice): every suite
the project has, one file at a time under `timeout 120`; benchmarks not
required.

**Blocked by:** 02, 03, 04.

**Status:** done

- [ ] build, `cargo fmt --check`, clippy, lib unit tests (timing tests once, rerun alone on a failure), doc test.
- [ ] Every Node test file in `test/`, every `web/test/*.test.mjs`, and the VS Code extension's node tests pass.
- [ ] The docs above name the feature and the spec's status is done.
