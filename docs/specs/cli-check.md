# Command-line check and format (for agents and CI)

Status: spec, not scheduled. Research: docs/research/cli-check-mode.md.

## Problem Statement

turtle-lsp finds problems only for someone running an editor with a
language server client. The binary has two modes: it runs as a server, or it
prints its version with `--version`.

**Coding agents.** Most agents (Codex, Aider, Cursor's agent, Claude Code in
the cloud) work through shell commands, and have no language server client.
An agent that writes Turtle or SPARQL cannot check its work partway through a
task. It finds out about a missing prefix, a misspelled vocabulary term or a
SHACL violation only after the user does.

**CI.** Teams cannot fail a pull request on the problems their editor shows.
The RDF validators they could use instead have gaps:
- **No positions:** pySHACL, Jena `shacl validate`, rudof and TopBraid report
  a focus node and a path, never file:line:col.
- **Wrong exit codes:** Jena and rudof exit 0 on data that does not conform.
- **One error per file:** `riot --validate` stops at the first error in a file.
- **No CI output:** none of them writes GitHub annotations or SARIF.

## Solution

Two subcommands on the same binary. With no arguments it is still the
language server, so no editor setup changes.

- **`turtle-lsp check [PATHS...]`** reports every problem the editor would
  show, SHACL validation included, for the files under PATHS. It loads the
  whole workspace (prefixes.ttl, shapes, cached imports) so results match the
  editor. Default output is one line per problem:
  `path:line:col: severity[code] message`, then a summary line. It exits 0
  when clean, 1 when problems fail the check, and 2 when the tool itself
  fails.
- **`turtle-lsp format [PATHS...]`** formats files in place. With `--check`
  it changes nothing and lists the files that would change; with `--diff` it
  prints a unified diff instead.

Output formats: `concise` (the default), `json`, `github` (workflow
annotations) and `sarif` (GitHub code scanning). `--fix` applies the safe
quick fixes.

## User Stories

1. As a coding agent, I want to run `turtle-lsp check` after editing a Turtle file and get every problem with its file, line and column, so that I can fix my mistakes before I hand the work back.
2. As a coding agent, I want the output short and stable, one line per problem, so that it costs few tokens and is easy to parse.
3. As a coding agent, I want "did you mean" suggestions and prefix declarations in the message, so that I can fix a problem without another lookup.
4. As a coding agent, I want `--fix` to apply safe fixes (add a missing prefix, remove an unused one), so that I spend no turns on mechanical fixes.
5. As a coding agent, I want `--format json` with each problem's range, code and available fixes, so that I can act on the result programmatically.
6. As a coding agent, I want to check one file and still have the project's shapes, prefixes and imports taken into account, so that I don't report false "undeclared prefix" or "unknown term" errors.
7. As a coding agent, I want the exit code alone to tell me if the files are clean, so that a hook or script can decide without parsing output.
8. As a Claude Code user, I want a documented `PostToolUse` hook that runs the check after every edit of an RDF file, so that the agent sees problems automatically.
9. As a user of any agent, I want a ready-made AGENTS.md / CLAUDE.md snippet telling the agent to run the check, so that setup is a paste.
10. As a CI maintainer, I want `turtle-lsp check` to exit 1 on syntax errors and SHACL violations, so that a broken graph fails the build.
11. As a CI maintainer, I want warnings not to fail the build unless I pass `--max-warnings N`, so that I can adopt it gradually.
12. As a CI maintainer, I want `--format github` to annotate the pull request at the exact lines, so that reviewers see problems inline.
13. As a CI maintainer, I want `--format sarif` that GitHub code scanning accepts, so that problems show in the Security tab and on pull requests beyond the annotation limit.
14. As a CI maintainer, I want `turtle-lsp format --check` to fail when a file is not formatted, so that the repository stays consistently formatted.
15. As a CI maintainer, I want a copy-paste GitHub workflow that downloads the release binary and verifies its checksum, so that setup takes minutes and needs no Rust toolchain.
16. As a CI maintainer, I want imports read from the offline cache by default and fetched only with `--network`, so that builds are reproducible and don't depend on remote servers.
17. As a CI maintainer, I want SHACL being skipped (data too large) reported as a warning, so that a build never passes quietly without validation.
18. As a pre-commit user, I want a `.pre-commit-hooks.yaml` in the repository, so that adding the hook is three lines of config.
19. As a project owner, I want an optional settings file in the repository with the same keys as the editor settings, so that CI, agents and every editor apply the same rules.
20. As a project owner, I want `--select` and `--ignore` by diagnostic code, so that I can switch off a rule my project doesn't follow.
21. As a project owner, I want `--no-shacl`, so that I can run fast syntax-and-lint checks separately from validation.
22. As a user with a large dataset, I want the check to parse files in parallel and build the SHACL data graph once, so that a whole repository checks in seconds.
23. As any user, I want `--statistics` to count problems per code, so that I can see what dominates a large backlog.
24. As any user, I want colour only when output goes to a terminal and `NO_COLOR` is unset, so that logs and agent transcripts stay clean.
25. As any user, I want paths printed relative to the current directory, so that they are short and clickable in a terminal.
26. As any user, I want problems sorted by file and position, so that output is the same on every run and diffs of the output are meaningful.
27. As an editor user, I want the bare binary to keep working as the language server, so that nothing in my editor setup breaks.
28. As a Windows user, I want the same commands to work with backslash paths and CRLF files, so that CI on Windows behaves like CI on Linux.
29. As a maintainer of turtle-lsp, I want the CLI and the browser toolbox to share one batch-analysis module, so that the problems they report never drift apart.

## Implementation Decisions

**Batch analysis**
- **A shared core module.** Today the web engine's `analyse` and `fix_all`
  build a `Workspace` from a list of files and collect diagnostics, SHACL
  results, fixes and formatted text. That logic moves into the core crate.
  The web engine and the CLI both call it.
- **Its interface:** a set of documents (path and text), plus "context only"
  documents. It returns, per document:
  - the diagnostics with their fixes (each marked preferred or not);
  - the formatted text;
  - for `--fix`, a fix-all pass.
- **No LSP plumbing.** It doesn't depend on the server's event loop. SHACL
  runs synchronously, not on the server's worker thread.

**Workspace**
- **The root:** `--root` if given, otherwise the nearest ancestor with `.git`,
  otherwise the current directory.
- **What gets loaded:** every RDF and SPARQL file under the root, as the
  server indexes it at startup.
- **What gets reported:** only files under PATHS (default: the root). The
  other files are context only.
- **Imports:** come from the existing import cache. A missing import is
  reported as it is in the editor. `--network` allows fetching, following
  `imports.network`.

**Severity, exit codes and positions**
- **Severities** are the LSP ones: error, warning, info, hint.
- **Exit 1:** any error, or more warnings than `--max-warnings`. Syntax errors
  and `sh:Violation` are errors. `sh:Warning` and `sh:Info` keep their
  severities.
- **Exit 2:** a bad flag, an unreadable path or an internal failure. The
  message goes to stderr.
- **`--exit-zero`** always exits 0 when the run completes.
- **Positions:** lines and columns start at 1 and count Unicode code points.
  That matches what ruff and GitHub annotations expect. The LSP's UTF-16
  stays for the server only.

**Output**
- **`concise`:** `path:line:col: severity[code] message`, sorted by path, then
  position, then code. The summary goes to stderr ("3 errors, 1 warning in 2
  files"), so stdout is only problems.
- **`json`:** a versioned object, `{"version": 1, "files": [{"path",
  "diagnostics": [{"range", "severity", "code", "message", "fixes": [{"title",
  "safe", "edits"}]}]}], "summary": {...}}`. It reuses the shape the web
  engine already returns. Keys are only added, never renamed, within a
  version.
- **`github`:** `::error file=…,line=…,col=…,endLine=…,endColumn=…,title=…::message`.
  Info and hints map to `notice`. Because GitHub keeps only 10 annotations of
  each severity per step, the README recommends `sarif` for large
  repositories.
- **`sarif`:** SARIF 2.1.0, with:
  - one run, and the tool driver `turtle-lsp` at the server's version;
  - a rule per diagnostic code, with its help text;
  - `fixes` from the safe fixes;
  - artifact URIs relative to the root, with `%SRCROOT%`.
- **`--output-file`** writes the report to a file instead of stdout.
- **Messages** keep their current wording, which already includes "did you
  mean" and the prefix declaration to add.

**Fixes**
- **Safe** fixes are the preferred quick fixes, except term renames. Unsafe
  fixes are the "did you mean" renames of vocabulary terms, which may change
  meaning.
- **`--fix`** applies the safe ones; **`--unsafe-fixes`** adds the unsafe
  ones.
- **The fix-all loop is the web engine's** (rounds until stable, edits never
  overlap). Afterwards the check reports what is left.

**Format**
- **Same formatter** as the editor, at the line width from settings.
- **Skipped files:** a file with syntax errors is left untouched and reported
  (exit 2 with `--check`, since it cannot be judged).
- **Exit codes with `--check` and `--diff`:** 1 if any file would change.

**Selection**
- **Filters:** `--select CODE,...` and `--ignore CODE,...` filter by
  diagnostic code. SHACL results use the codes they already carry.
- **`--no-shacl`** skips validation.
- **`--max-diagnostics N`** caps output per file, so a broken generated file
  doesn't flood an agent's context. The summary still counts everything.
- **`--statistics`** prints counts per code instead of problems.

**Settings file**
- **An optional `turtle-lsp.json`** at the workspace root, with the same keys
  as the editor's `turtle` settings, nested or dotted, through `Settings::apply`.
- **Who reads it:** the CLI, and the server as a layer under the client's
  settings. A missing or invalid file falls back to defaults (invalid gives
  one warning).
- **Flags win** over the file.
- **Open:** the file name (`turtle-lsp.json` vs `.turtle-lsp.json`). The
  first is easier to find.

**Argument parsing**
- **Hand-written** over `std::env::args`, the way `--version` is handled now.
  Only add a dependency (lexopt-sized, not clap) if that gets unwieldy, to
  keep the binary and build time small.

**Performance**
- **One process** per run. Files are parsed in parallel with rayon, as the
  server's startup indexing does. SHACL builds the data graph once.
- **No result cache** for now.
- **Pre-commit:** the hook sets `require_serial: true`, so pre-commit does not
  start several processes that each load the whole workspace.

**Distribution and docs**
- **The release archives** (already published per platform) contain the CLI.
  No new assets are needed.
- **New in the repository:**
  - `.pre-commit-hooks.yaml`, with hooks `turtle-lsp-check` and
    `turtle-lsp-format`;
  - README sections: "Check from the command line", "For coding agents" (an
    AGENTS.md / CLAUDE.md snippet and a Claude Code `PostToolUse` hook) and
    "In CI" (a GitHub workflow with checksum-verified download and a SARIF
    upload).
- **A composite GitHub Action** comes later.

**Release**
- **Version 0.6.0,** since this is a new feature. The CHANGELOG lists both
  commands and the settings file.

## Testing Decisions

**What a good test is.** It runs the real binary on a temporary folder and
asserts only what a user sees: stdout, stderr, the exit code and file
contents. It never checks internals. This is the seam `test/nvim-plugin.test.mjs`
and the LSP tests already use: a process, its input and its output.

**What gets tested**
- **One test file,** `test/cli.test.mjs`, under `npm test`. It uses fixture
  folders written by the test, with the shared prefix header from the
  real-world corpus where useful.
- **Check:**
  - a clean folder exits 0 with no output;
  - a syntax error exits 1, at the right line and column;
  - an undeclared prefix suggests the declaration;
  - a SHACL violation is reported at the triple's line;
  - checking one file still uses a sibling prefixes.ttl and shapes file;
  - `--max-warnings` and `--exit-zero` behave as described;
  - a bad flag exits 2.
- **Output formats:**
  - JSON parses against version 1;
  - the GitHub lines match the workflow-command grammar;
  - SARIF validates against the 2.1.0 schema. The schema is vendored in the
    test fixtures; there is no network in tests.
- **Fixes and format:**
  - `--fix` adds a missing prefix and leaves a "did you mean" alone;
    `--unsafe-fixes` applies it too;
  - `format --check` exits 1 on an unformatted file and changes nothing;
  - `format` writes the file and is idempotent.
- **Code points:** columns are correct on a line with non-ASCII characters
  (code points, not UTF-16 or bytes).

**The shared module and the corpus**
- **Toolbox parity.** The web engine's existing tests (`npm run web:test`)
  must still pass unchanged after `analyse` and `fix_all` move into the core.
- **The real-world corpus:** `check` on the ../ontology-design fragments
  matches the editor's diagnostics for the same files.

**Performance**
- **A benchmark** in `npm run bench`: checking the 30,000-line file and a
  whole-corpus run, with a number recorded in the README.
- **Time limits:** each test run stays under 2 minutes and leaves no temp
  folders behind.

## Out of Scope

- **An MCP server.** Shell-based agents get the same results from the CLI.
  Revisit if users ask, or if there is first-party evidence that agents do
  better with MCP tools than with a CLI.
- **Query-related commands:** running SPARQL queries, reasoning, or anything
  the editor doesn't already report.
- **More output formats:** GitLab Code Quality and JUnit. They are cheap to
  add later, from the same internal model.
- **Caching:** a result cache, or a watch mode.
- **Other packaging:** a composite GitHub Action, Docker images, and
  publishing to package managers (Homebrew, crates.io, npm).
- **New file types:** checking JSON-LD or RDF/XML files.

## Further Notes

- **The Reddit reply.** It promised a "check" command in the next release,
  so this is the natural headline for 0.6.0. A short follow-up post could show
  an agent fixing its own Turtle with it.
- **Claude Code.** Its LSP plugins already give Claude diagnostics after each
  edit, on machines where plugins run. The CLI covers cloud sessions, other
  agents, CI and pre-commit. A Claude Code plugin pointing at `turtle-lsp`
  is a small, separate follow-up.
- **No other RDF tool found with these outputs.** The research found none
  with file:line:col SHACL results, SARIF or GitHub annotations. Say "the
  ones I found", not "the only one", in posts.
