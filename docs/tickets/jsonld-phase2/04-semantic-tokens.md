# 04: Semantic tokens for JSON-LD

**What to build:** semantic tokens (full and range) for JSON-LD from the source index: `keyword` for keywords and aliases (keys and values), `property` for keys that expand to properties, `class` for `@type` values, with the deprecated modifier as Turtle uses it. No new legend entries.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** src/features/semantic.rs only.

- [ ] Promote: "semantic tokens mark keywords, properties and classes"
- [ ] Added: an alias used as a value is a keyword; range requests return only tokens in range
