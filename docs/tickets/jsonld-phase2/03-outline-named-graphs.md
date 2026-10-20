# 03: Outline nests a named graph's nodes

**What to build:** in the outline, a node object with `@id` and `@graph` is a graph symbol whose children are the node objects in its `@graph`, as TriG named graphs are shown.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

**Parallel slices:** src/features/symbols.rs only.

- [ ] Promote: "the outline nests a named graph's nodes under the graph"
- [ ] The phase-1 outline test ("lists each node object by its @id") still passes
