# 01: Settings seam

**What to build:** `lint.ignore` and `lint.enable` in `Settings` (lists that add up across layers; the file may set them), `Settings::lint_on(code)`, the composer's filter in `diagnose::document`, `diagnostics::Options::house_style`, the stub `features::curation` with the codes and `OPT_IN`, and the eleven new codes in `cli::output::CODES`.

**Blocked by:** none.

**Status:** done

- [x] `settings::tests::lint_lists_add_up_and_ignore_wins`.
- [x] Lib tests, clippy and fmt pass.
