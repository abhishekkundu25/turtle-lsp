# Command-line check and format: conventions and recommendations

Date: 2026-10-20. Purpose: decide the shape of `turtle-lsp check` and
`turtle-lsp format`, so coding agents and CI can validate RDF without an LSP
client. Today `src/main.rs` either prints `--version` or runs the LSP server.

Method: official docs, specs and source code, fetched on the date above. Claims
about the behaviour of another tool's source are from its default branch on that
date. Where only secondary sources were found, the text says so.

## 1. How established linters and formatters behave

### Exit codes

Every tool surveyed separates "problems found" from "the tool failed", and most
use the same three codes.

| Tool | 0 | 1 | 2 | Source |
|---|---|---|---|---|
| `ruff check` | no violations, or all fixed | violations found | "invalid configuration, invalid CLI options, or an internal error" | [ruff linter](https://docs.astral.sh/ruff/linter/) |
| `ruff format --check` | nothing would change | a file would be reformatted | same as above | [ruff formatter](https://docs.astral.sh/ruff/formatter/) |
| ESLint | no errors, and warnings at most `--max-warnings` | at least one error, or more warnings than allowed | "configuration problem or an internal error" | [ESLint CLI](https://eslint.org/docs/latest/use/command-line-interface) |
| Prettier `--check` | "Everything formatted properly" | "Something wasn't formatted properly" | "Something's wrong with Prettier" | [Prettier CLI](https://prettier.io/docs/cli) |
| rustfmt `--check` | no changes needed | changes needed | (not split out; without `--check`, 1 means an error) | [rustfmt README](https://github.com/rust-lang/rustfmt) |
| Biome | not documented as a table; `--error-on-warnings` "Exits with an error status if any warning diagnostics are emitted" | | | [Biome CLI](https://biomejs.dev/reference/cli/) |

Details worth copying:

- ESLint exits 0 when there are only warnings, unless `--max-warnings n` is
  set ([ESLint CLI](https://eslint.org/docs/latest/use/command-line-interface)).
  Biome does the same with `--error-on-warnings`.
- ruff `--exit-zero` forces 0 for violations "but will still exit with a status
  code of `2` if it terminates abnormally". `--exit-non-zero-on-fix` returns 1
  even when every violation was fixed
  ([ruff linter](https://docs.astral.sh/ruff/linter/)).

### Fixes

ruff `--fix` applies safe fixes only; unsafe ones (which can change behaviour or
drop comments) need `--unsafe-fixes`, and JSON output lists every fix with an
`applicability` field ([ruff linter](https://docs.astral.sh/ruff/linter/)).
`ruff check --diff` prints fixes instead of writing them; `ruff format --diff`
prints the formatting diff and exits non-zero if there is one (`ruff --help` on
[ruff configuration](https://docs.astral.sh/ruff/configuration/)). ESLint has
`--fix-dry-run`; Biome has `check --write` / `--unsafe`, and `biome ci` is the
same check with no write flags ([Biome CLI](https://biomejs.dev/reference/cli/)).

### Output formats

| Format | ruff `--output-format` | Biome `--reporter` | ESLint `-f` (built in) |
|---|---|---|---|
| Rich terminal (snippet, arrows) | `full` (default) | `default` | `stylish` (default) |
| One line per problem | `concise`, `pylint` | `concise` | – |
| Grouped / summary | `grouped`, `--statistics` | `summary` | – |
| JSON | `json`, `json-lines` | `json` ("experimental and may change in patch releases"), `json-pretty` | `json`, `json-with-metadata` |
| GitHub annotations | `github` | `github` | – |
| GitLab Code Quality | `gitlab` | `gitlab` | – |
| SARIF | `sarif` | `sarif` | – (not on the built-in list) |
| JUnit XML | `junit` | `junit` | – |
| Others | `azure`, `rdjson` | `checkstyle`, `rdjson` | `html` |

Sources: `ruff check --help` on [ruff configuration](https://docs.astral.sh/ruff/configuration/)
(the [settings page](https://docs.astral.sh/ruff/settings/) lists a shorter set
without `sarif`, `json-lines` and `rdjson`), [Biome reporters](https://biomejs.dev/reference/reporters/),
[ESLint formatters](https://eslint.org/docs/latest/use/formatters/).
ruff's `--statistics` "Show[s] counts for every rule with at least one
violation". Biome limits the diagnostics it prints with `--max-diagnostics`
(default 20, `none` for no limit).

The one-line format follows the GNU convention `sourcefile:lineno:column:
message`, with lines and columns starting at 1
([GNU Coding Standards](https://www.gnu.org/prep/standards/html_node/Errors.html)).
Tools that print color should turn it off when `NO_COLOR` is set and not empty
([no-color.org](https://no-color.org/)).

Other CI conveniences: `-o/--output-file` (ruff, ESLint), changed-files-only
(Biome `--changed`, `--staged`), result caches (ESLint and Prettier `--cache`)
and a thread count (`biome ci --threads`).

## 2. Output that works for coding agents

### What the first-party sources say

- Claude Code best practices: "CLI tools are the most context-efficient way to
  interact with external services", and Claude "is also effective at learning
  CLI tools it doesn't already know" from `--help`
  ([best practices](https://code.claude.com/docs/en/best-practices)).
- Anthropic's guide to writing tools for agents: return "only high signal
  information"; offer a concise and a detailed form (the concise form used
  "~⅓ of the tokens" in their example); paginate or truncate "with sensible
  default parameter values" and, when truncating, "steer agents with helpful
  instructions"; make errors "specific and actionable improvements, rather than
  opaque error codes or tracebacks"
  ([Writing tools for agents](https://www.anthropic.com/engineering/writing-tools-for-agents)).
- Claude Code shows a Bash result inline up to about 30,000 characters; beyond
  that Claude gets a file path and a 2,000-character preview
  ([tools reference](https://code.claude.com/docs/en/tools-reference)). Hook
  output is capped at 10,000 characters per field, with the same file fallback
  ([hooks](https://code.claude.com/docs/en/hooks)).
- AGENTS.md (used by Codex, Cursor, Jules, Amp, Factory; stewarded by the
  Agentic AI Foundation) is where projects tell agents which lint and test
  commands to run; its example says "Always run `pnpm lint` and `pnpm test`
  before committing" ([agents.md](https://agents.md/)). Codex concatenates
  AGENTS.md files from the repository root down to the working directory and
  stops at 32 KiB by default
  ([Codex AGENTS.md](https://learn.chatgpt.com/docs/agent-configuration/agents-md)).

### What follows for turtle-lsp

Derived from the above, not from a single source:

1. One problem per line, with path, line, column, severity, rule code and
   message. An agent can jump to it with its file tools and never needs a
   snippet it can read itself.
2. A stable, versioned JSON format that includes fixes as text edits, so an
   agent (or a script) can apply them without re-deriving them.
3. A cap on printed problems with a closing line that says how many were left
   out and how to see them (`--max-diagnostics`), to stay inside the 10,000 and
   30,000 character limits above.
4. Fix hints in the message itself ("did you mean `rdfs:label`?",
   "declare `foaf:`; run with `--fix`").
5. Quiet on success: one summary line, or nothing with `--quiet`.
6. No color unless stdout is a terminal and `NO_COLOR` is unset.

### LSP plugin, hook, or MCP?

- **Claude Code already consumes LSP.** A code intelligence plugin gives Claude
  "the errors and warnings the server reports" after each edit, and plugins
  declare servers in `.lsp.json`
  ([code intelligence](https://code.claude.com/docs/en/plugins/code-intelligence),
  [plugin components](https://code.claude.com/docs/en/plugins/components)).
  But "in cloud sessions, Claude Code doesn't start plugin language servers"
  (same page). A CLI covers cloud sessions, CI, other agents and pre-commit.
- **Hooks.** A `PostToolUse` hook with matcher `Edit|Write` is the documented
  way to run a linter after edits. For `PostToolUse`, exit code 2 "Shows stderr
  to Claude; the tool already ran"; stderr from a hook that exits 0 is never
  shown to Claude ([hooks](https://code.claude.com/docs/en/hooks),
  [hooks guide](https://code.claude.com/docs/en/hooks-guide)).
- **MCP.** The MCP spec offers typed tool results (`structuredContent`,
  `outputSchema`) and separates tool execution errors (`isError: true`, meant
  for the model to self-correct) from protocol errors
  ([MCP tools spec](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)).
  Anthropic's code-execution article names two costs of MCP: "Tool definitions
  overload the context window" and "Intermediate tool results consume
  additional tokens"
  ([Code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp)).
  rudof ships an MCP server (`rudof mcp`, in its
  [README](https://github.com/rudof-project/rudof)).

Evidence is thin on a direct comparison: no first-party source compares an MCP
linter with a CLI linter. The best-practices line above is about external
services, not linters. Blog benchmarks exist but are third-party and predate
Claude Code's deferred tool loading, so they are not used here. Conclusion: ship
the CLI first; the LSP plugin covers interactive Claude Code sessions; an MCP
server adds little until someone needs typed results inside a non-coding agent.

## 3. CI formats

### GitHub workflow commands (annotations)

`::error file={name},line={line},endLine={endLine},col={col},endColumn={endColumn},title={title}::{message}`,
likewise `::warning` and `::notice`; line and column start at 1
([workflow commands](https://docs.github.com/en/actions/reference/workflows-and-actions/workflow-commands)).
Escaping, from the toolkit source: in the message, `%`→`%25`, CR→`%0D`,
LF→`%0A`; in properties additionally `:`→`%3A` and `,`→`%2C`
([actions/toolkit command.ts](https://github.com/actions/toolkit/blob/main/packages/core/src/command.ts)).
Limits: 10 error, 10 warning and 10 notice annotations per step, 50 per job;
the rest are dropped
([toolkit problem-matchers](https://github.com/actions/toolkit/blob/main/docs/problem-matchers.md)).
So the `github` format suits small diffs; SARIF suits whole repositories.
`GITHUB_ACTIONS` is "Always set to true when GitHub Actions is running the
workflow" ([variables](https://docs.github.com/en/actions/reference/workflows-and-actions/variables)).

### SARIF 2.1.0 and code scanning

From the [OASIS SARIF 2.1.0 spec](https://docs.oasis-open.org/sarif/sarif/v2.1.0/os/sarif-v2.1.0-os.html):

- A log has `version: "2.1.0"`, `$schema`, and `runs[]`; each run has
  `tool.driver` (`name`, `version`, `rules[]`) and `results[]`.
- A result has `ruleId`, `level` (`error`, `warning`, `note`, `none`),
  `message.text`, and `locations[].physicalLocation` with
  `artifactLocation.uri` (relative URIs SHOULD carry a `uriBaseId`) and
  `region` (`startLine`, `startColumn`; `endColumn` is one past the last
  character).
- `run.columnKind` is `utf16CodeUnits` or `unicodeCodePoints`; consumers
  recompute columns if they use another unit.
- A result can carry `fixes[]`: `artifactChanges[]` of `replacements` with a
  `deletedRegion` and `insertedContent`.

GitHub code scanning
([SARIF support](https://docs.github.com/en/code-security/code-scanning/integrating-with-code-scanning/sarif-support-for-code-scanning)):
only 2.1.0; up to 10 MB gzip-compressed per file, 20 runs per file, 25,000
results per run (only the top 5,000 shown); `defaultConfiguration.level` is
`note`, `warning` or `error`; `partialFingerprints.primaryLocationLineHash` is
the only fingerprint used, and `upload-sarif` fills it in when missing. Upload
needs `security-events: write` and `github/codeql-action/upload-sarif@v4` with
`sarif_file` and an optional `category`
([uploading SARIF](https://docs.github.com/en/code-security/code-scanning/integrating-with-code-scanning/uploading-a-sarif-file-to-github)).

### GitLab Code Quality

A JSON array; each object needs `description`, `check_name`, `fingerprint`,
`location.path` (relative, no `./`), `location.lines.begin`, and `severity`
(`info`, `minor`, `major`, `critical`, `blocker`); no byte order mark
([GitLab Code Quality](https://docs.gitlab.com/ci/testing/code_quality/)).

### pre-commit

A hook repository has `.pre-commit-hooks.yaml` with `id`, `name`, `entry`,
`language`; useful options are `files`, `types`/`types_or`, `pass_filenames`,
`require_serial` and `args`. "The hook must exit nonzero on failure or modify
files." `language: rust` installs the repository with `cargo install --bins`
(bootstrapping Rust if needed); a prebuilt binary on `PATH` uses
`language: unsupported` (formerly `system`) ([pre-commit](https://pre-commit.com/)).

## 4. Existing RDF and SHACL command-line validators

| Tool | Checks | Output | Exit codes | Positions |
|---|---|---|---|---|
| pySHACL `pyshacl` | SHACL (Core, Advanced, JS) | `-f human` (default), `table`, `turtle`, `xml`, `json-ld`, `nt`, `n3` | 0 conforms, 1 non-conformant, 2 runtime error, 3 SHACL feature not implemented ([README](https://github.com/RDFLib/pySHACL)) | None: the report is a graph of focus nodes and paths |
| Jena `shacl validate` | SHACL Core and SPARQL ([docs](https://jena.apache.org/documentation/shacl/)) | Turtle report, or `--text` | The command prints the report and returns; the source has no exit on non-conformance ([shacl_validate.java](https://github.com/apache/jena/blob/main/jena-cmds/src/main/java/shacl/shacl_validate.java)) | None |
| Jena `riot --validate` | Syntax, plus literal and IRI checks ("same as --strict --sink --check=true", [Jena I/O](https://jena.apache.org/documentation/io/)) | Log lines `[line: N, col: M ] message` ([SysRIOT.java](https://github.com/apache/jena/blob/main/jena-arq/src/main/java/org/apache/jena/riot/SysRIOT.java)) | 1 if any file had errors ([CmdLangParse.java](https://github.com/apache/jena/blob/main/jena-cmds/src/main/java/riotcmd/CmdLangParse.java)) | Line and column, but the standard error handler throws on an error, so parsing a file stops at the first one ([ErrorHandlerFactory.java](https://github.com/apache/jena/blob/main/jena-arq/src/main/java/org/apache/jena/riot/system/ErrorHandlerFactory.java)) |
| rudof `shacl-validate` (Rust) | SHACL; also ShEx, PG-Schema | `--result-format` `details` (default), `minimal`, `compact`, `json`, `csv`, and RDF syntaxes ([source](https://github.com/rudof-project/rudof/blob/master/rudof_cli/src/cli/wrappers/validation.rs)) | Command returns `Ok` after printing the report whatever the result ([shacl_validate.rs](https://github.com/rudof-project/rudof/blob/master/rudof_cli/src/commands/shacl_validate.rs)), so non-conforming data exits 0 | None |
| TopBraid `shaclvalidate.sh` | SHACL, owl:imports (`-noImports` to skip) | Turtle (`-outputFormat ttl` or `jelly`) | 1 when `sh:conforms false` ([Validate.java](https://github.com/TopQuadrant/shacl/blob/master/src/main/java/org/topbraid/shacl/tools/Validate.java)) | None |
| rdflint (Java, 0.2.1, May 2022) | Syntax, undefined subjects, SPARQL rules, SHACL, encoding | File name, then indented messages with `line: N` (per its VS Code problem matcher, [integration](https://imas.github.io/rdflint/integration/)) | 1 for problems at or above `minErrorLevel`, and also 1 for an unknown option ([RdfLint.java](https://github.com/imas/rdflint/blob/master/src/main/java/com/github/imas/rdflint/RdfLint.java)) | Line only; GitHub Action `imas/setup-rdflint` |
| IDLab `turtle-validator` (`ttl`, npm 1.1.1, June 2020) | Turtle/N-Triples syntax, XSD regex checks | `ERROR: …` / `WARNING: …` lines and a count | 2 on errors, 0 otherwise; the process exits in the first file's callback ([TurtleValidator.js](https://github.com/IDLabResearch/TurtleValidator/blob/master/TurtleValidator.js)) | Whatever N3.js puts in the message |

None of these publishes a speed benchmark for its command line that this survey
found. A web search found no RDF or SHACL tool with SARIF, GitHub annotation or
GitLab output (not exhaustive).

Gaps turtle-lsp can fill:

1. **SHACL results with file:line:col.** Every SHACL tool above reports focus
   node and path only. turtle-lsp already maps each result to the offending
   triple.
2. **All syntax errors per file, not the first** (riot and N3.js stop).
3. **Exit codes that mean something.** Jena `shacl validate` and rudof exit 0
   on violations; rdflint uses 1 for both violations and bad options. None
   separates "problems" from "tool error" the way ruff and ESLint do.
4. **CI-native formats**: SARIF, GitHub annotations, GitLab Code Quality.
5. **One binary for syntax, vocabulary lints, SHACL, SPARQL and formatting**,
   with no JVM or Python, and with `prefixes.ttl` support that no other tool has.
6. **Fixes** (`--fix`), which none of them offers.

## 5. Recommendations

### Command shape

```
turtle-lsp check  [PATHS...] [options]   # default PATHS: .
turtle-lsp format [PATHS...] [--check | --diff]
turtle-lsp        # no subcommand: LSP server over stdio, as today
turtle-lsp --version
```

- Keep the bare binary as the LSP server, so no editor configuration breaks.
  `turtle-lsp server` can be an explicit alias.
- PATHS are files or directories, walked with the server's existing rules
  (`SKIP_DIRS`, `MAX_FILES`, `MAX_FILE_BYTES` in `src/workspace.rs`); `-` reads
  stdin, with `--stdin-filename` naming the path used for syntax detection,
  prefixes and messages.
- **Paths choose what is reported; the root chooses the context.** Load the
  whole workspace root (prefixes, shapes, vocabularies, cached imports) as the
  server does, but print problems only for the given paths. This keeps
  `check one-file.ttl` and the editor in agreement, since SHACL validates the
  union of the workspace's Turtle files.
- Root: `--root DIR`, otherwise the nearest ancestor with `.git`, otherwise the
  current directory.

### `check` options

| Option | Behaviour | Precedent |
|---|---|---|
| `--output-format F` | `concise` (default), `full`, `json`, `github`, `sarif`; later `gitlab`, `junit` | ruff, Biome |
| `--output-file PATH` | Write the report there; a one-line summary still goes to stderr | ruff, ESLint |
| `--fix` | Apply safe fixes (declare a known prefix, remove an unused prefix) | ruff |
| `--unsafe-fixes` | Also apply "did you mean" renames, which change the graph | ruff |
| `--diff` | Print fixes as a unified diff, write nothing | ruff, ESLint `--fix-dry-run` |
| `--max-warnings N` | Exit 1 when warnings exceed N | ESLint |
| `--max-diagnostics N` | Print at most N (default: no limit); the summary says how many were left out | Biome |
| `--exit-zero` | Report but exit 0 (2 still means failure) | ruff |
| `--select` / `--ignore CODE,...` | Filter by rule code (`shacl-violation`, `undescribed-term`, …) | ruff |
| `--no-shacl`, `--shacl-max-triples N` | Mirror `turtle.shacl.enabled` / `maxTriples` | settings.rs |
| `--network` | Allow fetching `owl:imports`; off by default, like `turtle.imports.network` | settings.rs |
| `--statistics` | Counts per rule code | ruff |
| `--quiet` | Nothing on success | ESLint |

### Exit codes

| Code | Meaning |
|---|---|
| 0 | No errors (and warnings within `--max-warnings`), or `--exit-zero` |
| 1 | At least one error-severity problem, or too many warnings; for `format --check`, a file would change |
| 2 | Tool failure: bad arguments, unreadable path, invalid configuration, internal error |

Severity: syntax errors and SHACL `sh:Violation` are errors; `sh:Warning`,
vocabulary lints and unresolved imports are warnings; `sh:Info` and hints are
notes and never fail the run. SHACL skipped for size (`shacl-skipped`) is a
warning, so CI does not pass silently on an unvalidated workspace. Document this
table in `--help`.

### Default output

```
shapes/person.ttl:12:5: error[shacl-violation] Value "forty" is not a xsd:integer (ex:PersonShape, ex:age)
data/people.ttl:3:1: warning[undescribed-term] ex:lable is described nowhere; did you mean ex:label? (fix: --unsafe-fixes)
Found 2 problems (1 error, 1 warning) in 2 of 214 files; 1 fixable with --unsafe-fixes.
```

- Paths relative to the current directory; line and column 1-based; columns in
  Unicode code points (the server works in UTF-16 for LSP; convert, and set
  SARIF `columnKind` to `unicodeCodePoints`).
- One line per problem, sorted by path then position, so output is stable
  between runs and diffs well.
- `full` adds the source line and a caret, for people.
- `json`: an object with `version: 1`, a `summary`, and `diagnostics[]` of
  `{path, range{start{line,column},end}, severity, code, message, related[],
  fix{applicability, edits[]}}`. Document it as stable, unlike Biome's
  experimental JSON.

### Formats to build first

1. `concise` (default; agents, terminals, pre-commit).
2. `json` (scripts, agents that want fixes).
3. `github` (pull-request annotations, within the 10-per-step limit).
4. `sarif` (code scanning, with `rules[]` from the rule codes and `fixes[]`).

`gitlab` and `junit` are cheap once these exist. Skip `rdjson`, `checkstyle`,
`azure` until someone asks.

### Configuration

Today settings arrive only from the LSP client (`src/settings.rs`). The CLI
needs the same settings from disk, or the editor and CI will disagree:

- Read an optional `turtle-lsp.json` (or a `turtle` key in it) at the root,
  with exactly the keys of `initializationOptions`, passed through
  `Settings::apply`. The server should read the same file as its first layer,
  under the client's settings.
- `prefixes.ttl` discovery stays as it is (nearest file up to the root).
- Command-line flags override the file.

### Performance

- Parse and analyse files in parallel with rayon, as workspace indexing already
  does; build the SHACL graph once per dataset, not per file.
- One process per run. Tell pre-commit `require_serial: true`, or it may split
  the file list over several processes, each loading the whole workspace.
- Offline by default: imports come from the cache (`TURTLE_LSP_CACHE_DIR`, see
  `imports::cache_dir`); `--network` fetches. CI can cache that directory.
- No result cache at first. The server formats a 30,000-line file in 5 ms and
  validates about 180k triples in 13 ms (README); measure a large corpus before
  adding ESLint-style caching.
- Add the CLI to `npm run bench` with a check of the 200-file benchmark
  workspace, and keep every test run under the two-minute cap.

### `format`

- Writes in place by default; `--check` exits 1 and lists the files; `--diff`
  prints a unified diff and exits 1 if there is one (ruff, Prettier, rustfmt).
- `-` formats stdin to stdout.
- `--line-width` mirrors `turtle.format.lineWidth`.
- A file that does not parse is reported as an error (exit 2 is wrong here; it
  is a problem in the input, so exit 1) and left untouched.

## 6. Documentation snippets to ship

README:

````markdown
## Command line

```sh
turtle-lsp check                      # syntax, prefixes, vocabularies, SHACL
turtle-lsp check --fix data/          # also apply safe fixes
turtle-lsp format --check             # exit 1 if any file would be reformatted
```

Exit codes: 0 clean, 1 problems found, 2 tool error.
````

AGENTS.md / CLAUDE.md:

```markdown
- After editing .ttl, .trig, .nt, .nq, .rq or .ru files, run
  `turtle-lsp check <files>` and fix every error before finishing.
  Output is `path:line:col: severity[code] message`. Exit 1 means problems,
  exit 2 means the command itself failed.
- Run `turtle-lsp format <files>` on the files you changed.
```

Claude Code hook (`.claude/settings.json`); exit 2 so Claude sees the problems:

```json
{
  "hooks": {
    "PostToolUse": [{
      "matcher": "Edit|Write",
      "hooks": [{
        "type": "command",
        "command": "f=$(jq -r '.tool_input.file_path'); case \"$f\" in *.ttl|*.trig|*.nt|*.nq|*.rq|*.ru|*.sparql) turtle-lsp check --max-diagnostics 30 \"$f\" 1>&2 || exit 2;; esac"
      }]
    }]
  }
}
```

Users with the code intelligence plugin already get diagnostics after edits;
the hook is for cloud sessions and for teams that want the check enforced.

GitHub Actions:

```yaml
jobs:
  rdf:
    runs-on: ubuntu-latest
    permissions: { contents: read, security-events: write }
    steps:
      - uses: actions/checkout@v6
      - run: |
          curl -sL https://github.com/abhishekkundu25/turtle-lsp/releases/latest/download/turtle-lsp-linux-x64.tar.gz | tar xz
          ./turtle-lsp check --output-format sarif --output-file rdf.sarif
      - uses: github/codeql-action/upload-sarif@v4
        if: always()
        with: { sarif_file: rdf.sarif, category: turtle-lsp }
```

(`turtle-lsp-linux-x64.tar.gz` is a v0.5.2 release asset and holds the bare
binary. For annotations without code scanning, use `--output-format github`.) A small
composite action, `abhishekkundu25/turtle-lsp@v0`, that downloads the binary and
runs `check` would shorten this to one step.

pre-commit (`.pre-commit-hooks.yaml` at the repository root):

```yaml
- id: turtle-lsp-check
  name: turtle-lsp check
  entry: turtle-lsp check
  language: rust            # builds from source; `unsupported` uses a binary on PATH
  files: \.(ttl|trig|nt|nq|rq|ru|sparql)$
  require_serial: true
# turtle-lsp-format: the same with `entry: turtle-lsp format`
```

## Open questions

- Whether to detect `GITHUB_ACTIONS=true` and switch to the `github` format
  automatically. Proposed: no; keep output a function of the flags.
- The config file name. `turtle-lsp.json` is a proposal; check what the VS Code
  extension and Neovim plugin would need to read the same file.
