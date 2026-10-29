# 03: `lint.ignore` and `lint.enable` end to end

**What to build:** VS Code declares `turtle.lint.ignore` and `turtle.lint.enable` (arrays of strings; the descriptions list the opt-in codes and say ignore wins) and passes them on like the other settings. Node tests show that the lists from `turtle-lsp.json`, the editor settings and `--ignore` add up.

**Blocked by:** 01.

**Status:** done

- [x] `test/settings-file.test.mjs`: `lint.ignore` in the file hides a code (e.g. `unused-prefix`) for open documents and workspace diagnostics; the editor's `turtle.lint.ignore` adds to it; `lint.undescribedTerms: false` still works.
- [x] `test/cli.test.mjs`: `lint.ignore` in the file hides a code in `check`; `--ignore` adds to it; an opt-in code (`missing-ontology-license` once 05 lands; until then only that it is accepted by `--select`) is reported only when `lint.enable` lists it; exit codes with and without `--max-warnings`.
- [x] `editors/vscode` tests pass (`node --test test/*.test.mjs`), including any check that package.json settings match the server's keys.
