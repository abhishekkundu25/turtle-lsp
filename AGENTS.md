# AGENTS.md

turtle-lsp is a Rust language server and command-line checker for RDF and
SPARQL, with a VS Code extension, a Neovim plugin and a browser build.
[ARCHITECTURE.md](ARCHITECTURE.md) maps the modules and the rules between
them; each file's `//!` doc says what it owns.

## Build and test

Build and test in release mode only: timing tests and budgets mean nothing
in a debug build. The Node tests run the binary in `target/release`, so
build it first.

```sh
cargo build --release --locked      # about a minute (whole-program optimisation)
cargo fmt --check
cargo clippy --release --locked --all-targets -- -D warnings
```

Keep every test run under two minutes: wrap it in `timeout 120`, and run one
file at a time when a whole suite takes longer. Compiling is not a test run.

```sh
timeout 120 cargo test --release --locked --lib            # unit tests
timeout 120 cargo test --release --locked --test chaos     # one integration test file
timeout 120 node --test --test-timeout=30000 test/cli.test.mjs
timeout 120 npm run test:latency                           # wall-clock checks, one at a time
node web/build.mjs && timeout 120 node --test --test-timeout=60000 web/test/engine.test.mjs
(cd editors/vscode && timeout 120 node --test test/manifest.test.mjs)
```

- Leave `TMPDIR` at its default, outside the repository. The tests make
  their workspaces in the system temp folder and run `check --fix` there
  without `--root`, so the root is the nearest folder with a `.git`: under
  the repository, that is the repository, and the fixes rewrite tracked
  fixtures.
- A test that changes files works on a copy in a temp folder;
  `test/fixtures/` stays as committed. Fixtures use neutral names
  (`ex:`, `http://example.org/`): no employer or private project.
  `test/fixtures/real/` is private and gitignored; tests skip it when it is
  absent.
- The corpus tests read `test/corpus/` (downloaded by `npm run corpus`,
  gitignored). Without it they print `skipped`; with `CI=true` they fail.
- Nothing uses the network by default, and no test reaches the internet. A
  test that fetches serves the document from a local listener; the servers
  that `test/lsp-client.mjs` starts get `TURTLE_LSP_ALLOW_PRIVATE_HOSTS=1`
  (loopback is refused otherwise) and a private `TURTLE_LSP_CACHE_DIR`, so
  no test writes the user's cache. Start servers through it.
- A pending (`todo`) test that starts passing fails `npm run test:node`,
  which names it; promote it by removing its `todo`.

## Release

1. Set one version in `Cargo.toml`, `Cargo.lock`,
   `editors/vscode/package.json`, `editors/vscode/package-lock.json` (two
   fields) and the top heading of `CHANGELOG.md`: rename `## Unreleased` to
   `## X.Y.Z — YYYY-MM-DD`. `web/engine/Cargo.toml` stays `0.0.0`.
   `RELEASE_TAG=vX.Y.Z node --test test/versions.test.mjs` checks all of
   them.
2. When `rust-toolchain.toml` moves to a new compiler, run clippy on it;
   lower `rust-version` in `Cargo.toml` only after clippy passes on the
   lower number (its comment says why it is 1.91).
3. When spareval moves to a new release, re-apply the patches in
   `vendor/spareval` (`vendor/spareval/PATCH.md`), update `VERSION` and
   `SHA256` in `tools/check-vendor.sh`, keep the exact `=` pin in
   `Cargo.toml` (the `[patch.crates-io]` entry applies only while it
   matches), and run `tools/check-vendor.sh`.
4. Push the tag `vX.Y.Z` on a commit of `main` whose CI passed. The release
   workflow publishes to the Marketplace from the GitHub environment
   `release`, which must hold the `VSCE_PAT` secret.
