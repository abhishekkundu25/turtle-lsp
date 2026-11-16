# Cancelling and capping background reasoning

Status: done. Issues #85 and #89. Builds on
[turbowl-reasoning.md](turbowl-reasoning.md) and
[proof-limits.md](proof-limits.md), and needs TurbOWL 0.2.2
(`Reasoner::reason_cancellable`, `Cancel::with_check`).

## Problem Statement

Reasoning is on by default since 0.17.0, so a background run that cannot
stop reaches every user. A run uses TurbOWL's `reason`, which no one can
stop, and then reads every inferred triple to find the inferred types,
with no cap. Measured with reasoning on, on an `owl:sameAs` chain with one
property per node: 3,000 nodes take 1 s, 10,000 nodes 11 s, and 30,000
nodes are still running after 90 s. Each save queues another run behind
the one going on.

Also, every edit, open and close reruns reasoning, including in a SPARQL
query file, which adds no triples. When a run's outcome arrives after a
newer edit, its progress stays on, so "TurbOWL: reasoning…" can show while
nothing runs.

A cloned repository's `turtle-lsp.json` can choose the rules, and
`turtle.reasoning.rules` is not one of VS Code's restricted settings in
untrusted workspaces.

## Solution

A run stops when newer work comes in, and at a time limit of 10 s. Either
way the previous result stays: hover reads it until a run that finishes
replaces it. A run stopped by its time limit says so once, in plain words.
Reading the inferred types stops at `turtle.reasoning.maxTriples` inferred
triples, and hover says that the list may be incomplete. Edits, opens and
closes of SPARQL files do not rerun reasoning, and a dropped outcome ends
the progress when no newer run is going on. In an untrusted VS Code
workspace only the user's own `turtle.reasoning.rules` counts.

## User Stories

1. As an ontology author, I want an edit to stop the reasoning run going on, so that the next run starts on the new text at once instead of waiting behind a stale one.
2. As an ontology author, I want hover to keep showing the last result while a new run goes on, so that inferred types do not vanish while I type.
3. As a user with a large or deeply chained workspace, I want a run to stop after about 10 s, so that reasoning, which is on by default, never keeps a core busy for minutes.
4. As that user, I want to be told once, in plain words, that reasoning stopped at its time limit and that hover shows the last result, so that I know why the inferred types are old and how to turn reasoning off.
5. As that user, I want no pop-up on every pause in typing, so that a workspace too large for the limit stays usable.
6. As an ontology author, I want hover's inferred types read at most up to `turtle.reasoning.maxTriples` inferred triples, with a note when they were cut, so that an `owl:sameAs` clique cannot make the server read billions of triples.
7. As a SPARQL author, I want my edits in `.rq` and `.ru` files not to rerun reasoning, so that typing a query costs nothing.
8. As a user, I want the progress "TurbOWL: reasoning…" to end when no run is going on, so that it never shows over nothing.
9. As a user who opens someone else's repository in VS Code without trusting it, I want its `turtle-lsp.json` and workspace settings unable to choose the rules, so that only my own setting decides what runs.
10. As a browser-editor user, I want the same time limit in the browser, which has no threads, so that a file cannot freeze the page.
11. As a user, I want opening, closing and saving a file whose triples did not change to cost no reasoning, so that the new cancellable run is no slower than before.
12. As a maintainer, I want the time limit as a constant beside the other reasoning limits, so that there is no new setting to document.

## Implementation Decisions

- **One cancel handle per job.** The server makes a job's handle when it makes the job and keeps it; a newer job, an edit that supersedes the run (`invalidate`) and turning reasoning off trip it. The run reasons with `reason_cancellable` through a handle made with `Cancel::with_check`: it trips when the job's handle is tripped or the run's deadline passes, by the server's clock (`crate::clock::Instant`, the page's in the browser). TurbOWL checks it before each round; the run checks it again every 65,536 inferred triples it reads. A round is not interrupted.
- **The time limit** is `Limits::run`, 10 s, beside the hover and findings limits; the run takes it from the state's `Limits`, so tests pass a short one. Explaining the findings keeps its own 1 s limit after it.
- **The previous result stays.** A run reasons on a reasoner of its own, outside the state's lock, and swaps its result in only when it finishes. A stopped run leaves the state as it was. The cost is that, while a run goes on, the previous reasoner is held beside the new one. Hover reads the previous result during the run instead of saying that reasoning is running; it says so only during the swap.
- **No reasoning when the triples did not change.** When the base has the same triples (as a set), the same rules and the same `maxTriples` as the result held, the run keeps that reasoner and its inferred types, and only takes the new base (the new places of the triples) and its findings. Opening, closing or saving a file, or an edit to a comment, costs no round. TurbOWL's own reuse of the last run (an incremental run after additions) is given up: it changes the reasoner hover reads.
- **What the server does with an outcome.** A run that finished replaces the findings and the result hover reads, as before. A run stopped by newer work gives nothing; its outcome is dropped. A run stopped at its time limit keeps the findings shown, has hover read the result the state holds (the previous one), ends the progress with "Stopped at the 10 s time limit; hover shows the last result" (or "hover and the findings show", with findings on), logs one line (`window/logMessage`), and shows one warning (`window/showMessage`) only on the first stop since a run last finished, so a workspace too large for the limit does not raise a pop-up after every pause in typing.
- **Inferred types.** Reading them stops past `turtle.reasoning.maxTriples` inferred triples (the limit queries use), and the types read so far are kept. Hover then adds "TurbOWL inferred more than N triples (`turtle.reasoning.maxTriples`): only those were read, so this list may be incomplete."
- **Which edits rerun reasoning.** An edit, open or close of a SPARQL document does not invalidate the run or schedule one; any other document does. Saves and disk changes go through the workspace's effects as before.
- **Progress after a dropped outcome.** The server keeps the generation of the last job it submitted. A dropped outcome whose job was the last submitted means no run is going on: the progress timer is cleared and the progress ends. A dropped outcome of an older job leaves the progress to the newer one.
- **Untrusted workspaces (VS Code).** `turtle.reasoning.rules` joins `restrictedConfigurations`, so workspace settings do not count there. The server reads `turtle-lsp.json` itself and does not know VS Code's trust, so in an untrusted workspace the extension always sends `reasoning.rules`: the user's own value, or the default. That overrides the file, natively and in the browser (where the extension layers the file under the user's settings). The default stays `default`: reasoning stays on in untrusted workspaces, now that a run is capped; the user can turn it off in their own settings. Other editors have no workspace trust; there `turtle-lsp.json` may still choose the rules, under the same time limit.

## Testing Decisions

- Test at the reasoning worker's seam (`run_inline` with a job and a state) and at the server's (`Runs`, the embedded server), as the existing tests do: hand in a workspace and check the outcome, what hover reads and what the server sends.
- A long `owl:sameAs` chain makes a slow run. With a short time limit passed in, the run stops, the state still holds the previous result, and hover reads it. A run cancelled from another thread stops well before a full run. Tests that measure time are named `timing_…` and bound by a measured full run or by generous multiples of the limit.
- Inferred types: a small `maxTriples` cuts them, keeps the ones read, and hover's note says so (a unit test of the hover section).
- Unchanged triples: a second run over the same text with other places reasons no round (a test counter) and hover's links follow the new places.
- The server: an edit to a SPARQL document schedules no run; a dropped outcome of the last job ends the progress; a stopped run keeps the findings and shows one warning per streak.
- VS Code: the manifest test checks `turtle.reasoning.rules` is restricted and that an untrusted workspace sends the user's rules or the default.
- Prior art: the worker tests in `src/reasoning/worker.rs`, `src/server/reasoning.rs`, the embedded server tests in `src/server.rs`, `test/reasoning.test.mjs` and `editors/vscode/test/manifest.test.mjs`.

## Out of Scope

- A memory budget for a run (TurbOWL #7, turtle-lsp #86): a run's memory is bounded only by its time.
- Interrupting a TurbOWL round partway through.
- A setting for the time limit.
- Workspace trust in editors other than VS Code.
- Restricting `turtle.reasoning.diagnostics` in untrusted workspaces: its proof searches already have their 1 s limit.
