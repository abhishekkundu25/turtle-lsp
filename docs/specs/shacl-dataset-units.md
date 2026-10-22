# SHACL: less repeated work for TriG and N-Quads datasets

Status: steps 2 and 3 in part done after 0.7.1 (Unreleased; audit 2026-10, ticket 20): the shared graph is built once and each dataset is read as an overlay on it, with only the nodes the dataset mentions as focus nodes. Steps 1 and 4 (a measurement, parallel dataset units) are not done.

## Problem Statement

SHACL validation splits the workspace into units:
- one **shared unit** of every Turtle document;
- one **dataset unit** per TriG or N-Quads document, holding that document
  plus every shared document.

Each dataset unit builds its own data graph (and, for SHACL-SPARQL, its own
query index) from scratch, including a full copy of the shared data. It then
validates the dataset's focus nodes, and a focus node the shared data also
describes is validated again. Since the duplicate-results fix, results the
shared unit already reported are discarded afterwards, so the output is right
but the work is repeated.

The cost grows with:
- the number of dataset documents;
- the size of the shared data;
- how many shared nodes the datasets mention.

The one measured project spends about 0.5 s on SHACL:
- 8 generated TriG files of about 30 MB in total;
- shared Turtle shapes and data.

Nobody has measured what share of that is repeated work. A project with many
small TriG files over a large Turtle model pays the copy of the model once
per file.

The repeated work is on the critical path of both the editor (validation
after typing stops) and `turtle-lsp check`.

## Solution

Do each piece of work once, keeping exactly the same results:
1. **Measure first.** Record, for the dataset units, the time spent building
   graphs and indexes against the time spent validating, and the share of
   discarded results. The remaining steps are justified only by what this
   shows.
2. **Build the shared graph once.** Each dataset unit layers its own triples
   over the shared graph and index, without copying them.
3. **Skip what cannot change.** Re-validate a shared focus node in a dataset
   unit only when the dataset adds a triple that a shape can reach from it.
   Skip a dataset unit entirely when no shape targets anything it contains.
4. **Run dataset units in parallel.** They are independent once the shared
   graph is read-only.

## User Stories

1. As an editor user with many TriG files, I want SHACL results to arrive sooner after I stop typing, so that validation keeps up with editing.
2. As a CI maintainer, I want `turtle-lsp check` on a dataset-heavy repository to take time proportional to the data, not to the data times the number of files.
3. As a user, I want exactly the same SHACL results as today, in the same files and positions.
4. As a user on a laptop, I want background validation to use less CPU and memory, so that the editor stays responsive and the battery lasts.
5. As a maintainer, I want a measurement of where dataset validation time goes, so that optimisation targets real cost.
6. As a maintainer, I want a benchmark for "many datasets over a large shared model", so that this stays fast.

## Implementation Decisions

**Results are the contract.** Every change must leave validation output
byte-identical: results, the files they are placed in, positions, messages,
and the per-document truncation. The before-snapshot technique used for the
SHACL-SPARQL batching (one sorted line per result, compared byte for byte)
is the acceptance check.

**Measure first.** Use timing spans around graph building, index building
and validation per unit. These are not shipped as a user setting, and are
removed or left behind a test-only switch. Steps 2 to 4 are done only as far
as the numbers justify.

**A layered graph and index.**
- The shared unit's graph and SPARQL index become read-only and shared.
- A dataset unit is the shared layer plus a small overlay of the dataset's
  triples, and queries see the union.
- **Ids:** term interning must give the overlay ids that do not collide.
  Blank nodes keep their document of origin; the duplicate-results fix
  already needed this mapping.
- **Shape parsing:** a dataset that contributes shapes needs the shapes
  re-read from the union. That is the uncommon case and may stay a full
  rebuild.

**Skipping, kept conservative.**
- A shared focus node is re-validated in a dataset unit only if the dataset
  has a triple whose subject or object is reachable from that node along the
  paths of the shapes that target it, or that changes its targets (a new
  `rdf:type`, a new subject of a target predicate).
- When reachability is unclear, the node is re-validated. Correctness
  outranks speed.
- **Never skipped:** SHACL-SPARQL constraints (they can read the whole graph)
  and shapes using `sh:closed`, inverse or alternative paths deeper than one
  step, or `sh:qualifiedValueShape`.

**Parallel units.**
- Dataset units run on the existing thread pool. Results merge in document
  order, so output is identical whatever the scheduling.
- **Cancellation** (a new edit arriving) stops every unit.
- **Memory:** the overlay design keeps peak memory near one copy of the
  shared data, not one per thread.

**Unchanged:**
- The unit rules themselves: each dataset is validated with the shared data,
  never with other datasets.
- The duplicate-results rule from the earlier fix.
- The `maxTriples` limit, which still applies to each unit's union.

## Testing Decisions

**What a good test is.** Only validation output is asserted. The
before-snapshot comparison runs on:
- the SHACL Core and SHACL-SPARQL W3C suites, unchanged;
- a generated project of many small TriG files over a large Turtle model,
  with violations in shared data, in datasets, and caused only by
  combining the two;
- the existing dataset tests, including the duplicate-results regression
  test.

**Test cases for skipping:**
- a dataset adds a type to a shared node;
- a dataset adds a value along a two-step path;
- a dataset adds a triple a SPARQL constraint reads;
- an unrelated dataset.

Each must give the same results as validating everything.

**Parallelism:** the same output on repeated runs, a cancellation test, and
no leftover threads.

**Performance:** a benchmark row for "N datasets over a large shared model",
with a budget that rises with total data size, not with N × the shared size.
Asserted in release builds only.

**Prior art:**
- the batched-SPARQL equivalence test (forced on and off, identical output);
- the before/after snapshot used for the batching change.

## Out of Scope

- **The unit rules:** validating datasets together, or validating named
  graphs separately within a dataset.
- **Incremental validation across edits:** re-validating only what an edit
  touched. That is a bigger, separate design.
- **SHACL-SPARQL batching for nested `sh:node` checks.**
- **JSON-LD datasets:** they follow these rules once JSON-LD lands
  (see the JSON-LD spec).

## Further Notes

- **Order:** steps 1, 4, 2, 3 are in rising order of risk. Parallel units
  (4) are likely the cheapest real win. The layered graph (2) is the
  structural one. Skipping (3) has the most ways to be subtly wrong, so it
  goes last and only if the measurement shows re-validation dominates.
- **Background:** this follows the fix that stopped a TriG dataset repeating
  results the shared data already reports. That fix discards the duplicate
  results after computing them. This spec avoids computing them.
