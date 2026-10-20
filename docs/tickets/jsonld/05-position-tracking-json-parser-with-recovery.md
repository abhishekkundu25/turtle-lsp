# 05: JSON-LD: position-tracking JSON parser with recovery

**What to build:** a fault-tolerant JSON parser that keeps the span of every key, value and bracket, recovers at commas and brackets, and reports `json-syntax` errors. A `.jsonld` file with a missing comma shows one error at the right place and still parses the rest. Tokens feed semantic highlighting later.

**Blocked by:** 01

**Status:** ready-for-agent

**Parallel slices:** owns the JSON parser module inside the JSON-LD engine.

- [ ] Syntax diagnostics appear at exact positions in the editor and in `check`
- [ ] Recovery: errors later in the file are still reported; valid parts still produce values
- [ ] Phase-1 pending tests for JSON syntax are promoted
- [ ] Parses a 30,000-line file in about the same time as the Turtle parser on the same size

Spec: docs/specs/jsonld.md ("Engine parts").
