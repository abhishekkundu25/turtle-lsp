# Chaos hardening: every mutated input keeps the graph, the fixed point and the clock

Status: proposed (2026-10-23), revised after review. Tickets:
docs/tickets/chaos-hardening/ (GitHub tracking issue #23). Root-cause reports
and the review are outside the repo (scratchpad chaos10/).

## Problem Statement

The nightly chaos run (200,000 mutated inputs per format, fresh seed) found
two bugs on its first night and the fixes were one-line patches at the point
of failure. Twenty-one local runs of the same size show that was a sample of
a larger population: 12 of 21 runs failed, 15 failures and two hangs, from
five root causes. Each cause is a rule that an edit, the formatter or the
parser relies on without the code enforcing it. Patching the symptom (as
PR #14 did for two adjacent IRIs, and as a reverted change tried for the
declaration point) leaves the siblings in place.

### The twenty-one runs (main at 94d58a1, 200,000 iterations each)

| batch | run | seed | result |
|---|---|---|---|
| 0 | - | 1792760239 | A: Extract prefix changed the graph (turtle 11540) |
| 1 | 1 | 1792768158 | pass |
| 1 | 2 | 1792776077 | pass |
| 1 | 3 | 1792783996 | A (73609) |
| 1 | 4 | 1792791915 | pass |
| 1 | 5 | 1792799834 | E: `mutated_sparql_never_panics_3` still running after 600 s (iteration 107183) |
| 1 | 6 | 1792807753 | B: second format refused (10348) |
| 1 | 7 | 1792815672 | B (35229), C: second format differs (195134), D: Shorten all changed the graph (149959) |
| 1 | 8 | 1792823591 | A (28769), B (95806) |
| 1 | 9 | 1792831510 | B (110860) |
| 1 | 10 | 1792839429 | pass |
| 2 | 1 | 1792769971 | pass |
| 2 | 2 | 1792777890 | A (112031) |
| 2 | 3 | 1792785809 | pass |
| 2 | 4 | 1792793728 | pass |
| 2 | 5 | 1792801647 | pass |
| 2 | 6 | 1792809566 | pass |
| 2 | 7 | 1792817485 | E: SPARQL shard 3 hung (iteration 107745, found by regenerating the shard); C in turtle shard 1 (49915, found by replaying the shard alone) |
| 2 | 8 | 1792825404 | B (55759) |
| 2 | 9 | 1792833323 | B (13323) |
| 2 | 10 | 1792841242 | C (123532), B (177942) |

By class: A 4, B 7, C 3, D 1, E 2. Class B is a test bug, so the shipped
binary had 10 real failures in 4.2 million inputs. Two of those failures
only surfaced because a timeout hid a shard's report and a hang gave no
iteration number: the harness reports are part of the problem.

### The five causes

**A. A declaration is inserted by text search, not at a token boundary.**
`declaration_point` (src/syntax/names.rs) puts a new `@prefix` at the first
`\n` after the last declaration. That byte can be mid-statement (11540,
73609, 112031) or inside a `"""` string (28769, a `\r`-only file whose only
`\n` is inside a literal). SPARQL `add_prefix` (src/sparql/features/edits.rs)
has the same rule. Completion auto-import, the undeclared-prefix quick fixes
and "Add all missing prefixes" share the inserter. Seven sibling inputs
reproduce on the shipped binary, including every `\r`-only file. The
inserted text itself is not safe either: in the leading-break arm no line
break follows it, so `@prefix x: <..> .` before a `1` lexes as `.1`.

**B. The chaos harness reparses the formatter's output without the document
base.** The first pass parses under `file:///chaos/doc.ttl`; the second calls
`parse_doc(&out, syntax, None)`. A relative `BASE <hbase/>` is valid Turtle
that resolves against the document URI, and with no base at all it is an
error, so the second format is refused. The formatter and the server are
right (all seven inputs format to a fixed point over LSP under `file:`,
`untitled:` and `urn:` URIs). The test is wrong.

**C. The formatter keeps a redundant `;` when a comment trails it, then
moves the comment past the `.`.** On the next pass the comment trails the
`.`, so the `;` is dropped: `<o> ; # c\n.` prints `<o> ; . # c` then
`<o> . # c`. The exemption in tests/chaos.rs and the pending test only cover
one occurrence per file; all three inputs have two. TopBraid writes files
in this shape.

**D. Edits that replace, insert or delete a token check at most one
character of context, on one side, against the old text.** `spaced`
(names.rs) has the two-character rule for a `.` on the right but not on the
left, so `ex:a.b.<http://s/V>` shortened becomes `ex:a.b.sht:Validate`, one
name (149959). PR #14's adjacent-IRI loop lives only in Turtle "Shorten
all". Fifteen edit sites write or delete a name, IRI, keyword, number or
directive in existing text: shorten (one and all) in both syntaxes, Turtle
extract, SPARQL expand, prefix and term rename in both syntaxes, the
"Change to" quick fix, and "Remove unused/duplicate prefix" (a safe fix that
`check --fix` applies unasked: `ex:o.@prefix u: <..> .ex:t` becomes
`ex:o.ex:t`). Fourteen have a confirmed breaking input. `batch::fix_all`
also applies edits from different fixes side by side without judging the
combined text. One more: in SPARQL, our lexer reads `<` as an IRI opener
whenever a `>` follows later on the line, while spargebra reads `<=` as an
operator, so a shortened `?a<=ex:b&&?c>?a` is one IRI to us and three tokens
to spargebra; `check` then calls the prefix unused and `check --fix` deletes
it. No character-local rule can model that; only a re-lex can.

**E. `spargebra`'s grammar takes time exponential in the nesting of
`REGEX`, `SUBSTR`, `REPLACE`, `GROUP_CONCAT` and `!`.** Those five rules
have two ordered alternatives that share a prefix containing `Expression()`,
with no memoization, so each level parses its operands twice. `REGEX(`
nested 20 deep takes 8.5 s; 223 deep (107183) or 188 deep through a custom
function call (107745) never returns. A 300-byte `.ttl` with 22 levels inside
`sh:select` hangs `turtle-lsp check` and the server's SHACL thread. The
review read every ordered choice in the grammar: these five are the only
ones exponential in nesting; the others cost a constant factor. The existing
`too_complex` guard bounds depth (256) because it was chosen against stack
overflow; the cost here is time, and depth is the wrong unit. The browser
build has its own `[patch.crates-io]` table and lock file, so a fix in the
root manifest alone leaves the toolbox exponential.

Each class is a one-sentence invariant the code did not enforce:

- A and D: an edit that inserts, replaces or deletes whole tokens leaves the
  token stream equal to the old one with exactly those tokens changed,
  judged by the syntax's own lexer on the text after every edit of the set.
- B: every parse the editor makes of one document uses the same base.
- C: comments attach as a parse of the output will attach them; a token the
  printer omits takes no part in attachment.
- E: checking a document takes time linear in its tokens.

## Solution

Enforce each invariant once, in the one place every caller goes through,
and make the chaos harness check the invariants directly, with a
per-iteration clock, so the next violation is reported as "token split at
byte N in iteration i" rather than as a graph diff or a job timeout.

1. One `token_edits` helper that every token-writing edit goes through,
   including insertions (an empty old span) and deletions (an empty new
   text). It re-lexes the edited window with the lexer the caller passes
   (Turtle or SPARQL) and compares token sequences; where the junction
   would fuse or split, it adds a space or a line break. Declaration
   insertion becomes a token-based point plus this splice check.
2. The SPARQL lexer's `<` rule agrees with spargebra after an operand, so
   the two never disagree on a token boundary.
3. The formatter builds its token document with the semicolons it will not
   print removed from comment attachment, and never prints a redundant `;`.
4. `spargebra` vendored unchanged (root and browser manifests, exact pin,
   golden of parsed queries), then left-factored so no failure follows a
   recursive nonterminal with an alternative that parses the same text
   again; a fuel counter inside the vendored grammar as the bound in the
   unit of the real cost (rule visits per input byte), measured.
5. The chaos harness: reparse under the document URI, print reparse errors,
   no exemptions, a watchdog with a per-iteration budget that saves the
   input and names the iteration, a before/after diff, token-sequence
   checks, a splice check for every inserter and remover, `fix_all` (safe
   and unsafe) on every input, external prefixes in some iterations, graph
   checks for SPARQL actions and renames, a whitespace-squeeze mutation,
   snippets that put data, strings and `; # c` where the bugs live,
   `CHAOS_ONLY=<i>` for replays, and the two old ignores fixed and removed
   (JSON-LD multi-byte spans, TriG one-line graph block).

Acceptance for the whole plan, pass/fail:

- 5 consecutive runs (the owner set 5 for now, from 20, on 2026-10-24) with `CHAOS_ITERS=200000` and fresh seeds on macOS,
  each format's shards as their own command under `timeout 120`
  (`-- mutated_turtle`, `-- mutated_sparql`, `-- mutated_jsonld`, same
  seed), no failure and no iteration over the watchdog budget. Then the
  nightly in CI, twice.
- Every saved failing input of the twenty-one runs (listed in the ticket
  README) replays green with its seed and `CHAOS_ONLY`.
- `bench/perf.mjs`: the median of three runs on every keystroke-latency row
  and `big-file-peak` within 5% of the same median on main.
- `tests/w3c_sparql.rs` stays at its current pass count (425/426); the
  golden of parsed W3C and corpus queries is byte-identical before and after
  left-factoring; the W3C Turtle, TriG and SHACL suites stay green.
- No `#[ignore]`, `todo` or exemption anywhere in tests/chaos.rs; none
  added elsewhere.

## User Stories

1. As a user who runs Extract prefix on a file with data on the declaration line, I want the new `@prefix` placed before the data, so that the file still parses.
2. As a user whose file uses `\r` line endings and who edits through the CLI, the browser editor, or an editor applying offset-based edits, I want quick fixes and Extract prefix to insert at a real line break, so that the declaration is not appended after the data or inside a string. (Line and column positions in `\r`-only files are a separate defect, listed under Out of Scope.)
3. As a SPARQL user whose `PREFIX` line is followed by `SELECT` on the same line, I want "Add prefix" to insert in the prologue, so that the query stays valid.
4. As a user who accepts a completion with an auto-import, I want the import to land at a statement boundary with the file's own line endings, so that accepting a completion never breaks the file.
5. As a user who runs Shorten all on compact Turtle (`ex:a.b.<iri>`, `<s><p><o>`, `1.<iri>`), I want every shortened name separated from its neighbours, so that the graph is unchanged.
6. As a SPARQL user who runs Shorten, Expand or Shorten all inside a FILTER (`?a<ex:b`, `ex:p?<o>`, `?a<=<iri>&&?c>?a`), I want the edit to keep the tokens apart and the checker to read the result the way spargebra does, so that the query is not rewritten into `<<`, a variable or an IRI.
7. As a user who renames the empty prefix (`@prefix:<..>`) or renames a prefix to one ending in a digit, I want the declaration and every use to stay separate tokens.
8. As a user who renames a term next to `-1` or `.5`, I want the rename not to absorb the number.
9. As a user who runs `check --fix`, I want "Remove unused prefix" and every other fix to leave a file that parses, even when the directive sits between two names with no whitespace, and even when two fixes touch.
10. As a user whose files (from TopBraid) end statements with `;` then a comment then `.`, I want Format Document to reach its final form in one pass, so that a second format changes nothing.
11. As a user who formats a file with a relative `BASE` in an untitled buffer, I want the format to succeed and stay stable, so that unsaved files behave like saved ones.
12. As a user who opens a query with `REGEX` nested a few dozen deep, or a shape whose `sh:select` has one, I want the check to finish in milliseconds in the server, the CLI and the browser, so that nothing hangs.
13. As a user whose SHACL shapes graph contains a pathological query, I want start-up indexing to finish, so that a single bad shape does not stall the server.
14. As a user with JSON-LD files in any script, I want `check` never to abort on a malformed file with multi-byte characters.
15. As a user with a large TriG graph block on one line, I want it parsed in linear time.
16. As a maintainer reading a chaos failure, I want the report to show the iteration, the before/after diff with `\r` visible, the edits, and the first differing token, so that the cause is clear without a probe script.
17. As a maintainer whose nightly hangs, I want the harness to name the slow iteration and save its input, and `CHAOS_ONLY=<i>` to replay it alone, so that a hang is a replayable failure and not a job timeout.
18. As a maintainer, I want the chaos harness to hit each of these classes within a few thousand iterations, so that a regression fails a push run (30,000 iterations), not only a nightly.
19. As a maintainer upgrading `spargebra`, I want a golden of parsed queries, a linearity test and a fuel bound, so that neither a changed algebra nor an exponential rule can come back silently.

## Implementation Decisions

- **`token_edits` (A and D).** `pub(crate) fn token_edits(text: &str,
  reps: Vec<(Span, String)>, lex: impl Fn(&str) -> Vec<Token>) ->
  Vec<(Span, String)>` in `syntax::names` replaces `spaced` and PR #14's
  loop. Callers pass whole tokens; an insertion is an empty span, a
  deletion an empty text. Sort by start; build the final text; for each
  junction (the two ends of every replacement), re-lex a window from the
  previous kept token to the next with the caller's lexer and compare
  against the expected token sequence (old tokens with the swapped tokens
  substituted, by kind and text). Where they differ, insert a space (or the
  document's line break when the replacement is a directive) at that
  junction and re-check. The lexer is a parameter because `syntax` must not
  import `sparql`; `features` passes `syntax::lexer::tokenize` and
  `sparql::features` passes `sparql::lexer::tokenize`. A trim step strips
  the common prefix and suffix of old and new by `char`, never by byte.
  Every site goes through it: Turtle Shorten, Shorten all, Extract, Expand;
  SPARQL Shorten, Shorten all, Expand; prefix and term rename in both
  syntaxes (whole-token spans); variable rename; "Change to";
  `quickfix::removal`; declaration insertion.
- **Fix composition.** `batch::fix_all_in` applies, in one round, only
  edits that do not touch; an edit that touches one already taken in the
  round is deferred to the next round, where it is recomputed against the
  new text. Every fix is thereby judged against the text it was computed
  on. The web engine's `fixAll` goes through the same function.
- **Declaration point (A).** `declaration_point` works on `a.tokens()`.
  The anchor `B` is the end of the last `PrefixDecl` before `before` whose
  span is outside every `parsed.graphs` block (no new field on the public
  `PrefixDecl`); with none, the end of the leading comment run, where the
  run ends at the first gap holding a blank line (so a licence header stays
  separate from a doc comment above the first statement, as today); else 0.
  The gap runs from `B` to the start of the next non-comment token `N`. If
  the gap holds a line break (`\r\n`, `\r`, `\n`), the point is the start of
  the line after the first one, when that is at or before `before`;
  otherwise the point is `B` with a leading line break. The text then goes
  through `token_edits` as an insertion, so the `.1` case gets its break.
  The helper `insertion_point(text, anchor_end, next_start, before) ->
  (usize, bool)` is shared with SPARQL `add_prefix`, whose no-prologue arm
  returns an offset above the first non-comment token below the leading
  comment run (the same rule as Turtle; today SPARQL inserts above the
  comments, which changes). SPARQL completion's auto-import goes through
  `in_document_style` like the code actions, so it writes `\r` in `\r`
  files. Visible change: data sharing the declaration's line no longer
  pushes the new line below that data.
- **SPARQL lexer `<` (D).** After an operand (a variable, literal, number,
  `)` or a name) `<` is the operator, as in spargebra; `<` opens an IRI
  only where an IRI can start. The lexer tests pin both readings, and a
  test parses the lexer's token boundaries against spargebra's on the W3C
  query corpus.
- **Formatter (C).** `Doc` gains a constructor (or a second attachment
  pass) that treats a given set of token indexes as absent: they get no
  lead and no trail and do not advance the previous-end cursor for token
  gaps, but `own_line` for a comment after an omitted token is decided from
  the gap between that token and the comment, and `blank` is counted from
  whitespace only, so no blank line is invented and `<o>\n ; # c\n .` still
  prints `<o> . # c`. `features::format` collects the redundant semicolons
  after building the CST (`semis[1..]` of every `Po`, all `semis` of the
  last `Po` of every list: top level, `[ ]`, `{| |}`, graph blocks) and
  rebuilds the doc with them omitted. `Semicolons::Needed` writes only the
  first `;` between two pairs; `extra_semi` is deleted.
  `Semicolons::AsWritten` (SPARQL) is unchanged. Golden outputs containing
  `; #` are updated. CHANGELOG states that `format --check` now fails on
  files with `; # c` before `]`, `}` or `.` that were stable before.
- **SPARQL grammar (E, root cause).** Two steps. First, vendor `spargebra`
  0.4.7 unchanged as `vendor/spargebra` with its licence, a
  `[patch.crates-io]` entry in both Cargo.toml and web/engine/Cargo.toml,
  both lock files updated, the root requirement changed from `"0.4"` to
  `"=0.4.7"` (the patch applies only while the registry version satisfies
  the requirement; the `"0.4"` in the tree today is not a pin), `PATCH.md`,
  tools/check-vendor.sh turned into a loop over a table of crates, a
  Release step in AGENTS.md beside spareval's, and a golden file of
  `Query::to_string()` over every W3C and corpus query. Second, left-factor:
  `REGEX ( a , b ("," c)? )`, same for `SUBSTR`; `REPLACE ( a , b , c (","
  d)? )`; `GROUP_CONCAT ( DISTINCT? e (";" SEPARATOR "=" s)? )` and
  `DISTINCT?` in the other aggregates; `UnaryExpression = "!"
  UnaryExpression / ("+"/"-")? PrimaryExpression` with the second
  alternative no longer accepting `!`, keeping the behaviour without the
  `sparql-12` feature. The review rule in `PATCH.md`: no failure after a
  recursive nonterminal may be followed by an alternative that parses the
  same text again (this covers a prefix shared across rules and a `{? }`
  action that fails after a recursive parse, not only two alternatives of
  one rule). The golden must be byte-identical after factoring. The
  error-message readers (`syntax_message`, `describe`) keep their tests
  green, with any changed expected string listed. `MAX_NESTING` and the
  chain limits are re-measured against stack overflow, since the factored
  rules generate different frames. The same patch goes upstream.
- **Parse fuel (E, backstop).** In the vendored grammar, `ParserState`
  carries a fuel counter that `Expression`, `GroupGraphPattern` and
  `PrimaryExpression` decrement through a `{? }` predicate; parsing fails
  with a "query too complex" error when fuel reaches zero. Fuel is `k x
  input bytes` with `k` measured as the largest visits-per-byte over the
  W3C and corpus queries on the factored grammar, times a stated margin
  (recorded in the constant's comment). It counts the real unit (rule
  visits), applies to every construct, and fires only when the grammar
  regresses. The static depth guard `too_complex` stays as it is (it
  bounds stack). No single-parse change in `check_tokens` and no removal of
  `algebra()` in `sparql::format`: the token-spaced reparse is what places
  errors on token boundaries, and `algebra()` is the one check that
  catches our lexer and spargebra disagreeing.
- **Harness (B and reporting).** The second format builds
  `Analysis::with_syntax(doc_uri(syntax), &out, a.external.clone(), ...)`,
  the editor's second Format Document with the same external prefixes.
  When it is refused, the Error diagnostics of that reparse are printed.
  The `known` exemption and the pending test are deleted (with the
  formatter fix). Failure reports print edits and texts with `{:?}` and a
  before/after diff. A watchdog thread per shard reads `(iteration, start,
  text)` from a shared slot every second; past the budget it writes the
  input through `failure_report` with the replay line, writes the report
  with `std::io::stderr().lock().write_all` (libtest's capture swallows
  `eprintln!` from spawned threads) and exits the process with 101. The
  budget is fixed from measurement: the close-out ticket records the five
  slowest iterations of each of the 20 runs, and the budget is the largest
  of those times a stated margin (at least 10x), with
  `CHAOS_ITERATION_BUDGET_MS` to override; until that figure exists the
  budget is 10 s. `CHAOS_ONLY=<i>` runs one iteration.
- **Harness (wider checks).** A splice assertion for every edit set that
  inserts or removes a declaration (undeclared-prefix quick fixes, "Add all
  missing prefixes", the JSON-LD undefined-prefix fix, completion
  `additional_text_edits`, SPARQL `add_prefix`, "Remove unused/duplicate
  prefix"): the token sequence after equals the sequence before with the
  directive's tokens inserted or removed at one index, whose predecessor
  ends a document-level statement or directive. A token-sequence check
  (kind and text, with the swapped tokens substituted) beside the graph
  check for 1:1 rewrites, reporting the first differing token and offset.
  SPARQL code actions and both renames are graph-checked: a rename's
  expected graph is the old graph with the old IRI replaced. Rename targets
  come from a boundary-stressing set (`""`, `e1`, `e5`, `x1`, a local
  ending in a digit). `fix_all` (safe, then unsafe) runs on every input and
  must leave no new syntax error. One iteration in eight uses a non-default
  `PrefixMap` so the external branches of Shorten, Scope and the quick
  fixes run. A `squeeze` mutation deletes inter-token whitespace when the
  tokens stay the same. New snippets: data on a declaration's line, a `"""`
  string with `\n` on a declaration's line, `; # c` before a `.`, nested
  `REGEX(` and `!(` in the SPARQL set. `survive` cases for the nested forms.
  The JSON-LD shard runs the full corpus, not ASCII only.
- **Old ignores (E and spans).** The JSON-LD multi-byte span defects
  (`jsonld_spans_stay_on_character_boundaries`: a dropped key without quotes
  or a value without its closing quote ends a span inside a character, and
  `check` aborts) are fixed at the span producers and in
  `semantic::encode_with`, and the ignore is removed. The quadratic TriG
  one-line graph block (`trig_graph_block_on_one_line_parses_in_linear_time`)
  is fixed in the Turtle parser and its ignore removed. These are the two
  remaining exemptions in the harness; "no exemptions" is only true once
  they are gone.
- **Ordering.** The harness widening lands after the fixes it would
  expose, or the push run (30,000 iterations) goes red in between.

## Testing Decisions

- A: `declared()` cases in src/syntax/names.rs for each shape (mid-statement
  break, `"""` string on the line, `\r` file with and without declaration,
  leading comment run with and without a blank line, TriG in-block
  declaration not an anchor, anchor followed by `1`); Extract prefix on
  minimised 73609 and 28769 in src/features/refactor.rs tests; the
  undeclared-prefix quick fix for the mid-statement and in-string cases in
  src/features/quickfix.rs tests; `added()` cases q1 to q3 in
  src/sparql/features/edits.rs tests; a SPARQL completion test on a `\r`
  file. Prior art: `a_declaration_goes_before_the_first_use_it_serves`.
- D: a table test over `token_edits` in names.rs with both lexers that
  compares token sequences; graph-keeping cases in refactor.rs with the
  149959 line reduced; SPARQL actions tests for `<s><p><o>`, `ex:p?<o>`,
  `?a<ex:b`, `?a<=<http://e/b>&&?c>?a`; navigation.rs and sparql rename.rs
  cases for `@prefix:<..>`, `(1ex:b)` to `e5`, `( ex:-1 )`, and a prefix
  declared after its use; a quick-fix case for `?a<ex:typo`; a removal case
  for `ex:o.@prefix u: <..> .ex:t`; a `fix_all` case with two touching
  fixes in src/batch.rs tests; SPARQL lexer tests for `<` after an operand
  and a corpus comparison against spargebra's boundaries.
- C: `a_redundant_semicolon_with_a_comment_is_dropped_in_one_pass` in
  tests/format.rs (ten shapes, including two per file, `<o>\n;\n# c\n.`
  and `<o>\n ; # c\n .`); `an_omitted_token_hands_its_comments_to_its_neighbours`
  in src/layout/doc.rs tests; golden updates in test/format.test.mjs.
- B: `a_relative_base_formats_to_a_fixed_point_under_the_document_uri` in
  tests/format.rs under `file:` and `untitled:`; `a_relative_base_needs_a_document_base`
  in src/syntax/parser.rs tests.
- E: a golden file of parsed queries checked by a test before and after
  factoring; `timing_nested_builtins_parse_in_linear_time` in
  src/sparql/tests.rs (each construct at n = 200, open and closed, under
  4 x the n = 50 time plus 5 ms, on a 2 MiB stack);
  `timing_every_builtin_nests_in_linear_time` 64 deep; the 107183 and
  107745 inputs generated in the test; a fuel test that an artificially
  low fuel refuses a long query with the "too complex" message and the
  measured fuel accepts every corpus query; an e2e `.ttl` with 200 levels
  in `sh:select` in test/shacl-sparql-lint.test.mjs, `query -e` in
  test/cli.test.mjs, and a web engine test that the browser build checks
  the same query in bounded time; `survive` cases in tests/chaos.rs.
- Replays: every saved input in the ticket README, with its seed and
  `CHAOS_ONLY`, as the last step of each ticket that fixes its class.
- Whole plan: the 5-run acceptance above, run by the close-out ticket on
  a quiet machine, results recorded in the ticket README.

## Out of Scope

- `LineIndex` and `LineStarts` (src/analysis.rs) end lines at `\n` only,
  so line and column positions in a `\r`-only file disagree with the LSP
  specification. Real, separate, not a chaos failure (the harness applies
  edits by offset and cannot see it): a follow-up ticket.
- Prefix files parsed with no base (src/workspace.rs, src/knowledge.rs), so
  a relative `@prefix` IRI in `prefixes.ttl` stays relative. Same family as
  B, not a chaos failure: a follow-up ticket.
- Extract prefix still refuses IRIs above the last declaration
  (src/features/refactor.rs gate); a top-of-file point would serve them.
- Lowering `MAX_NESTING`, or any depth limit as a fix for E.
- Memoization (`#[cache]`) in the vendored grammar: left-factoring removes
  the need, and caching would change memory use.
- Custom aggregate registration (the one other exponential shape in the
  grammar, `iriOrFunction` then `Aggregate`); turtle-lsp registers none.

## Further Notes

- Why vendor rather than guard only: a guard turns a hang into a refusal
  of a legal query. Twenty nested `REGEX` is a legal query that a user can
  write, and the server should check it in milliseconds, which only the
  grammar fix gives. The fuel bound is kept because a future `spargebra`
  can regain the shape, and the linearity test catches only the constructs
  it nests. A static table of constructs (the first draft of this plan) was
  rejected in review: it fires on the constructs the linearity test already
  covers and on nothing else.
- Why the harness is widened in a later wave: the splice, token-sequence,
  `fix_all` and SPARQL checks would fail today on classes A and D; landing
  them first makes every push red until the fixes land.
- The mutation changes (squeeze, new snippets) raise the hit rate of each
  class from about one in 100,000 iterations to a few thousand, which is
  what makes the 30,000-iteration push run a guard and not a lottery.
