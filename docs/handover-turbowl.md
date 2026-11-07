# Handover: reasoning and "why is this true?" through TurbOWL

Branch: `feat/turbowl-reasoning`, from `main` (4488f0f, release 0.13.1).
Written 2026-11-07 from the TurbOWL repository's session, for the session
that builds this. Start with `/spec-to-slices` on this file. Ask the owner
only about the decisions marked **Owner** below; take the rest as given.

## The goal

The editor shows what a model implies, and why. TurbOWL, the owner's
OWL 2 RL reasoner, works out the closure in the background. Hover on any
triple, asserted or inferred, then shows a proof: the base triples it rests
on, one W3C rule per step, each base line a link to its file and line.
TurbOWL's diagnostics (contradictions, ill-typed literals) become editor
diagnostics with their proofs. The editor never waits on it: slow work is a
progress indicator, like SHACL's.

## This replaces the engine in `docs/specs/reasoning.md`

That spec (status: not scheduled) plans a hand-built RDFS and "OWL RL lite"
engine that stores provenance and leaves out `owl:sameAs` (user story 17,
Out of Scope). TurbOWL changes three of its decisions:

- **Engine.** TurbOWL is the engine: every W3C OWL 2 RL rule, verified
  against several oracles, fast (editor-size files reason in milliseconds,
  and edits reuse earlier work).
- **Provenance.** Nothing is stored while reasoning. A proof is computed
  when asked for, typically in 0.01 to 0.6 ms on editor-size files with the
  default rules.
- **`owl:sameAs`.** It is on in TurbOWL's defaults, and explained: a proof
  shows the chain of sameAs links and the triples that caused the merge.
  This is the case RDFox refuses to explain.

Keep the rest of `reasoning.md`: its user stories, the consumers (hover,
completion ranking, type hierarchy, SHACL with inference as a separate
opt-in), the size limit, imports, browser parity and the testing decisions.
Rewrite it, or write a new spec that points to it.

**Owner:** the setting's levels. TurbOWL has no RDFS-only mode. Its rule
groups are switches: `equality`, `property-chains` and `checks` are on by
default; `identity`, `schema`, `trivial` and `literal-values` are off. The
choice is either `off` / `default` / `complete` (TurbOWL's
`Switches::default()` and `Switches::complete()`), or keeping `rdfs` and
`owl-rl-lite` and mapping them onto switches. The first is simpler and is
the recommendation.

## TurbOWL, as it is today

- Repository: `~/repos/turtle-reasoner`, crate `crates/turbowl`. The
  feature is on its `explain` branch (84bd0f7), not yet merged into its
  `main` and not pushed. The GitHub remote is private and empty.
- Dependency: a **path** dependency, `turbowl = { path =
  "../turtle-reasoner/crates/turbowl", default-features = false }`. A git
  dependency needs a push, and publishing needs the owner's approval
  (neither now). TurbOWL reads its rule program from
  `oracles/rules/*.rls` at the repository root with `include_str!`, so the
  whole checkout must be there; a path dependency has it.
- Features: `io` (its own Turtle and N-Triples parsers, N3 and RDF/XML) and
  `parallel` (rayon) are on by default. turtle-lsp already parses, so use
  `default-features = false` and feed triples. Add `parallel` for the
  native build if large workspaces need it. It builds for
  `wasm32-unknown-unknown` with `--no-default-features`, so the browser
  editor can use it too.
- Toolchain and shared dependencies: the same as turtle-lsp's (Rust 1.99.0,
  edition 2024, `rustc-hash` 2.1.3, oxrdf 0.3.4).
- **Check first: oxrdf's `rdf-12` feature.** turtle-lsp turns on
  `rdf-12` (through `oxrdfxml` and the dev-dependencies), and Cargo
  unifies features, so TurbOWL will be compiled against an oxrdf with RDF
  1.2's triple terms. TurbOWL has never been built that way. If it fails
  to compile, or must handle triple terms, the fix belongs in TurbOWL (a
  small change, its own ticket in that repository, with its benchmark
  check). Try `cargo check` with the dependency added before planning
  anything else.

### The interface turtle-lsp needs

Rust, `turbowl::*`; the crate doc in `crates/turbowl/src/lib.rs` has
the details.

```rust
let mut r = Reasoner::with_switches(Switches::default());
r.set_threads(Threads::ONE);                 // or ALL on native, with `parallel`
r.set_base(triples.iter().map(Triple::as_ref)); // the workspace's triples; returns whether it changed
let summary = r.reason();                    // reuses the last run's work
r.inferred();                                // Iterator<Item = oxrdf::Triple>
r.diagnostics();                             // &[Diagnostic { code, severity, message, terms }]

let explainer = r.explainer()?;              // Err(Error::NotReasoned) after a change, until reason()
let proof: Option<Proof> = explainer.explain(triple.as_ref()); // None: not in the closure
explainer.explain_diagnostic(&d);
let cache: ExplainCache = explainer.into_cache(); // owned, Send + Sync: keep it between hovers
let explainer = r.explainer_with(cache)?;    // reuse; drop it when the reasoner changes
```

- `Proof`: `steps()` (topological, the goal last), `leaves()` (the base
  triples), `depth()`, `reason()`, `goal()`, and `Display` as an indented
  tree. A `Step` is a `Statement` (three `oxrdf::Term`s: a generalized
  triple, since a proof may pass through a literal subject) and a
  `Reason`, which is `Base` or `Rule { rule: &'static str, premises:
  Vec<usize> }`. Rule names are the W3C ones (`cax-sco`, `prp-dom`,
  `eq-rep-s`).
- Proofs are written in the terms of the triples given to `set_base`,
  blank nodes included. Feed turtle-lsp's own blank nodes and the names
  match. Do not round-trip through TurbOWL's parser.
- `Reasoner` owns its state. `Explainer` borrows it, so hovers run on a
  thread that holds the reasoner (or a read lock on it), with the cache
  passed in and out.
- A run cannot be cancelled halfway today. Discard a stale result (keep a
  generation counter, as the SHACL scheduling does) rather than wait on
  it.

### Speed

Measured on TurbOWL's `explain` branch (one thread, warm cache, its bench
`turbowl-bench --explain`, load 1.5 to 2.3):

| input | typical hover | slowest 1 in 100 |
|---|---|---|
| Brick plus small1 (26 k closure triples), default rules | 0.009 ms | 0.21 ms |
| Brick plus soda hall, default rules | 0.03 ms | 0.32 ms |
| lubm-1 (184 k), default rules | 0.003 ms | 0.07 ms |
| the same Brick files, complete rules | 2.2 ms | **157 ms**, worst 3.9 s |

The complete rules are slow on Brick, with a cache up to 2.6 GB after 1,000
hovers. That is TurbOWL's ticket 06 (`docs/tickets/explain/` there). Until
it lands, either keep `complete` out of the settings or label it as slow.
Reasoning on editor-size files takes milliseconds; lubm-50 (10 M
triples) takes about 0.4 s on one thread.

## The work, in turtle-lsp

Roughly in order. Slice them per `/spec-to-slices`.

1. **The dependency, and the `rdf-12` check** (above).
2. **Background reasoning**, following the SHACL pattern in
   `src/server.rs` (its `progress` and `shacl` groups):
   - on open, on save and after a pause in typing, hand the workspace's
     triples and loaded imports to `set_base` on a thread of its own, then
     `reason()`;
   - work-done progress ("TurbOWL: reasoning…");
   - a generation counter, so that a stale run's result is dropped;
   - the size limit (`turtle.reasoning.maxTriples` or SHACL's cap), skipped
     with a status note;
   - reasoning off by default: off changes nothing, and the server's
     benchmark stays unchanged.
3. **Hover** on an asserted or inferred triple, or on a resource (its
   inferred types):
   - the proof as markdown, with each leaf as a `file#L<line>` link, as
     "Described in" does;
   - a time limit on the explanation, about 200 ms, on the reasoning
     thread. If it is not done in time, the hover says so and the search
     finishes in the background, so the next hover is instant. A "Show
     why" command that opens the proof when ready is an alternative.
4. **The triple-to-location index**: base triple → file and range, from
   the parser's positions in the knowledge index. This is the largest
   piece.
5. **Diagnostics**: TurbOWL's findings (`code`, `severity`, `message`,
   `terms`) as editor diagnostics on the lines of the terms. The proof
   goes in the hover or in related information.
6. **The existing consumers** of `reasoning.md`: completion ranking by
   inferred types, inferred links in the type hierarchy, and SHACL with
   inference (opt-in). These come after hover works; a separate wave is
   fine.
7. **The browser build**: the same through `web/engine`, with TurbOWL
   built for wasm32 (one thread).

## Tests

- Protocol tests over stdio (the existing Node end-to-end suite):
  - hover shows the proof and its links for an inferred type, for a
    triple that holds through `owl:sameAs`, and for a base triple;
  - a contradiction appears as a diagnostic with its proof;
  - an edit updates the closure after the pause;
  - a stale run's result is not shown;
  - the size limit skips with a note;
  - reasoning off answers exactly as today.
- Browser parity, as the web LSP parity tests do.
- The server's benchmark: unchanged with reasoning off; measured and
  reported with it on.
- Correctness of the reasoning and the proofs is TurbOWL's: its oracles and
  its proof checker cover them. turtle-lsp tests the wiring and what the
  user sees.

## Rules (both repositories)

- Commits carry the owner's name only: no AI attribution and no co-author
  lines.
- Nothing is pushed or published without the owner's approval.
- A change TurbOWL needs is made in `~/repos/turtle-reasoner` as its own
  ticket, under that repository's `AGENTS.md`. Reasoning speed there must
  not change: it has a benchmark gate.
- Stop processes by PID, never `pkill` or `killall`.
- Fixtures use neutral names (`ex:`, `http://example.org/`).
- Subagents: model opus, effort high.
- Read turtle-lsp's own `AGENTS.md` and `ARCHITECTURE.md` before
  planning; they win over this file where they differ.
