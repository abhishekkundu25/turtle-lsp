# 05: JSON-LD context identity and work budget

**What to fix:** two defects in `src/jsonld/context.rs` and one in `to_rdf.rs`.

1. **The processed-context cache returns a context built for another input.**
   `process_in` clones the active context into `result`; inline context objects mutate
   `result` in place but it keeps the active context's `id` until line 1432. A remote
   context applied after an inline object (`[{...}, "url"]`) is cached under
   `Input::Id(<old id>)`, process-wide (`:1366`, `:1419-1432`, `:846-852`, `:963-970`).
   Step 21 (scoped-context validation) sees the same half-built `result`. Reproduced:
   `@graph` siblings where node 1 has `[{"zz":…}, credentials/v1]` and node 2 has
   `credentials/v1`: node 2 gets `<evil/zz>` triples it never defines; swap the siblings
   and both lose `zz` with a false `jsonld-undefined-term`. In the LSP, `a.jsonld` alone
   is clean; with `b.jsonld` in the workspace it loses a triple. The cache is the hidden
   global `ContextCache::global()` (`:894-897`).
2. **Nested remote contexts grow exponentially.** Only depth (32) is limited
   (`:91`, `:1399-1418`). `["c2","c2"]` → `["c3","c3"]` … doubles the work per level and
   misses the cache. Reproduced: depth 14 0.09 s, depth 17 0.70 s; 21 tiny files hang
   `check` and the parse thread.
3. `@json` canonicalization dedupes keys pairwise (`src/jsonld/to_rdf.rs:522-530`):
   100k keys 6 s, 200k 18 s, on every keystroke.

**Files owned:** `src/jsonld/context.rs`, `src/jsonld/to_rdf.rs`, `src/jsonld/mod.rs`
(thread the cache through `parse_document`), `src/analysis.rs` (the one call site that
constructs the processor), `src/jsonld/json.rs` (only if `map_entries` moves there as
`Object::entries()`), `src/jsonld/expand.rs` (only the `map_entries` call).
**Hands off:** `src/jsonld/{lint,rename,context_edit,loader,format,source}.rs` (ticket
18), `tests/w3c_jsonld.rs`.

**Blocked by:** nothing. Ticket 21 edits `src/analysis.rs` too: land this first.

**Status:** ready-for-agent

- [ ] `result` gets a fresh `id` after every inline `Value::Object` item and before
      `remote()` or step 21 sees it (or `Input` is keyed by content hash). Tests in
      `context.rs`: both sibling orders of the repro; two documents sharing one cache.
- [ ] The cache is passed by the owner (`Workspace`/`Analysis`); `Processor::new` no
      longer reaches for `global()`; `global()` stays for the CLI only.
- [ ] A step counter on `Processor` (remote applications + `process_in` entries) fails
      with `ContextOverflow` past a constant. Test: 20 nested doubling contexts through
      `NoNetwork` finish in under 1 s with one error.
- [ ] `jcs` dedupes with the `map_entries` approach, then sorts. Test: 100k-key `@json`
      under 200 ms in release.
- [ ] `tests/w3c_jsonld.rs` (814 pass, 2 known-fail), `tests/jsonld_*`,
      `test/jsonld.test.mjs`, `test/jsonld-syntax.test.mjs` green.
