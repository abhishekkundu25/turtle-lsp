# JSON-LD phase 2 tickets

Spec: docs/specs/jsonld-phase2.md. Built with parallel slices.

| # | Ticket | Blocked by | Files (lock in bold) |
|---|---|---|---|
| 01 | Source index | none | **src/jsonld/expand.rs, context.rs, mod.rs** |
| 02 | Formatting | none | new src/jsonld/format.rs; src/features/format.rs; src/batch.rs |
| 03 | Outline nests named graphs | none | src/features/symbols.rs |
| 04 | Semantic tokens | 01 | src/features/semantic.rs |
| 05 | Definition, references, highlights | 01 | **src/features/navigation.rs** |
| 06 | Rename | 05 | **src/features/navigation.rs**, new src/jsonld/rename.rs |
| 07 | Completion | 01 | src/features/completion.rs, new src/features/completion/jsonld.rs |
| 08 | Close-out | all | README, CHANGELOG, bench, corpus round trip |

Waves: 01, 02, 03 together; then 04, 05, 07; then 06; then 08.

Every slice promotes its own pending tests in test/jsonld.test.mjs (separate
hunks, so cherry-picks merge). No slice edits README or CHANGELOG; 08 does.
