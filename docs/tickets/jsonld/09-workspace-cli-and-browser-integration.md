# 09: JSON-LD: workspace, CLI and browser integration

**What to build:** JSON-LD files join the workspace index like TriG files:
- their terms count in completion, hover and references in Turtle and SPARQL files;
- undescribed-term stops flagging terms they describe;
- `owl:imports` of JSON-LD resolve;
- `turtle-lsp check` reports them with the same codes, JSON and SARIF output;
- the browser editor's sample workspace can hold a `.jsonld` file.

**Blocked by:** 08

**Status:** ready-for-agent

**Parallel slices:** touches the workspace index, knowledge, the CLI file collection and the web engine. It does not touch SHACL validation (10) or JSON-LD lints (11).

- [ ] Phase-1 pending tests for cross-file knowledge, CLI and the browser are promoted
- [ ] A mixed Turtle and JSON-LD project shows no false undescribed-term warnings
- [ ] The editor and `check` agree on a mixed project (same diagnostics)

Spec: docs/specs/jsonld.md ("Workspace, SHACL, CLI and web").
