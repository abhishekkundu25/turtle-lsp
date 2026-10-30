## Problem Statement

The graph view's Hierarchy mode lists classes as a tree of names. It answers "where does this class sit" and nothing else. Someone modelling or maintaining an ontology needs to know what each class *has*: its properties, which of them the data really uses, which the shapes require and how many instances break that, and where the ontology, the shapes and the data disagree. Today that takes reading several files and running SHACL by hand, and no RDF tool we found draws it.

The graph view also ignores the editor: moving the cursor to a term does not show that term in the graph. The research (docs/research/graph-view-features.md, local) ranked "follow the cursor" as the most asked-for graph feature; every tool found follows the open file, none the term.

## Solution

Hierarchy mode becomes **Schema** mode. It keeps the subClassOf tree, and each class is a box with a row per property:

- declared rows from `rdfs:domain`, `schema:domainIncludes` and SHACL property shapes (`sh:targetClass` + `sh:property` / `sh:path`);
- observed usage on each row: how many instances of the class use the property;
- a state per row: ok, never used, not declared, bad values (literals outside the declared datatype), or conflict (the `rdfs:range` and the shape's `sh:datatype`/`sh:class` disagree);
- SHACL required (`sh:minCount` ≥ 1) with the count of instances missing it;
- link rows (`→ Pipe`) that, on hover, draw an arrow to the target class's box;
- a problem count on each box, a "Problems only" filter, and imported classes collapsed into one box.

Selecting a class fills the graph view's info panel with the class inspector: the rows as declared vs. used, each row's sources (file:line, clickable), and the class's problems, each with a button that opens the place to fix it.

A **Follow cursor** toggle, shared by every mode, re-centres the view on the term under the editor's cursor: in Schema on the class (and its row for a property), in Graph and Focus on the node. It is off by default.

All the work runs off the server's main loop over a copy of the index, is capped, reuses per-document results that did not change, and is measured.

## User Stories

1. As an ontology author, I want each class drawn with its properties, so that I see what a class has without reading three files.
2. As an ontology author, I want the subClassOf tree kept in Schema mode, so that I still see where a class sits.
3. As an ontology author, I want rows declared by `rdfs:domain` to appear on the domain class, so that the drawing matches my ontology.
4. As a schema.org user, I want `schema:domainIncludes` to count as a declaration, so that schema.org-style models are drawn too.
5. As a SHACL author, I want each `sh:property` of a shape targeting a class to appear as a row of that class, so that shapes and ontology show in one place.
6. As a SHACL author, I want `sh:minCount ≥ 1` shown as "required" and `sh:maxCount` as the upper bound, so that I see the cardinality at a glance.
7. As a data engineer, I want each row to say how many instances use the property ("340/352"), so that I see how the data really looks.
8. As a data engineer, I want instances of a subclass to count for the superclass's rows, so that inherited properties are counted the way SHACL targets them.
9. As an ontology author, I want declared rows no instance uses marked "never used", so that I find dead schema.
10. As an ontology author, I want properties that instances use but nothing declares for their class shown as "not declared", so that I find undocumented data.
11. As an ontology author, I want "not declared" rows only for properties outside the standard vocabularies (rdf, rdfs, owl, skos, …), so that `rdfs:label` on every instance is not noise.
12. As a data engineer, I want rows whose literals are not of the declared datatype marked with the count ("12 bad values"), so that I find bad data before validation fails.
13. As a SHACL author, I want a row whose `rdfs:range` and shape datatype or class disagree marked as a conflict with both values and their sources, so that I fix the model, not the data.
14. As a SHACL author, I want the number of instances missing a required property on the row, so that I see validation trouble without running validation.
15. As a modeller, I want link rows (object properties) to show their target class and, on hover, an arrow to its box, so that I see relations without a hairball.
16. As a modeller, I want clicking a link row's target to select and centre that class, so that I can walk the model.
17. As a modeller, I want each box to show its number of problems, so that I see where the trouble is from far away.
18. As a modeller, I want a "Problems only" filter that fades classes without problems and hides rows that are fine, so that I can work through the problems.
19. As a modeller, I want classes from `owl:imports` collapsed into one box with a count, and a toggle to show them, so that an imported upper ontology does not drown my own classes.
20. As a modeller, I want terms used but never defined shown as ghost boxes, so that I find typos and missing definitions.
21. As a user, I want selecting a class to show the inspector with its rows, declared vs. used, so that I can read the details without hovering each row.
22. As a user, I want each row's sources (file:line) listed and clickable, so that I go straight to the declaration.
23. As a user, I want the inspector to list the class's problems, each with a button that opens the place to fix it, so that a problem is one click from its fix.
24. As a user, I want the inspector for an individual or a literal to stay as it is today, so that the other modes do not change.
25. As a user with the graph in a narrow side split, I want Schema to show the inspector alone below a width, so that a narrow split stays readable.
26. As a user with a big ontology, I want Schema to open on a focus class with its parents, children and linked classes when there are more classes than the cap, so that the first view is readable.
27. As a user with a big ontology, I want boxes zoomed far out to collapse to their name and problem count, so that the overview stays legible.
28. As a user, I want rows beyond a per-class cap summarised as "+N more", so that a class with hundreds of properties does not fill the screen.
29. As an editor user, I want a "Follow cursor" toggle in the graph view, so that the view re-centres on the term at my cursor.
30. As an editor user, I want Follow cursor to wait until the cursor rests, so that the view does not jump on every keystroke.
31. As an editor user, I want the cursor on whitespace, a literal, a comment or punctuation to leave the view where it is, so that it does not flicker.
32. As an editor user, I want the cursor on a class to focus its box in Schema, and on a property to focus the class that declares it with the row lit, so that the cursor picks the right place.
33. As an editor user, I want the cursor on a node to centre it in Graph mode and walk to it in Focus mode, so that Follow cursor works in every mode.
34. As an editor user, I want Follow cursor to work with Vim keys, so that any cursor movement counts.
35. As an editor user, I want Follow cursor to remember its setting with the view's other settings, so that it stays as I left it.
36. As an editor user, I want a hidden or closed graph view to send nothing when the cursor moves, so that it costs nothing when I am not looking.
37. As a user of the browser editor, I want Schema and Follow cursor there too, so that it works without VS Code.
38. As a user, I want Schema to update after I save or edit a file, so that it is never stale.
39. As a user of a big workspace, I want typing to stay as fast as it is today while Schema is open, so that the view never slows editing.
40. As a user, I want Schema to answer on a 100,000-triple workspace in well under a tenth of a second, so that it feels instant.
41. As a user, I want the colours to mean one thing each: red and amber for errors and warnings as in the Problems panel, nothing else in those colours, so that the view is not confusing.
42. As a user, I want a legend behind a "?" button, so that it is there when I need it and out of the way when I don't.
43. As a keyboard user, I want boxes and rows reachable with the keyboard and the inspector buttons focusable, so that Schema is accessible.
44. As a screen-reader user, I want rows to say their state in words, not only by colour or glyph, so that the state is not lost.
45. As a maintainer, I want the schema request to be a separate experimental request (`turtle/schema`), so that the graph request and its tests do not change shape.
46. As a maintainer, I want every count in the answer to be checked against a slow, obvious recount in the tests, so that the numbers can be trusted.
47. As a maintainer, I want the schema computation run on random and real-world workspaces in the tests, so that it never panics or hangs on odd input.
48. As a maintainer, I want the answer for a workspace to be the same whether per-document results were reused or computed fresh, so that the cache cannot drift.

## Implementation Decisions

**One new request, `turtle/schema`**, listed under `experimental` next to `turtle/graph`. It is answered like `turtle/graph`: on a thread of its own over a copy of the index (`Spawn::Thread`), inline in the browser engine (`Spawn::Inline`).

**Parameters** (all optional; checked once at the boundary, as `GraphParams` is):

```text
files?: string[]          documents; omitted: every workspace document
focus?: string            a class or property IRI
at?: { uri, position }    the term at a document position (Follow cursor); wins over focus
maxClasses?: number       default 300, at most 2000
maxRows?: number          rows per class, default 64, at most 512
imports?: boolean         default false: imported classes are counted, not listed
examples?: string         a class IRI: the class whose rows list `examples` (below)
```

**Answer**:

```text
classes: [{
  id, name, label?, uri?, range?,        // where the class is defined, when it is
  kind: "class" | "imported" | "undefined",
  parents: number[],                     // indexes into classes (subClassOf)
  instances: number,                     // direct and through subclasses
  problems: number,                      // rows whose state is not ok, plus rows with missing > 0
  rows: [{
    property, name,
    link: boolean, target?: number,      // link rows: index of the range class when listed
    range?: string, rangeName?: string,
    declared: [{ how: "domain" | "domainIncludes" | "shape", uri, range }],
    used: number, of: number,            // instances using it / instances of the class
    state: "ok" | "unused" | "undeclared" | "badValues" | "conflict",
    badValues?: number,
    required?: { min: number, max?: number }, missing?: number,
    conflict?: { declared: string, shape: string },
    examples?: [{                        // one class's rows only (below); omitted when empty
      kind: "missing" | "badValue" | "undeclared",
      uri, range,
      name: string,                      // the instance: prefixed name, <iri> or _:b<n>
    }],
  }],
  moreRows: number,
}]
focus?: number, focusRow?: number        // from focus or at
total: { classes: number }               // before the cap
imported: number                         // imported classes not listed (imports: false)
```

Problems are derived on the client from the rows; the answer does not carry a second list.

**Examples.** `examples` says where a row's problems are, so the inspector can open them. Only the rows of one class list them: the class the `examples` parameter names, when it is listed; else the focus class (from `focus` or `at`); else none. Every other row has none, and its counts are the same. The server collects places for those classes only (at most two: the focus and the `examples` class), so an answer without them costs neither the time nor the size: on 300 listed classes, examples on every row made a 4.8 MB answer. The view asks again with `examples` when the user selects a class whose rows have problems and no examples, with every other parameter unchanged, so the classes listed and their order stay; it merges the rows' examples into the class it shows and lays nothing out again. Each kind lists its first 10 places, in the request's document order (workspace documents by URI, then imports) and then text order; a row lists `missing` examples first, then its state's:

- `missing` (when `missing` > 0): the instances with fewer than `sh:minCount` values, at the place their first document first names them (the statement's subject, or for a blank node or a nested description the first triple's predicate and object). An instance described in several documents is located in the first of them.
- `badValue` (state `badValues`): each literal outside the declared datatype, at the literal.
- `undeclared` (state `undeclared`): each instance using the property, at its first use (the triple's predicate and object).

`conflict` and `unused` rows have none; their place is the declaration (`declared`). So on the class that lists them, the count of a kind's examples is the smaller of 10 and `missing`, `badValues` or `used`; the recount test checks the count, the order and every range.

**Per-document facts, merged per request.** For each document, one pass over its triples yields its facts: class declarations, subClassOf edges, domain/domainIncludes/range declarations, shape property declarations, `rdf:type` assertions and property uses (subject, property, value kind: literal datatype or IRI). The facts are keyed by the document's `Arc<Analysis>`; a request reuses the facts of every document whose analysis did not change and computes the others in parallel (rayon, as `features::graph` does from 2000 nodes). The merge joins subjects across documents (an instance typed in one file and described in another counts). The facts are deterministic, so a reused result equals a fresh one; a test asserts it.

**SHACL shapes** are read with `shacl::model` (`Shapes`), not a second parser: targets via `sh:targetClass` (and implicit class targets), properties via `sh:property` with a predicate `sh:path`. Paths that are not a single predicate (inverse, sequence, alternative) are listed by their written form with no usage count.

**Counts.** Instances of a class are subjects typed with it or with any of its subclasses (transitive, cycle-safe). `badValues` counts literal values whose datatype is not the declared XSD datatype (string-like and numeric types compared exactly, `rdf:langString` against `xsd:string` as not matching), and only when exactly one datatype is declared. `missing` counts instances of the target class with fewer than `sh:minCount` values.

**Caps and focus.** Over `maxClasses`, the answer keeps the focus class (from `focus` or `at`, else the class with most instances), its ancestors, descendants and link targets, breadth-first, then the rest by instance count, ties by IRI. Rows over `maxRows` are cut, worst state first, and counted in `moreRows`.

**Follow cursor, `at`.** The server resolves `at` on the main loop (the open document's analysis, the term at the position, as hover does) to an IRI before the job starts; a position on no term is answered with `null` (nothing to follow; the view stays as it is). `turtle/graph` takes the same `at` and resolves it to its `focus`. Hosts send `at` only when the toggle is on and the view is visible, after the cursor rests for 150 ms; the view keeps one request in flight and drops answers older than the latest cursor.

**Client.** `web/app/graph/schema.js` draws the boxes (DOM, one element per box, rows as children) in a tidy top-down tree layout; tree.js and the Hierarchy tab go. Zoomed out under 0.5, boxes render as their header only. The link arrow is one SVG path drawn on hover. The info panel (`panel.js`) gets the class inspector. Under 700 px of width (container query), Schema shows the inspector list alone. The legend sits behind a "?" button. Diff colours are not used in this PR; red and amber are only for problems.

**Hosts.** VS Code (`graph.ts`) forwards `turtle/schema` like `turtle/graph` and posts the active editor's cursor (`onDidChangeTextEditorSelection`, active end) to the webview when the view is visible. The browser editor posts Monaco's cursor (`onDidChangeCursorPosition`).

**Fix buttons** open the source location of the problem beside the view: the first example when the row has one (missing, bad values, not declared), else the row's first declaration (conflict, never used). The inspector lists a problem's examples behind "Show N", each opening its place, with "+N more" past the ten listed. While the view asks for a class's examples, its problems show their counts and "Loading places…", and "Open first" waits for the places. New code actions for schema problems are a later PR.

## Testing Decisions

A good test here checks what a user or client sees: the counts and states in the answer, the boxes and inspector in the page, the request a cursor move sends. It does not check private helpers or the shape of the cache.

- **Unit tests in `features::schema`** (prior art: the tests in `src/features/graph.rs`): each row state from a minimal workspace; domainIncludes; subclass counting and cycles; cross-file instances; bad values per datatype pair; conflicts; required/missing; caps and focus order; `at` on a class, a property, whitespace.
- **Oracle against a recount**: a seeded generator builds random workspaces (classes, subclass chains with cycles, domains, ranges, shapes, instances, literals of mixed datatypes, blank nodes, undefined terms, broken syntax) and checks every count against a naive recount written plainly in the test. Thousands of seeds per run, under the two-minute cap.
- **Reuse equals fresh**: for random edit sequences, the answer with reused per-document facts equals the answer computed from scratch.
- **Never breaks**: the chaos tests (tests/chaos.rs) run the schema computation on every iteration's document, with a watchdog budget like the other features, and require no panic.
- **Real data**: a Node test (prior art: test/graph.test.mjs) runs `turtle/schema` over every corpus workspace in `test/corpus/` (skipped without it, failing with `CI=true` as the corpus tests do) and over `test/fixtures/real/` when present, and checks the answer's invariants (indexes in range, used ≤ of, problems = the rows' count, deterministic across two requests).
- **Protocol**: test/schema.test.mjs: the capability is listed; params are validated (InvalidParams on bad values, the session goes on); `at` resolves; the request does not block a hover sent while it runs.
- **Browser**: web/test/graph-ui.test.mjs and editor-graph.test.mjs (prior art): Schema draws boxes and rows from a fixture answer; Problems only; hover arrow; inspector; narrow layout; Follow cursor sends `at` after the cursor rests, drops stale answers, and sends nothing while hidden; a 300-class answer draws within a frame budget.
- **VS Code**: editors/vscode/test/suite.cjs: the webview gets `turtle/schema` answers and cursor positions when Follow cursor is on.

## The core server stays untouched (pass/fail)

- No change under `src/syntax/`, `src/analysis.rs`, `src/knowledge.rs`, `src/workspace*`, `src/diagnose.rs`, `src/features/` other than the new `schema.rs` and its `mod` line, and no change to any existing request's or notification's handling.
- `src/server.rs` and `src/ide/display.rs` gain only additive code: the `turtle/schema` route and capability entry, its thread job, the facts cache it holds, and the resolution of `at` (for `turtle/schema` and `turtle/graph`, both experimental requests).
- Every existing test passes unchanged, apart from the Hierarchy tests that Schema replaces in the graph view.

## Performance budgets (pass/fail)

Measured by a new `schema` scenario in `bench/perf.mjs`, on a quiet machine, 20 runs, median and spread reported, with the limiter named:

- `turtle/schema` on the `editing` workspace (202 files): first request under 50 ms, repeat after editing one file under 10 ms.
- `turtle/schema` on the 100,000-triple file: under 80 ms first, under 20 ms repeat with no change.
- The keystroke-latency rows of the `editing` and `big-file` scenarios do not move (within their run-to-run spread).
- A hover sent while a `turtle/schema` request runs on the 100,000-triple file answers within the hover budget it has today.
- The page draws a 300-class answer in under 50 ms (median of 5).
- Follow cursor sends at most one request per 150 ms rest, and none while the view is hidden.

## Out of Scope

- Diff against git (rows and boxes added, removed or changed since a commit): needs reading files at a commit from the server; a later PR.
- New code actions for schema problems (declare a property, align a range, convert values, insert placeholders): a later PR; the buttons here open the place to fix.
- `owl:Restriction` axioms (`someValuesFrom`, `allValuesFrom`, cardinality restrictions).
- Editing the schema from the boxes (add property, rename from a row).
- Rows inherited from superclasses shown on subclasses (the counts include subclass instances; the rows stay on the declaring class).
- SPARQL-based SHACL targets and `sh:or`/`sh:and` shapes beyond listing their written form.
- Neovim: it has no graph view.

## Principles

- **redesign-from-first-principles**: Schema replaces Hierarchy instead of adding a fourth mode; the info panel becomes the inspector for classes rather than a second panel.
- **foundational-thinking / model-the-domain**: per-document facts are the data structure the merge and the cache are built on; row state is one enum, not flags.
- **type-system-discipline**: `RowState` and the declaration kinds are Rust enums with exhaustive matches; `at` is parsed into a typed position once.
- **boundary-discipline**: `SchemaParams` are validated once in `schema()`; the rest trusts them, as `features::graph` does.
- **explain-the-number**: each budget above is reported with its run count, spread and limiter.
- **experience-first**: one meaning per colour, legend behind "?", inspector-only when narrow, Follow cursor off by default.
- **separate-before-serializing-shared-state**: the per-document facts cache is written only by the job that computed a missing entry, keyed by the analysis it came from; two jobs computing the same entry write equal values.
