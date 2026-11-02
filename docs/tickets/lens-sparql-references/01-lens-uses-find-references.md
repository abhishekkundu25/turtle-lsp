# 01: One search for a term's uses; the lens uses it

Status: done. Blocked by: none.

## What to build

Move the RDF-term branch of `Session::references` (`src/ide/navigation.rs`):
the knowledge index's references plus `sparql_features::term_references`
over `Workspace::sparql_documents`, into one `Session` helper that takes an
IRI and `include_declaration`. `Session::references` and
`Session::resolve_code_lens` (`src/ide/display.rs`) both call it.
`features::lens::resolve` takes the locations instead of `&Knowledge`.

Delete the SPARQL exclusion from the module comment of
`src/features/lens.rs`, and keep the rest of it true.

## Acceptance

- [x] `ide::display::tests::a_reference_lens_counts_the_uses_in_sparql_files` passes.
- [x] `code_lenses_count_references_in_turtle_and_run_a_query_file` expects "2 references".
- [x] The lens unit tests in `src/features/lens.rs` pass, changed only for the new signature.
- [x] `ide::navigation` reference tests pass unchanged.
- [x] `web/test/lsp.test.mjs` checks that the lens on `id:acme` in the published workspace resolves to "4 references".
- [x] `cargo fmt --check` and clippy with `-D warnings` pass; the lib unit tests pass.
