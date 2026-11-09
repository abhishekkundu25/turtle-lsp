# Time limits on proof searches

Status: in progress. Builds on [turbowl-reasoning.md](turbowl-reasoning.md)
(hover proofs, reasoning findings) and needs TurbOWL 0.2.1
(`Cancel::with_check`).

## Problem Statement

A proof search has no limit. Hovering a term, or a run that explains its
findings, asks TurbOWL's explainer for proofs and waits for all of them.
On a small file this takes milliseconds, but the cost grows fast with the
length of a chain:

- An instance at the start of a 200-link `rdfs:subClassOf` chain takes
  6.8 s and 423 MB to hover. At 400 links the search was still running
  after 90 s, at 1.5 GB. The file is 15 KB.
- The same 400-link chain with a contradiction at its end, with findings
  on, takes the server past 3 GB about 10 s after the file opens, with no
  hover at all. The run explains its findings while it holds the reasoning
  state, so hover and the next run wait behind it.
- One `cax-adc` finding on a 32,000-member `owl:members` list takes 31 to
  71 s to explain.

`turtle-lsp.json` can turn reasoning and findings on for anyone who opens
a repository, and the browser editor has reasoning on by default and
searches inline with no limit, so one file can freeze the editor.

## Solution

Every proof search gets a time limit: 2 s for one hover's proofs, and 1 s
in total for one run's explanations of its findings. A search stops at its
limit and keeps what it found. Hover still lists every inferred type; a
type whose proof was not reached says "proof search stopped at its time
limit". A finding past the limit keeps its message without its proof, as
findings past the hundredth do today. The limit works the same natively
and in the browser.

## User Stories

1. As an ontology author, I want a hover on a term in a long class chain to answer within a few seconds, so that the editor never hangs on a hover.
2. As an ontology author, I want hover to keep listing every inferred type when the proofs take too long, so that I still see what the model implies.
3. As an ontology author, I want hover to say a proof search stopped at its time limit, not "no proof found", so that I do not mistake a limit for a bug.
4. As an ontology author, I want the proofs found before the limit to still show, so that the closest types keep their explanations.
5. As an ontology author, I want hovering again after the search stopped to answer at once from what was found, so that the same hover does not search again.
6. As a user with findings on, I want a workspace with an expensive contradiction to still get its findings quickly, so that a file with a long chain does not stall reasoning.
7. As a user with findings on, I want findings past the explanation limit to keep their message, so that I still see every finding, only without its proof.
8. As a user, I want hover and the next reasoning run not to wait behind a long explanation, so that editing stays responsive.
9. As a browser-editor user, I want the same limits in the browser, where reasoning is on by default, so that an example file cannot freeze the page.
10. As a user who opens someone else's repository, I want its `turtle-lsp.json` turning reasoning and findings on to be unable to drive the server to gigabytes through proof searches, so that opening a repository is safe.
11. As a user, I want small files to behave exactly as today, so that the limits change nothing when proofs are cheap.
12. As a maintainer, I want the limits as constants beside the other reasoning limits, so that there is no new setting to document.
13. As a maintainer, I want the limit to be TurbOWL's cancel handle, so that the search stops between its own steps and keeps its indexes for the next search.

## Implementation Decisions

- **TurbOWL 0.2.1** adds `Cancel::with_check`: a cancel handle that trips when a function returns true at one of its checks. turtle-lsp builds a deadline from it with its own clock (`crate::clock::Instant`, which the browser page supplies), so the same code limits native and browser searches, with no watcher thread. turtle-lsp pins `v0.2.1`.
- **Hover**: the reasoning state's proof search uses `explain_cancellable` with a 2 s deadline for all of the term's types together. It proves the types in their sorted order; once the handle trips, the remaining types are marked as stopped. A finished search, stopped or not, is the answer for later hovers of that term in that result, as today.
- **An inferred type's proof** has three states, not two: found, not found (a bug in the explainer, as today), and stopped at the time limit. Hover writes the third as "proof search stopped at its time limit" and sorts it with the types that have no proof.
- **Findings**: one run's explanations share one 1 s deadline through `explain_diagnostic_cancellable`. After it trips, the rest of the run's findings get no proof, as the findings past the hundredth.
- **The limits** are constants in the reasoning module, beside the existing caps (findings explained per run, findings per document, types listed per hover). The function that searches takes the limit as a parameter, so tests can pass a short one.

## Testing Decisions

- Test at the reasoning state's seam (the hover search) and at the findings' seam (the function that builds a run's findings), as the existing tests in the reasoning module do. A good test hands in a workspace and checks what the user would see: the types, which have proofs, which are stopped, the findings and their messages.
- Each limit has a test on a long `rdfs:subClassOf` chain (and one with a contradiction at its end for findings) with a short limit passed in: the search returns within a generous bound, every inferred type is still listed, and at least the far types are marked stopped. A small file with the same short limit still gets every proof.
- Hover's rendering of a stopped type has a test beside the existing hover proof tests.
- The browser path needs no test of its own: it runs the same function with the same clock type, and the browser engine's existing hover test still passes.
- Prior art: the worker tests in the reasoning module (`one_proof_search_at_a_time_and_a_hover_again_gets_its_answer`), the findings tests, and `test/reasoning-hover.test.mjs`.

## Out of Scope

- A memory budget for the explainer (TurbOWL #7, turtle-lsp #86).
- Cancelling or capping the reasoning run itself (#85).
- Making the explainer cheaper on transitive chains (TurbOWL #6).
- Settings for the limits.
- Building findings only up to the per-document cap (filed separately).

## Further Notes

- The numbers in the problem statement come from an independent review of the reasoning work, measured on an M-series Mac.
- How long the search takes to stop after its deadline depends on how long one step of TurbOWL's search runs; the TurbOWL change measures that on the 400-link chain.
