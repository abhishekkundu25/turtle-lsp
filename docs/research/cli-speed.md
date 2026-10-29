# How fast are `turtle-lsp check` and `format --check`, and how to make them faster?

Research date: 2026-10-29, against commit `9ab3a95` of this repo. Claims
about this repo cite `file:line` at that commit. Claims about other projects
link to the source that owns them, pinned to a commit or a released crate
version. Every figure in the tables was measured for this note, on this
machine. Nothing was changed in `src/`: the measurement code was a temporary
example and a scratch crate, both since deleted.

## Summary

- **The commands are already fast, and half of their time is spent on
  work they do not report.** `check` on a 2.5 MB Turtle file takes 36 ms
  and `format --check` takes the same. Both run the same pipeline,
  `batch::analyse_on_disk` (`src/cli/check.rs:110`, `src/cli/format.rs:26`).
  That pipeline computes the formatter's verdict for every file
  (`src/batch.rs:283`) and the diagnostics for every file
  (`src/batch.rs:267`). `check` never reads the verdict, and `format` never
  reads the diagnostics. On single big files and on the 202-file bench
  workspace, the formatter verdict is **45 to 47% of `check`'s wall time**,
  and its safety re-parse alone is 19%. In one `check` run the text is lexed
  three times and parsed twice.
- **Our lexer and parser are not the bottleneck, and they beat the Rust
  RDF parsers.** On the same files and machine, our full parse (spans,
  references, tokens, error recovery) runs at 380 to 660 MB/s. oxttl 0.2.4
  runs at 120 to 480 MB/s and rio_turtle 0.8.6 at 260 to 330 MB/s. Only
  oxttl's `lenient` (unchecked) N-Triples mode is faster than us, at 820 to
  1,170 MB/s. Our edge comes from caching each IRI's text, so we validate
  and intern each distinct IRI once. oxttl and rio check every
  occurrence. Oxigraph's own benchmark shows the same effect: its checked
  N-Triples parse takes 132.9 ms and its unchecked one 59 ms.
- **`check` uses several cores only for reading and parsing files.** Files
  are read in parallel (`src/cli/files.rs:315`). Documents are analysed in
  parallel when there are two or more (`src/workspace.rs:1243`). The
  knowledge-index merge, SHACL, the per-file loop (diagnostics, formatter
  verdict, fixes), the column conversion and the output are sequential.
  The 202-file workspace reaches 2.7 cores, the corpus 1.35, and a single
  file 1.0. On the public corpus, **SHACL is 64% of `check`** (690 ms of
  1,076 ms) and runs on one core.
- **Parallel parsing of one file works for N-Triples and N-Quads, but is
  not where the time goes.** Parsing a 16.8 MB N-Quads file in 12 line
  chunks takes 4.2 ms instead of 25.6 ms. That would take `check` on that
  file from 78 ms to about 57 ms. Turtle and TriG cannot be split by a
  heuristic. An exact split is possible, because the lexer has no state
  between tokens and `parser::Start` already captures the parser state at a
  statement boundary. It is a large change for a 10 to 15% gain on big
  files only.
- **Recommendation:** first, stop computing what each command throws away.
  `check` should skip the formatter verdict, and `format` should skip
  diagnostics and the knowledge index. Both should reuse the analysis's
  line index for column conversion. These changes are small and low-risk,
  and they take about 45% off `check` on big files and on the bench
  workspace. Next, run the per-file loop in parallel, then look at SHACL
  focus-node selection for the corpus. Lexer micro-optimisations (byte-class
  tables, memchr) come after these, worth 2 to 5% of a `check`.

## Method

- Machine: Apple M6, 12 cores (2 "Super", 4 performance, 6 efficiency, from
  `sysctl hw.perflevel*`), macOS. **The machine was not quiet:** the load
  average was 1.5 to 2.5 during the runs, because other sessions were
  active. All figures are best-of-N, and the medians were within 2% of the
  best. Read them as indicative, not as bench results.
- Whole-command times: the shipped profile (`target/release/turtle-lsp`,
  fat LTO, mimalloc), run 5 to 11 times. A Python harness timed each run
  and read the child CPU time from `getrusage(RUSAGE_CHILDREN)`.
  `TURTLE_LSP_CACHE_DIR` pointed to a scratch folder.
- Inputs: `huge.ttl` (2.49 MB) and `vocab.ttl` (2.51 MB), generated exactly
  as `bigDoc` and `vocabDoc` in `bench/perf.mjs`. The `check-cli` workspace
  (200 files of 200 subjects, a 5,000-subject `big.ttl` and the shape file
  from `addShape`). `test/corpus`, which holds 1,356 `.ttl`, 372 `.jsonld`
  and 5 `.nt`/`.nq` files. `wdc-rdfa.nq` (16.8 MB, 57,000 lines, 30 syntax
  errors) and `dbpedia-ontology.nt` (4.1 MB) from the corpus.
- Phase split: a temporary example (`examples/zz_speed.rs`, deleted). It
  timed the public stages on one file, best of 15:
  `syntax::lexer::tokenize`, `syntax::parse_with_base`, `Analysis::new`,
  `Knowledge::set`, `features::diagnostics::diagnostics` and
  `features::format::format`. It also timed oxttl on the same bytes.
  `Parsed::graph_only` is `pub(crate)` (`src/syntax/parser.rs:120`), so a
  graph-only parse could not be timed from outside.
- Profiles: an unstripped build
  (`CARGO_PROFILE_RELEASE_STRIP=false CARGO_PROFILE_RELEASE_DEBUG=line-tables-only`)
  of the same example. It looped `turtle_lsp::cli::run` with mimalloc for
  9 s, and `sample` recorded it at 1 ms for 6 to 8 s. The shares below are
  inclusive samples on the thread that runs `cli::run`. The loop runs in
  one process, so after the first run the global IRI interner only looks
  strings up. The cost of a cold interner is measured separately
  ("parse, first in process" below).
- rio_turtle 0.8.6: a scratch crate built offline from the local cargo
  cache with the same release profile, best of 15.

## Measurements

### Whole commands (shipped binary)

| Input | `check` | `check --no-shacl` | `format --check` | CPU / wall |
|---|---:|---:|---:|---:|
| `huge.ttl`, 2.49 MB | 36.4 ms | 36.1 ms | 36.3 ms | 1.00 |
| `vocab.ttl`, 2.51 MB | 46.8 ms | 46.5 ms | 46.7 ms | 0.99 |
| 202-file workspace (`check-cli`) | 91.3 ms | 83.8 ms | 83.8 ms | 2.65 to 2.78 |
| `test/corpus` | 1,076 ms | 393 ms | 391 ms | 1.35 / 1.92 / 1.96 |
| `wdc-rdfa.nq`, 16.8 MB | | 77.9 ms | 73.4 ms (refused: syntax errors) | 1.17 |
| `dbpedia-ontology.nt`, 4.1 MB | | 84.8 ms | | 1.16 |
| `turtle-lsp help` (process start) | | | | 2.1 ms |

`format --check` costs the same as `check --no-shacl` everywhere, because
it runs the same work.

### Where the time goes (share of `cli::run` on its thread)

| Phase | `huge.ttl` check | `vocab.ttl` check | 202 files check | corpus check | `wdc-rdfa.nq` check --no-shacl |
|---|---:|---:|---:|---:|---:|
| Reading files, settings, output | about 2% | about 2% | about 3% | about 2% | about 2% |
| Analysis and knowledge index (`Input::new`, `src/batch.rs:492`) | 35% | 34% | 23% (the parses run on other cores) | 9.5% | 67% |
| of which: line index (`LineIndex::new`) | 8% | 6% | 4% | 2% | 12% |
| of which: `DocFacts` and the index merge | 8% | 9% | (other cores) | | |
| SHACL (`src/batch.rs:246`) | 0 | 0 | 10% | **64%** | 0 |
| Diagnostics (`src/batch.rs:267`) | 11% | 19% | 13% | 8% | 13% |
| **Formatter verdict (`src/batch.rs:283`)** | **47%** | **42%** | **45%** | 13% | 0 (refused) |
| of which: the safety re-parse (`same_graph`) | 19% | 18% | 19% | 5% | |
| Second line index, column conversion (`src/batch.rs:289`) | 4% | 3% | 4% | 2.5% | 14% |
| Lexing, all three passes together | 23% | 22% | 15% | 5% | |

The `huge.ttl` run, in milliseconds, from the stage timings (warm, best
of 15): parse 5.4 (lexing 2.1 of it), `Analysis::new` 7.3 in all,
`Knowledge::set` 2.0, diagnostics 3.2, formatter verdict 13.1. The first
parse in a fresh process takes 8.4 ms instead of 5.4. The extra 3 ms is
interning new strings plus first-touch memory, and the two cannot be told
apart from outside.

For `dbpedia-ontology.nt`, the stages add up to about 32 ms, but the
command takes 85 ms. I did not profile that gap (see "Not checked").

### Parser throughput on the same files (best of 15, warm)

| | `huge.ttl` 2.49 MB | `vocab.ttl` 2.51 MB | `wdc-rdfa.nq` 16.8 MB | `dbpedia-ontology.nt` 4.1 MB |
|---|---:|---:|---:|---:|
| `str::from_utf8` (validation) | 16,600 MB/s | 76,700 MB/s | 16,300 MB/s | 34,700 MB/s |
| Ours: lex only | 1,170 MB/s (2.1 ms) | 946 (2.7 ms) | 1,468 (11.4 ms) | 1,516 (2.7 ms) |
| Ours: full parse incl. lex | 465 (5.4 ms) | 379 (6.6 ms) | 657 (25.6 ms) | 584 (7.1 ms) |
| Ours: full parse, first in process | 298 | 290 | 596 | 499 |
| oxttl 0.2.4, checked | 140 (17.8 ms) | 120 (20.9 ms) | 480 (35.0 ms) | 404 (10.2 ms) |
| oxttl 0.2.4, `lenient()` | 142 | 122 | 1,172 (14.3 ms) | 817 (5.1 ms) |
| rio_turtle 0.8.6 | 332 (7.5 ms) | 314 (8.0 ms) | 259 (64.7 ms) | 259 (15.9 ms) |
| oxttl, `split_slice_for_parallel_parsing`, 12 threads | 870 | 756 | 3,299 | 2,647 |
| Ours, 12 chunks in parallel, no merge step | 692 | 647 | 4,002 | 3,245 |

What each parser produces differs. Ours keeps spans, references, prefix
uses, comments and tokens, recovers from errors, and interns terms. oxttl
yields owned `String` terms per triple. rio yields borrowed terms through
a callback. oxttl and rio validate every IRI occurrence. Ours validates
the distinct IRI texts once, through `iri_cache` and `pname_cache`
(`src/syntax/parser.rs:378-381`, used at `:1787-1823`). That is why we win
on IRI-heavy N-Triples, where oxttl's `lenient()` mode, which skips the
checks, catches up. oxttl's Turtle cost does not change with `lenient()`,
so for Turtle its time is in prefixed-name expansion and term allocation,
not in validation. The "ours, 12 chunks" row parses independent chunks
(each Turtle chunk with the prefix header copied in) and does not merge
them. It is an upper bound for the parallel parse discussed below. It also
shows that the 16 sharded interner locks (`src/syntax/iri.rs:23`) do not
stop a 6x speed-up.

Token bytes by kind: in the two Turtle files, prefixed names are 40 to 47%
of the bytes, strings 26 to 38% and whitespace 18 to 19%. In the N-Triples
and N-Quads files, IRIs are 79 to 87% of the bytes.

## 1. What `check` and `format --check` do today

1. `files::load_lenient` scans and reads the files in parallel
   (`src/cli/files.rs:315`). This is about 1.5% of the time.
2. `batch::analyse_on_disk` (`src/batch.rs:227`) builds an `Input`
   (`src/batch.rs:477`). The `Input` opens every file in a `Workspace` with
   `keep_tokens` off (`src/batch.rs:488`). It then calls `ws.knowledge()`
   through `imports::resolve_now` (`src/batch.rs:492-494`).
   `Workspace::knowledge` analyses stale documents with `into_par_iter`
   when there are two or more (`src/workspace.rs:1225-1252`). It merges
   each result into the index one at a time (`settle`,
   `src/workspace.rs:1296`).
3. `input.shacl` runs one SHACL job on the calling thread
   (`src/batch.rs:246`, `src/batch.rs:585-612`).
4. For each file, in order: `input.diagnostics` (`src/batch.rs:267`),
   `input.fixes` when asked (only the `json` and `sarif` formats), and
   `input.format` (`src/batch.rs:283`). The formatter verdict
   (`src/batch.rs:705`) calls `format::format_analysis`. That needs the
   tokens, which were dropped, so `Analysis::tokens` lexes again
   (`src/analysis.rs:352-360`). After printing, `same_graph` parses the
   output again with `Parsed::graph_only` (`src/features/format.rs:140-147`).
5. `to_code_points` builds a new `LineIndex` per file
   (`src/batch.rs:803-813`, `:851`), although each `Analysis` holds one.
6. `check` filters, sorts and renders. It never reads `FileReport::format`
   (`src/cli/check.rs`; the only reader is `src/cli/format.rs:39`).
   `format` never reads the diagnostics.

So `check` pays for the formatter, and `format` pays for the diagnostics,
the import resolution and the knowledge index. The browser engine calls
`batch::analyse` and shows both, so the shared path makes sense there.

## 2. How we compare with the fastest RDF parsers

- **oxttl (Oxigraph).** Its `lib/oxttl/README.md` makes no speed claim
  ([README at the 0.2.4 commit](https://github.com/oxigraph/oxigraph/blob/0e81a29d27575ab69e7f85ea9c7f1058b277fcd0/lib/oxttl/README.md)).
  Oxigraph benchmarks parsing with criterion on BSBM "explore 1000"
  N-Triples, checked and `lenient()`
  ([`lib/oxigraph/benches/store.rs`](https://github.com/oxigraph/oxigraph/blob/0e81a29d27575ab69e7f85ea9c7f1058b277fcd0/lib/oxigraph/benches/store.rs)).
  One CodSpeed run (CPU simulation, commit `76ec644`) reports
  132.9 ms checked and 59 ms unchecked for N-Triples
  ([CodSpeed run](https://app.codspeed.io/oxigraph/oxigraph/runs/69ff5ece4c8c4441a9f84573)).
  The page does not give the file size, so this is not a MB/s figure. The
  2.25x ratio matches ours on `wdc-rdfa.nq` (35.0 against 14.3 ms).
  oxttl uses `memchr`/`memchr2` to find the end of IRIs and strings
  (`lexer.rs` lines 212, 673-730 in the 0.2.4 crate source).
- **rio_turtle.** Its README says only "conformant and fast parsers"
  (rio_turtle 0.8.6 crate README, commit
  [`fdbe22f`](https://github.com/oxigraph/rio/tree/fdbe22f57b94050c6b11776633f9ade26622a433)).
  rio is the predecessor of oxttl. I measured it: 260 to 330 MB/s.
- **serd (C).** Its README calls serd "the fastest Turtle reader/writer by
  a wide margin". It compares `serdi` with rapper and riot on sp2b data on
  an AMD 1950x, and gives the figures only in SVG charts
  ([serd README](https://github.com/drobilla/serd/blob/b621b0bc790d1e8102d3138ead16b974ddda8432/README.md)).
  serd is not installed here, so I did not measure it.
- **sophia.** sophia_turtle 0.9 parses through rio (`sophia_rio`), so its
  parse speed is rio's. Its README points to
  [sophia_benchmark](https://github.com/pchampin/sophia_benchmark). That
  benchmark compares in-memory graph libraries (Jena, librdf, N3.js,
  Python) on the author's Ubuntu 18.10 machine. It is not a current
  parser benchmark.
- **For scale, outside RDF.** simdjson parses JSON at "gigabytes of data
  per second on a single core" with "a quarter or fewer instructions" than
  RapidJSON ([Langdale and Lemire, arXiv:1902.08318](https://arxiv.org/abs/1902.08318)).
  The logos lexer generator reports 1,037 to 1,575 MB/s on its own
  benchmarks ([logos README](https://github.com/maciejhirsz/logos/blob/fba6c4d4255bb213ca9f876f5ae8ab3a62c2433e/README.md)).
  Our hand-written lexer runs at 950 to 1,520 MB/s, which is in the same
  range.

Like with like: we cannot time `graph_only` from outside, but our full
parse already does more than oxttl and rio, and it is faster. An exact
comparison needs a `pub(crate)` bench inside the crate. A
`timing_`-prefixed unit test could measure `Parsed::graph_only` against
oxttl.

## 3. Techniques that make lexers and parsers faster

Each item says what the change would mean in `src/syntax/lexer.rs`,
`parser.rs` and `names.rs`, with a rough gain. The base figures: lexing is
37 to 45% of a parse, and a parse is about 15 to 20% of a single-file
`check` once the extra passes are removed.

1. **Skip passes (the largest win, not a lexer technique).** In `check`,
   the text is lexed three times: for the parse, for the formatter's
   tokens, and for the safety re-parse. Removing the formatter from
   `check` removes two lexes and one parse. For `format`, keep the tokens
   from the analysis (`ws.set_keep_tokens(true)`) instead of lexing again.
   That saves 8% of the run. Tokens are 12 bytes
   (`src/syntax/lexer.rs:58`), about one per 9 to 11 bytes of text, so
   they add about 1.2 bytes per input byte against the 64-byte budget.
2. **memchr / SIMD scanning for IRI, string and comment bodies.**
   `Lexer::lt` (`src/syntax/lexer.rs:266-299`) and `Lexer::string` (`:216-264`)
   check every byte against about ten cases. The comment scan (`:176-181`)
   uses `iter().position`. On the 13 MB of IRI bodies in `wdc-rdfa.nq`, I
   measured three scans: the current per-byte match loop runs at
   2,085 MB/s, a 256-entry class table that stops at the first special
   byte at 3,345 MB/s, and `str::find('>')` (core's word-at-a-time memchr)
   at 6,211 MB/s. The `memchr` crate adds SIMD on aarch64 and x86_64
   ([memchr 2.8.3 README](https://github.com/BurntSushi/memchr/blob/5fdb40c054e1fff359a2f7bdf7f87a13b34b465d/README.md)).
   It is already in `Cargo.lock` through `regex`, so a direct dependency
   adds no new crate. The design follows oxttl: find the next `>` or `\`
   with `memchr2`, then check the span for bad characters with a table.
   Expected gain: IRI scanning is about 6 of the 11.4 ms that lexing takes
   on `wdc-rdfa.nq`, and could drop to 2 or 3 ms. That is about 30% of
   lexing and 12% of parsing on N-Triples and N-Quads. Strings need
   `memchr3(q, '\\', '\n')`, a smaller gain because the strings are short.
3. **Table-driven byte classes for names.** `name_or_word`
   (`src/syntax/lexer.rs:434-480`) and `bnode` decode one `char` at a time
   (`char_at`, `:163`, which bounds-checks and UTF-8-decodes). They then
   test it against the `PN_CHARS` range chains
   (`src/syntax/names.rs:12-31`). Prefixed names are 40 to 47% of the
   bytes of a Turtle file. A `[u8; 256]` class table for ASCII, with the
   current `char` path only for bytes of 0x80 and above, would remove the
   decode and most branches. Expected gain: about 20 to 30% of Turtle
   lexing (0.5 ms of 2.1 ms on `huge.ttl`). This is the same idea logos
   describes ("Optimizes branches into lookup tables or jump tables").
4. **Fewer allocations.** Each literal allocates its value
   (`string_value(..).into()`, `src/syntax/parser.rs:1434`), and each
   language tag is validated by `oxilangtag` on every occurrence
   (`:1443`). Only the lowercased tag is cached (`:1466-1473`). Caching
   valid tags, as `iri_cache` does for IRIs, and validating only on a
   cache miss saves one parse per tagged literal (36,000 in `vocab.ttl`).
   Making literal values zero-copy (a span into the text when there is no
   escape) would remove 34,000 to 36,000 allocations per 2.5 MB file. But
   `Node` is compared and hashed across documents, so that is a wide
   change. Expected gain: under 5% of parsing for the tag cache, and 5 to
   10% for zero-copy values. `Triple` is 104 bytes and `Node` 32. Arenas
   and SmallVec would help little: the vectors are already reserved up
   front (`src/syntax/parser.rs:185`), and the profile shows allocator
   time at about 3%.
5. **Interning.** Warm lookups are about 2% of a `check`. A cold interner
   costs about 3 ms on the first parse of a 2.5 MB file, which is a third
   of the parse. That cost is the price of the IRI cache that makes us
   faster than oxttl. Pre-sizing the shard maps (16 shards,
   `src/syntax/iri.rs:23-56`) from the file size could cut rehashing.
   Expected gain: small, and not measured.
6. **Zero-copy tokens and UTF-8.** Tokens are already spans (12 bytes,
   no text). UTF-8 is validated once, by `read_to_string`, at 16 to
   77 GB/s, which is about 1 ms for 16.8 MB. Avoiding it would gain
   nothing measurable.
7. **Branch layout.** The main `match` on the first byte
   (`src/syntax/lexer.rs:171-214`) is already a dense dispatch.
   Reordering arms gains little next to items 2 and 3.
8. **Line index.** `LineIndex::new` costs 6 to 12% of a run, and the CLI
   builds it twice (item 1 of the ranked list). On non-ASCII text
   (`wdc-rdfa.nq`), it takes the slow path (`src/analysis.rs:522`). A
   memchr scan for newlines plus an `is_ascii` check per line chunk would
   speed it up, but first remove the duplicate.

## 4. Parallelism

### Across files in `check`

What must stay sequential:

- **The knowledge-index merge** (`settle`, `Knowledge::set_prepared`). It
  mutates one shared index. `DocFacts` (the expensive part) is already
  computed on the parsing threads.
- **Import resolution.** It reads the merged index.

What can run in parallel once the index is built:

- **Diagnostics per file.** `diagnose::document` takes `&Knowledge`
  (`src/diagnose.rs:66-71`), and `Knowledge` is plain maps and vectors
  (`src/knowledge.rs:599-622`), so it is `Sync`. `Input::diagnostics`
  takes `&mut self` only to reach `ws.analysis(uri)` and the memoised
  unresolved imports. Collect the `Arc<Analysis>` values and the
  unresolved map first, then `par_iter` over the files. `collect()`
  keeps input order, so the output stays deterministic.
- **The formatter verdict per file** (for `format`) depends only on the
  file's analysis.
- **The column conversion per file.**
- **SHACL next to diagnostics, with `rayon::join`.** The diagnostics take
  the file's SHACL results as an input (`src/batch.rs:266-267`). Either
  compute SHACL first (as now) or check that `compose` only appends them.
- **SHACL datasets.** `validate_unit` runs once per dataset in sequence
  and shares a triple budget, `spent`
  (`src/features/shacl_validate.rs:268-290`, `:432`). Running datasets in
  parallel needs the budget split before the run, or the "skipped" results
  change. The corpus profile puts 87% of SHACL in focus-node selection
  (`src/shacl/validate.rs:1755`, the `among` filter, inside the shape loop
  at `:1931-1937`). For every shape and every dataset, it re-reads the
  `rdf:type` of every node the dataset mentions. Looking up a type index
  per dataset once may be a bigger win than threads. This is a separate
  study.

Expected gain on the 202-file workspace: dropping the formatter verdict
takes 91 ms to about 50 ms. Running diagnostics in parallel takes another
8 to 10 ms off. On the corpus, dropping the verdict saves about 140 ms of
1,076 ms. The rest of the corpus time is SHACL.

### One N-Triples or N-Quads file, split by lines

- **How oxttl does it.** `split_slice_for_parallel_parsing` splits at the
  first `\n` after each 1/n of the slice, with `memchr`. A chunk is at
  least 16,384 bytes (`MIN_PARALLEL_CHUNK_SIZE`), and each chunk gets an
  independent parser
  ([`chunker.rs`](https://github.com/oxigraph/oxigraph/blob/0e81a29d27575ab69e7f85ea9c7f1058b277fcd0/lib/oxttl/src/chunker.rs),
  [`ntriples.rs:200-211`](https://github.com/oxigraph/oxigraph/blob/0e81a29d27575ab69e7f85ea9c7f1058b277fcd0/lib/oxttl/src/ntriples.rs#L200-L211)).
  Blank nodes stay consistent across chunks because oxttl keeps the label
  text as the node's identity (`BlankNode::new_unchecked(label)`,
  `terse.rs:223`, `:404`). The caller sees the triples of each chunk
  separately, in no global order.
- **What our merge must do.** Our output is more ordered than that:
  1. **Blank-node ids.** `labelled_bnode` numbers labels in order of first
     occurrence, with one counter for labels and `[]`
     (`src/syntax/parser.rs:825-845`). Each chunk numbers its labels
     locally. A sequential merge then maps each chunk's labels, in chunk
     order, to global ids. That gives exactly the ids of a sequential
     parse.
  2. **Statement indexes.** Shift `Triple::statement`, statement and graph
     indexes by the previous chunks' counts.
  3. **Error order.** `run` keeps lexer errors first, then parser errors,
     then the N-Triples restrictions (`src/syntax/parser.rs:186-198`, and
     the module doc at `:22-26`). Concatenate each kind separately in
     chunk order.
  4. **Spans.** Lex the whole chunk at its real offset, or add the chunk
     start to every span.
- **Why the line split is exact, with one exception.** The lexer ends a
  short string at a raw newline (`src/syntax/lexer.rs:249`). An IRI
  stops at whitespace (`:280`), and a comment stops at `\n` or `\r`
  (`:176-181`). So no token crosses a `\n`, except a long string (`"""`
  or `'''`), which the shared lexer reads across lines even in N-Triples.
  If the file contains `"""` or `'''` (one memmem pass), parse it
  sequentially. The rule "one triple per line" is local to a line.
- **Expected gain.** The parse of `wdc-rdfa.nq` takes 25.6 ms, and 12
  chunks take 4.2 ms. With the merge, perhaps 6 to 8 ms. That takes
  `check` from 78 ms to about 57 to 60 ms. Building the `Analysis` indexes
  and the line index per chunk would add more. Prove it in chaos with the
  oracle "the chunked `Parsed` equals the sequential `Parsed`", as the
  splice does.

### Turtle and TriG

A naive split breaks for these reasons:

- A `.` or a newline can sit inside a long string (`"""a .\n b"""`) or a
  comment.
- `@prefix` and `@base` can appear anywhere and change how every later
  name resolves.
- A statement spans lines. `[ ]`, `( )` and `<< >>` nest, and a TriG
  `{ }` block holds many statements.
- Error recovery depends on where the previous statement went wrong.
  `recover` resumes at the next `.` or at a column-0 token that can start
  a statement (`src/syntax/parser.rs:895`).

oxttl's Turtle split is a heuristic. It accepts a `.` when three triples
parse after it, and it learns the prefixes from the start of the file
([`chunker.rs`](https://github.com/oxigraph/oxigraph/blob/0e81a29d27575ab69e7f85ea9c7f1058b277fcd0/lib/oxttl/src/chunker.rs),
`next_terminating_char`). Its docs say it "Can fail or return wrong
results if there are prefixes or base iris that are not defined at the top
of the document, or valid turtle syntax inside literal values"
([`turtle.rs:211-212`](https://github.com/oxigraph/oxigraph/blob/0e81a29d27575ab69e7f85ea9c7f1058b277fcd0/lib/oxttl/src/turtle.rs#L211-L212)).
A checker that reports exact errors cannot accept that.

**An exact split is possible**, because of two facts the incremental
splice already relies on (see `docs/research/incremental-reparse.md`):

1. The lexer has no state between tokens, so a sequential lex is exact.
   It runs at about 1 GB/s, which is 2.1 ms on 2.5 MB.
2. `parser::Start` (`src/syntax/parser.rs:214-240`) holds everything the
   parser needs to begin at a statement: the prefixes, the base, the
   statement index, the TriG block and the token position.

The method:

1. One cheap pass over the tokens finds the top-level `Dot` tokens: at
   bracket depth 0, outside `{ }`. Pick about n of them as cuts, and note
   the prefix and base state at each cut.
2. Parse the chunks in parallel, each from its `Start`.
3. Accept a cut only if the previous chunk ended its last statement on
   that `Dot` without recovery. Otherwise, parse the next chunk again
   sequentially from where the previous chunk really stopped.
4. Merge as for N-Triples.

Gain on `huge.ttl`: the parse falls from 5.4 ms to about 2.1 ms (lexing)
plus 1 to 2 ms, so 2 to 3 ms of a `check` that would take about 20 ms
after the first fixes. That is 10 to 15%, on big files only. The change
is large, and the risk is in the merge and in recovery at the cuts. Do it
after everything above, if at all.

## 5. Options, ranked

Gains are on the shipped binary. "Big file" means `huge.ttl` or
`vocab.ttl`, and "workspace" means the 202-file bench.

| # | Change | Expected gain | Risk | Size |
|---|---|---|---|---|
| 1 | `check` skips the formatter verdict: an `Options` flag, off for the CLI's `check` and on for the browser engine | big file 36 → about 20 ms (−45%); workspace 91 → about 50 ms; corpus −13% | Very low | Small |
| 2 | `format` skips diagnostics, the knowledge index and import resolution, and keeps tokens to avoid the re-lex | `format --check` −20 to 30% (diagnostics 11%, index 8%, re-lex 8%) | Low | Small |
| 3 | Reuse each `Analysis`'s line index for the column conversion | −4% Turtle, −14% on `wdc-rdfa.nq` | Low | Small |
| 4 | Parallel per-file diagnostics, formatter verdict and conversion in `analyse_in` | workspace −8 to 10 ms after #1; scales with cores | Low to medium (output order, `&mut` helpers) | Small to medium |
| 5 | SHACL: a type lookup per dataset for focus nodes; then datasets in parallel with a pre-split budget | corpus: up to about 600 of 1,076 ms | Medium (budget and "skipped" semantics) | Medium |
| 6 | Lexer: ASCII class table for names, memchr for IRI, string and comment bodies | lexing −25 to 35%; a `check` −2 to 5% | Low (chaos, oxttl oracle, W3C tests cover it) | Small to medium |
| 7 | Language-tag validation cache; zero-copy literal values | parse −5 to 10% | Low (tag cache) to medium (zero-copy) | Small / large |
| 8 | Parallel N-Triples and N-Quads parse by line chunks, with an exact merge | `wdc-rdfa.nq` `check` 78 → about 58 ms | Medium (merge must equal the sequential parse) | Medium |
| 9 | Parallel Turtle and TriG parse from `Start` cuts | big Turtle `check` −10 to 15% | Medium to high | Large |

**Do #1 first, with #3 in the same change.** They are a few lines each,
cannot change any reported result, and take almost half off `check` on
big files and workspaces. Then do #2 and #4. Measure again with
`node bench/perf.mjs --only check-cli` and `--only big-file-peak` on a quiet
machine before choosing between #5 (corpus-like projects with shapes) and
#6 to #8 (single big files).

## Not checked

- serd, Jena RIOT and N3.js were not measured on this machine. serd's
  README figures are only in SVG charts, which I did not read.
- No published MB/s figure was found for oxttl or rio. The Oxigraph
  CodSpeed numbers are CPU simulation on an unknown file size.
- `Parsed::graph_only` could not be timed from outside the crate.
- The gap between the stage sum (about 32 ms) and the command time (85 ms)
  on `dbpedia-ontology.nt` was not profiled.
- The machine was not quiet (load average 1.5 to 2.5), so the figures are
  indicative. The profiles come from a warm in-process loop, which hides
  the cold-interner cost that a real process pays once.
- The gains for #4 to #9 are estimates from profile shares and from the
  12-chunk parse with no merge step. None of them was implemented.
- `format` without `--check` writes files. I did not time it apart from
  `--check`.
- I did not check whether `compose` does more with the SHACL results than
  append them, which matters for running SHACL next to the diagnostics.
