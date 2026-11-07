# 02: Command line: `--reasoning`

**What to build:** `turtle-lsp query --reasoning <off|default|complete>`
(default `off`), parsed in the argument parser with the other query flags
and listed in `--help`, an unknown value an argument error. It sets the
query's `query.reasoning` for the run. When the note is added or skipped,
one line goes to stderr ("Includes N inferred triples (TurbOWL, default
rules)", plus "TurbOWL reported K findings" when K > 0; or "Inferred triples
skipped: the workspace has N triples, over the reasoning limit of M"), so
stdout stays the results in every output format.

**Blocked by:** 01.

**Status:** open

- [ ] `test/query-cli.test.mjs`: `--reasoning default` returns a row only an inference gives, and prints the note on stderr; stdout parses as before in `--format json`; without the flag the output is unchanged; `--reasoning nope` exits 2 with an argument error.
- [ ] `cargo fmt --check`, clippy and the CLI unit tests pass.
