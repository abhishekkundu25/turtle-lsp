# Changes on the graph: a semantic diff against git HEAD

Status: done (unreleased). Builds on the workspace graph (`turtle/graph`,
`src/features/graph.rs`, the shared view in `web/app/graph/`). The
prototype is on the branch `prototype/graph-diff` (three variants; this
spec keeps B's change list and A's overlay and drops C, before/after side
by side).

## Problem Statement

A text diff of an RDF file says which lines moved, not what changed in the
data. Reformatting, reordering a subject's properties or renaming a
prefix fills a text diff with noise, while one changed object hides among
it. Nobody reviewing an ontology change can see from the diff which terms
changed, which new terms appeared, which disappeared, and which other
terms depend on them. No RDF tool found shows a git diff as a graph.

turtle-lsp already parses every workspace file and knows every reference,
so it can compare the data of the working copy with the data at git HEAD
and show the difference as triples and terms, on the graph and as a list.

## Solution

A new request, `turtle/graphDiff`, compares the workspace's data with a
base version of the files the editor supplies, and answers with the
triples added and removed per file, value changes as one row, and the
terms added, removed, changed and impacted. The graph view gets a
"Changes" toggle that colours the graph by that answer and shows the change
list beside it. VS Code supplies the HEAD version of every file git reports
as changed; the browser editor supplies the text each file had when the
workspace loaded. Clicking a row or a node opens its source line.

## User Stories

1. As an ontology author, I want to see which triples I added and removed since my last commit, so that I know what my change does to the data.
2. As an ontology author, I want a changed value shown as one row, `ex:bob ex:worksFor ex:alice → ex:acme`, so that I read it as a change and not as two unrelated rows.
3. As a reviewer, I want reformatting, reordering and prefix renames to show no change, so that only data changes reach me.
4. As an ontology author, I want the change list grouped by file, so that I see each file's part of the change.
5. As an ontology author, I want each row to open its source line (the current line for an added triple, the line it had for a removed one), so that I can go from the list to the text.
6. As an ontology author, I want the terms of the change listed as added, removed, changed and impacted, with counts, so that I see its size at a glance.
7. As an ontology author, I want terms that reference a changed or added term marked as impacted, so that I see what else my change can affect.
8. As a user of the graph view, I want a "Changes" toggle that colours the graph: added green, changed amber, impacted with a ring, so that I see the change in context.
9. As a user of the graph view, I want removed terms and triples drawn as faded dashed ghosts beside their old neighbours, so that I see what is gone.
10. As a user of the graph view, I want clicking a coloured node to show its added and removed triples, so that I can inspect one term's change.
11. As a user, I want the changes to update as I edit, so that the view is never stale.
12. As a user, I want a file created since HEAD to count as all added, and a file deleted since HEAD as all removed, so that new and deleted files are not missed.
13. As a user, I want a blank node that moved within a file, or one added before it, not reported as changed, so that blank-node renumbering is not noise.
14. As a VS Code user, I want the base to be git HEAD of the repository the files are in, read through VS Code's git, so that the diff matches what I am about to commit.
15. As a VS Code user outside a git repository, or with git off, I want the toggle to say there is nothing to compare with, so that I know why it is empty.
16. As a browser editor user, I want the base to be the text each file had when the workspace loaded, so that I can see what I changed in this session.
17. As a user of a large workspace, I want the changes computed only while the toggle is on and only for the changed files, so that the graph and typing stay as fast as today.
18. As a user, I want a very large change cut at a limit with a note saying how much was left out, so that the view stays usable.
19. As a user, I want the existing graph modes, filters, follow-cursor and layout to work as before with the toggle off, so that nothing I use changes.
20. As a keyboard and screen reader user, I want the change list to be a list of focusable rows with their status in text, so that the change does not depend on colour.
21. As a Neovim user, I want the request available to plugins even though the plugin has no graph view, so that a later plugin can use it.

## Implementation Decisions

- **Where the base comes from.** The client sends it; the server never runs
  git. Running git in a workspace can run programs its config names
  (`core.fsmonitor`, filters), which an untrusted repository controls, and
  the WebAssembly build cannot run processes at all. VS Code reads the HEAD
  version through the built-in git extension's API
  (`vscode.extensions.getExtension("vscode.git")`, `getAPI(1)`): the
  repositories' working-tree, index and untracked changes give the files
  that differ from HEAD, `repository.show("HEAD", path)` their text. The
  browser editor records each file's text when the workspace loads.
- **The request.** `turtle/graphDiff`, advertised under `experimental`
  like `turtle/graph` (`{"turtle/graphDiff": true}`). Params:

  ```json
  { "base": [ { "uri": "file:///w/data/people.ttl", "text": "…" },
              { "uri": "file:///w/data/new.ttl", "text": null } ],
    "limit": 2000 }
  ```

  Every file that differs from the base is listed, once. `text: null` means
  the file is not in the base (created since: all its triples are added).
  A listed URI with no workspace document now is a deleted file (all its
  base triples are removed). A workspace document not listed is unchanged.
  `limit` (default 2000, at most 20000) caps the rows and the impacted
  terms. The params are parsed once at the boundary into typed values:
  more than 5000 entries, a text over the server's file size limit, a URI
  that is not a workspace path, or a duplicate URI is `InvalidParams`.
- **The answer.**

  ```json
  { "files": [ { "uri": "…", "rows": [
        { "change": "added" | "removed" | "changed",
          "subject": Term, "predicate": Term, "object": Term,
          "was": Term,            // "changed" only: the old object
          "range": Range,         // added, changed: the current triple
          "baseRange": Range } ] } ],   // removed, changed: the base triple
    "terms": [ { "id": "…", "name": "…", "status": "added" | "removed" | "changed" | "impacted",
                 "uri": "…", "range": Range } ],
    "total": { "triplesAdded": 0, "triplesRemoved": 0, "added": 0, "removed": 0,
               "changed": 0, "impacted": 0, "cut": false } }
  ```

  `Term` is `{ "id", "name", "literal" }`, with `id` and `name` in the
  graph's scheme (`GraphNode::id`, `GraphNode::name`) on the current side,
  so the view matches rows and terms to the nodes and edges of a
  `turtle/graph` answer by id. A removed term's `uri` and `range` are where
  the base described it; it has no node in the graph and is drawn as a
  ghost. A literal's id in a removed row is the base side's.
- **What the words mean.** Triples are compared per file as sets of
  (subject, predicate, object) keys; the base file is parsed with the same
  prefixes and base IRI the current file gets.
  - *Added / removed triple*: in only one side's set.
  - *Changed row*: one removed and one added triple of the same file with
    the same subject and predicate, when that pair is the only removal and
    the only addition for that subject and predicate. Any other mix stays
    added and removed rows.
  - *Added / removed term*: an IRI that is a node on one side only, over the
    whole workspace (a term moved from one file to another is neither).
  - *Changed term*: on both sides, and the subject of an added or removed
    triple.
  - *Impacted term*: not added, removed or changed, and the subject of a
    current triple whose object is an added or changed term, found with the
    references index. `rdf:type` objects count, so a changed class impacts
    its instances; the list is cut at `limit`.
  - *Blank nodes* are keyed by their path from the nearest IRI subject (the
    subject's IRI, then each predicate, then the index among that
    subject's objects for that predicate with the same path), not by their
    position in the file. A blank node with no IRI ancestor keeps its
    position key.
- **Where the code lives.** A new module `src/features/graph_diff.rs` owns
  the comparison: pure functions over the knowledge index, the current
  analyses and the parsed base. It reuses `graph.rs`'s node id and name
  functions (made `pub(crate)` where they are private). `ide::display`
  gets `Session::graph_diff` and a `GraphDiffJob` to run off the session
  thread like `GraphJob`; `server.rs` routes the request. The view keeps
  one more piece of state, the diff answer, and the canvas reads a status
  per node and edge from it; the change list is a new view file,
  `web/app/graph/changes.js`.
- **The view.** A "Changes" toggle in the graph view's toolbar, shown when
  the host gives a `base` function (VS Code and the browser editor do). On:
  the view asks `turtle/graphDiff` with the host's base and draws the
  overlay on whatever mode is shown (Graph, Focus, Hierarchy keep working);
  the change list opens in the side panel, grouped by file, rows `+`, `−`
  and `~ old → new`, then the terms by status; ghosts for removed terms
  and edges sit beside their current neighbours. It re-asks on the same
  debounce as the graph. Off: nothing is asked and nothing is drawn.
- **Hosts.** The view's options gain `base: () => Promise<Base | Unavailable>`
  (a list of `{uri, text}`, or a reason string shown in the panel). The
  VS Code panel (`editors/vscode/src/graph.ts`) implements it with the git
  API and passes it through the webview (`graph-webview.js`). The browser
  editor (`web/app/editor.js`) records the load-time texts and lists the
  files whose text differs, plus files created and deleted since.

## Testing Decisions

- Behaviour at each seam, prior art for each:
  - The comparison: Rust unit tests in `src/features/graph_diff.rs` as in
    `src/features/graph.rs` (build a workspace of analyses, assert the
    answer): added, removed, changed rows; a value change paired and an
    ambiguous one not; reformatting, reordering and a prefix rename give no
    rows; blank-node insertion gives no change; created and deleted files;
    a term moved between files is not added or removed; impacted through
    `rdf:type` and through an object; the limit and `cut`; ranges on both
    sides.
  - The boundary: unit tests of the params parse (too many entries, a text
    over the limit, a duplicate URI, a non-workspace URI).
  - The request over stdio: `test/graph-diff.test.mjs`, as
    `test/graph.test.mjs` does for `turtle/graph` (capability advertised;
    an edit to an open file and a changed closed file both answered; ids
    match the `turtle/graph` answer's).
  - The view: `web/test/graph-ui.test.mjs` with its fake server answering
    `turtle/graphDiff` (toggle hidden without `base`; colours, ghosts, list
    rows and counts; a row opens its range; the toggle off asks nothing;
    the "nothing to compare with" reason).
  - The browser editor: `web/test/editor-graph.test.mjs` (edit
    `data/people.ttl`, turn on Changes, see the row and counts; the WASM
    server answers).
  - VS Code: a unit test of the base collection with a fake git API in
    `editors/vscode/test/`, and the panel check in `editors/vscode/test/suite.cjs`.

## Principles

- model-the-domain, type-system-discipline: the row and term status are
  enums with payloads (a changed row carries `was`, an added one does not);
  the params become typed values once.
- boundary-discipline: the client-supplied base is parsed and limited at the
  request boundary (entries, sizes, URIs); the comparison trusts it.
- foundational-thinking: the request and answer shape is fixed here first,
  so the server, view and VS Code slices build against it at once.
- experience-first: one toggle on the existing view instead of a new
  panel; the prototype's before/after variant is dropped.
- separate-before-serializing-shared-state: the slices own disjoint files;
  `src/features/mod.rs` and `server.rs` are each touched by one slice only.
- explain-the-number: the diff timing in the close-out.

## Non-functional budgets

- The keystroke rows of `bench/perf.mjs` do not move (nothing on the
  keystroke path changes).
- `turtle/graphDiff` answers faster than the `turtle/graph` request it
  colours, for the same workspace. Measured on 1,000 files of 1,000
  subjects (about 3 million triples), release build, load average 4 to 10:
  10 changed files with 5 changed values each, median 175 ms (2 runs of 10,
  170–197 ms); every line of the 10 files changed, 216 ms; `turtle/graph`
  with no layout, 1 to 1.5 s. About 60% of the diff is the job rebuilding
  the knowledge index from its sources, as `GraphJob` does (issue #71); the
  first budget, under 100 ms, assumed no rebuild.
- With the toggle off, the graph view sends no `turtle/graphDiff`.

## Out of Scope

- Comparing with the merge base, a branch or a commit other than HEAD.
- The Neovim plugin's UI (the request is there for it).
- The before/after side-by-side view.
- Running git from the server, or reading `.git` directly.
- Moving a blank node's identity across files.
- SPARQL files (they hold no data).
