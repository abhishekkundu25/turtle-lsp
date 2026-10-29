# 02: `format` skips diagnostics and import resolution

**What to build:**

- `batch::Options` gains `diagnostics: bool` (default true). With it off,
  `Input::new` does not resolve imports and `analyse_in` computes no
  diagnostics, fixes or SHACL; `FileReport::problems` is empty.
- The verdict is unchanged: the project prefixes still reach each document.
  For a SPARQL file with errors, the error count in "the query has N
  errors" comes without the full diagnostics run and equals the old count.
- `cli::format` sets `diagnostics: false`.
- Skip the knowledge index merge only if documents are still analysed in
  parallel and every verdict is the same; else keep it. Keep tokens to
  avoid the verdict's re-lex only if `big-file-peak` stays within budget
  with margin; else keep the re-lex. Say in the report which were done.

**Blocked by:** 01.

**Status:** done

- [x] Unit test: the report with `diagnostics: false` has the same verdicts
  as with it on, and no problems; on a SPARQL file with errors the refusal
  message is the same.
- [x] Unit test: a file with `owl:imports` is formatted without any import
  being loaded (no network, no cache read).
- [x] The format CLI tests green, unchanged.

Done as: `format` still builds the knowledge index, which is how the
workspace analyses its files in parallel; it skips imports, diagnostics,
fixes and SHACL. Tokens are not kept (the verdict re-lexes), so peak memory
does not move. The SPARQL "the query has N errors" count still runs that
file's diagnostics, without loaded imports; a SPARQL file's errors do not
depend on imports. The import test checks that the input loads none,
without reading the import cache.
