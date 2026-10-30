## Problem Statement

Five usability reviewers used the Schema view and Follow cursor (docs/specs/schema-view.md) in real Chrome on a small workspace with every problem kind and a big one (1,500 classes, 20,000 instances). They found it useful (mean 3.6/5) and correct (every count matched a hand count; every place opened on the exact line), but it felt rough (mean 3.0/5). Their shared findings, each confirmed in the code or by screenshot:

- Link chips read `→ //example.or…` for IRIs without a prefixed name.
- Fit shrinks text to 5-8 px; a big model opens at 5% zoom with no class in focus.
- In a split narrower than 700 px, Schema is blank until a class is selected.
- The Graph mode's filters (colour by, depth, node cap, kinds) stay in Schema, where they do nothing, and take 240 px.
- Follow cursor on an instance does nothing, and a lit row stays lit after the cursor moves.
- Search finds only the 300 listed classes, and Enter selects nothing.
- An edit resets zoom and pan and moves the boxes.

and, from single reviewers: problems listed after all the rows in the inspector; a conflict row that says only "range ≠ shape" while its values fail the shape; rows without an accessible name; Escape not closing the inspector or the legend; "declared nowhere" for a property declared for another class; a full rebuild of every box on each Follow cursor jump; a cap ("Showing 300 of 1,502") that does not say how to reach the rest; and a Follow cursor toggle with no visible effect when the graph is a tab rather than beside a file.

## Solution

The Schema view reads at a glance, keeps the user's place, and every class is reachable:

- Names are prefixed names or the IRI's local name, never a cut `<http://…`.
- The first view is readable: a small model fits; a big one opens on its focus class (the one with most instances) and its neighbours at a readable zoom.
- A narrow split shows the inspector for the focus class at once.
- Schema shows only the filters that apply to it (Files, search, Problems only, imports).
- Follow cursor on an instance goes to its class; the lit row follows the cursor.
- Search looks through every class, listed or not, and Enter goes to the first match, re-centring the answer on it when it was not listed.
- Edits keep zoom, pan and the selection; a box that moves keeps the view anchored to the selected class.
- The inspector lists problems first; a conflict row counts and lists the values that fail the shape; a not-declared row says which classes declare the property.
- Rows have accessible names that state their problem; Escape closes the legend, then the inspector, returning focus to the box.
- The status says how many classes are shown and how to reach the others; the Follow cursor tooltip says it follows the editor beside the graph.
- A Follow cursor jump that keeps the same listed classes updates the selection in place, without rebuilding the boxes.

## User Stories

1. As a user, I want link chips to show the target class's short name, so that I can tell links apart.
2. As a user, I want boxes and rows to show prefixed names, or the local name when no prefix applies, so that rows never look identical.
3. As a user with a small model, I want Fit to show every class at a readable size, so that the first view needs no zooming.
4. As a user with a big model, I want the first view centred on the most-used class and its neighbours at a readable zoom, so that I start somewhere useful.
5. As a user, I want Fit never to go below a readable zoom when a focus class exists, so that "fit" does not mean "unreadable".
6. As a user in a narrow split, I want the inspector of the focus class shown at once, so that the panel is never blank.
7. As a user in Schema, I want only the filters that apply to Schema shown, so that I am not offered controls that do nothing.
8. As a user with Follow cursor on, I want the cursor on an instance to show its class, so that following works in data files.
9. As a user, I want the lit row to move with the cursor and go when the cursor is off a property, so that it never points at the wrong thing.
10. As a user with a big model, I want search to find any class, listed or not, so that every class is reachable.
11. As a user, I want Enter in search to go to the first match, so that search is one keystroke.
12. As a user, I want a found class that was not listed to become the focus of a new answer, so that I see it with its neighbours.
13. As a modeller, I want an edit to keep my zoom, pan and selection, so that I do not lose my place while typing.
14. As a data engineer, I want problems listed before the rows in the inspector, so that the work comes first.
15. As a data engineer, I want a conflict row to count the values that fail the shape and list them, so that I can fix the data or the model.
16. As a modeller, I want a not-declared row to say which classes declare the property ("declared for Pipe, not Gasket"), so that I know whether to move a domain or fix the data.
17. As a screen-reader user, I want each row to have a name that says the property and its problem, so that I hear what a sighted user sees.
18. As a keyboard user, I want Escape to close the legend, then the inspector, and return focus to the box, so that I can back out without the mouse.
19. As a user with a big model, I want the status to say how many classes are shown and that search or Follow cursor reach the rest, so that the cap is not a mystery.
20. As a first-time user, I want the Follow cursor tooltip to say it follows the editor beside the graph, so that I know to split the view.
21. As a user moving the cursor quickly, I want each jump to update the view without rebuilding every box, so that following stays smooth.
22. As a user, I want the legend to explain the usage bar, "30/57" and "1..1", so that every mark has a meaning I can look up.

## Implementation Decisions

**Server (`features::schema`)**, additive fields, already typed in the seam commit:

- Params: `search?: string`. The answer's `matches: [{ id, name, label?, index? }]`, at most 20, best first: exact local name, then prefix of name or label, then substring of name, label or IRI; ties by instances then IRI. `index` when the class is listed.
- Focus: a focus IRI that is not a class or a property but a typed subject (an instance) resolves to its first type that is a listed-or-listable class; `focus` and, when the cursor was on a property use, `focusRow` follow.
- Conflict rows: `shapeBadValues` counts values outside the shape's `sh:datatype` (literals of another datatype) or `sh:class` (resources not typed with it, when the shape says `sh:class`); examples of kind `shapeValue` on the examples class, like `badValue`.
- Not-declared rows: `declaredFor` lists the names of the classes whose domain or shapes declare the property.

**Client (`web/app/graph/`)**:

- A display name helper: the prefixed name, else the IRI's local name (after the last `#` or `/`), with the full IRI as the title. Used for boxes, rows, chips and the inspector.
- Fit: when every box fits at zoom ≥ 0.6, fit; else centre the focus class (the answer's `focus`, else the first class) at zoom 0.9 with its neighbours around it. The Fit button keeps today's whole-model fit for those who want it, with a floor of `FAR` (folded headers).
- Narrow: the inspector shows the focus class when nothing is selected.
- Filters: in Schema mode the side panel shows only Files and search; the bar shows Problems only, imports and "?".
- Follow: the lit row is cleared and re-set on each follow answer; a follow answer whose class ids equal the current ones updates selection, lit row and camera in place (no rebuild).
- Search: Schema search sends `turtle/schema` with `search` (debounced 150 ms) and shows `matches`; Enter selects the first; a match without `index` asks again with `focus` set to it.
- Camera: a refresh keeps zoom and pan; when the layout changes, the view is shifted so the selected (else focus) box stays at the same place on screen.
- Inspector: problems first, then rows; conflict problems list `shapeValue` places; not-declared problems say "declared for X, not Y" from `declaredFor`.
- Accessibility: each row has `role="listitem"` (in a `role="list"` per box) and an `aria-label` "<property>, <range>, <state words>"; Escape closes the legend if open, else clears the selection and focuses the box.
- Status: "Showing 300 of 1,502 classes · search or Follow cursor reach the others" when capped.
- Follow cursor tooltip: "Show the term at the cursor of the editor beside this view".
- Legend gains the usage bar, "used/of" and "min..max".

## Testing Decisions

Tests check what a user sees and what a client receives (prior art: src/features/schema/tests.rs, web/test/schema-ui.test.mjs, test/schema.test.mjs).

- Rust unit tests: instance focus to its class (and focusRow for a property use); search ranking, unlisted matches without index, MAX_MATCHES; shapeBadValues for sh:datatype and sh:class with examples; declaredFor names. The recount oracle checks shapeBadValues and declaredFor against its plain recount.
- Protocol: `search` returns matches for an unlisted class with maxClasses 1.
- Browser (schema-ui): chip and box names for an unprefixed IRI; big model opens readable (zoom ≥ 0.6) centred on the focus; small model fits; narrow inspector not blank; Schema hides graph-only filters; follow on an instance; lit row cleared; search finds an unlisted class via the server and Enter re-centres on it; refresh keeps camera (box screen position of the selection unchanged within 2 px); problems first; conflict places; declared-for text; row aria-labels; Escape order; status text; follow updates in place (same box elements before and after).

## Performance budgets (pass/fail)

- The `schema` bench rows do not regress beyond their spread; search adds under 5 ms on the 2,000-class workload.
- A Follow cursor jump that keeps the listed classes takes under 16 ms of main-thread work in the page (no long task).
- The keystroke rows of `editing` and `big-file` do not move.

## Out of Scope

- Inherited rows shown on subclasses and per-subclass problem counts.
- The editor-wide focus-ring and amber contrast tokens.
- A severity filter for models where every class has a problem.

## Principles

- **redesign-from-first-principles**: one display-name helper replaces the scattered `slice(indexOf(":"))`; the camera keeps an anchor instead of refitting on refresh.
- **experience-first**: every change here comes from a reviewer's observed confusion; the first view, the narrow view and the place-keeping come first.
- **boundary-discipline**: `search` is validated with the other params; the client trusts the answer.
- **explain-the-number**: the follow-in-place budget is measured with long-task observation, as reviewer 5 measured it.
