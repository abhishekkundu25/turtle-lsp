# 04: Headless web test in CI

Status: todo. Blocked by: 03.

## What to build

A headless test of the browser entry in VS Code for the Web (spec:
"Testing Decisions").

- `@vscode/test-web` as a devDependency (exact version, lockfile
  updated), and a `test:web` script.
- `test/web/`: the runner (Chromium, headless, the fixture folder mounted)
  and the suite bundled for the web extension host.
- `test/fixtures/web/`: several files with neutral names (`ex:`,
  `http://example.org/`): a `prefixes.ttl`, a headerless file using its
  prefixes, a file that defines a class another file uses, a file with an
  error that is never opened, a SPARQL file.
- The suite checks: activation; diagnostics on an open file within 10 s;
  hover; formatting; a definition in another file; `prefixes.ttl` honoured
  (no undeclared-prefix error); the unopened file's problem listed;
  references still found after closing a file; no Run query lens.
- `.github/workflows/ci.yml`: the Browser job, which builds the engine,
  installs `editors/vscode` and runs `test:web`.
- Leave no browser or server process running; the runner's downloads stay
  under `editors/vscode/.vscode-test-web` (gitignored).

## Acceptance

- [ ] `timeout 120 npm run test:web` passes locally, three runs in a row.
- [ ] The desktop tests still pass.
- [ ] `ci.yml` parses (`actionlint` if present, else a YAML parse).
