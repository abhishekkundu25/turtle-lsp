# 07: Completion in JSON-LD

**What to build:** completion in keys (context terms, compact IRIs, `@vocab` properties the workspace describes) and `@type` values (classes), from the innermost scope at the cursor, working on half-typed JSON; completing a compact IRI whose prefix the context lacks adds the prefix to `@context` as an additional edit (idea credited to SWLS in a comment). Same ranking and resolve as Turtle.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** src/features/completion.rs (remove the JSON-LD early return, dispatch) and new src/features/completion/jsonld.rs.

- [ ] Promote: the three completion tests
- [ ] Added: half-typed key (unclosed string); scoped context offers its own terms; missing-prefix completion inserts the `@context` entry
