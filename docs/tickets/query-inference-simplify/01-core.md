# 01: One document list, no filter, no renumbering

**What to build:** spec Solution and Implementation Decisions, all but
Browser. Delete the queried-term filter, the reasoning-only document list
and the blank-node renumbering; the query reads and reasons over one list
(queried documents plus imports when reasoning is on), sorted as the base
sorts them when reasoning is on. `files` = those files plus external imports.
The note's sentence written once and used by the CLI and "▶ Run query".

**Blocked by:** none. **Status:** done (51487e2)

- [ ] Oracle test in `src/query/tests.rs`: for several workspaces (subclass chain, inverse, transitive, property chain, sameAs, blank nodes across two documents, an import), reasoning-on rows equal reasoning-off rows over TurbOWL's closure written as Turtle.
- [ ] Changed tests: an import's own triples are queried with reasoning on and not with off; `files` reads neither other workspace documents' axioms nor data (`test/query-request.test.mjs`).
- [ ] Every off-mode test unchanged and passing.
- [ ] fmt, clippy, lib unit tests, `test/query-cli.test.mjs`, `test/query-request.test.mjs`, `test/query-lsp.test.mjs`, `node web/build.mjs`, `web/test/engine.test.mjs`.
