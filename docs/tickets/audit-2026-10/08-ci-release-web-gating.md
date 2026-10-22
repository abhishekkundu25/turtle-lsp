# 08: CI, release and web gating

**What to fix:** delivery is not gated on tests and the browser build has no CI.

Release (`.github/workflows/release.yml`):
- A `v*` tag push builds six targets, attests them, creates the GitHub Release and
  publishes to the Marketplace. No test runs, nothing waits for CI, and no shipped
  binary is ever executed; four targets (darwin-x64 cross, two musl, linux-arm64) are
  built only here.
- The publish step runs `npm ci` with install scripts (`esbuild`, `@vscode/vsce-sign`
  have them) while `VSCE_PAT` is in the step environment, in a job that also holds
  `id-token: write` and `attestations: write` (`:88-91`, `:136-142`). The build job also
  runs install scripts before producing the attested `.vsix` (`:48-55`). `VSCE_PAT` has
  no protected `environment`; `workflow_dispatch` on a tag ref runs the release.
- The tag is checked against `editors/vscode/package.json` only; `build.lua` downloads
  by `Cargo.toml`'s version (404 on drift). The publish loop is not idempotent.

CI (`ci.yml`, `pages.yml`, `dependabot.yml`):
- Nothing compiles `wasm32-unknown-unknown`, runs clippy on `web/engine`, or runs any of
  the 139 web tests (57 need no browser). `pages.yml` deploys every push to `main`
  without waiting for CI. `web/engine/Cargo.lock` is outside Dependabot and `cargo
  deny`; `npm audit` skips the root tree that ships Monaco to the site.
- The VS Code job never runs on macOS, has no `timeout-minutes`, and downloads
  `stable` VS Code every run. The chaos long run has no step timeout. No `concurrency`
  group, no npm cache, no failure artifacts. The vendored spareval is never checked
  against upstream + patch (`spareval = "0.2"` would resolve to 0.2.8 and break).

**Files owned:** `.github/workflows/release.yml`, `.github/workflows/ci.yml`,
`.github/workflows/pages.yml`, `.github/dependabot.yml`, new `test/versions.test.mjs`,
new `tools/check-vendor.sh`.
**Hands off:** everything else. `Cargo.toml` pins are ticket 23.

**Blocked by:** nothing. Cannot be fully verified locally: verify with `actionlint` if
installed, by running the new Node test, and by a dry read of the YAML; the first tag
after landing is the real test.

**Status:** ready-for-agent

- [ ] release: a smoke step on each native runner after `Package` (`turtle-lsp
      --version`, `check` on a fixture, an `initialize`/`shutdown` round trip via a tiny
      Node script); publish moved to its own job (`needs: release`, `permissions:
      contents: read`, no `id-token`), `npm ci --ignore-scripts`, `VSCE_PAT` only on the
      `vsce publish` command, `--skip-duplicate`, `environment: release`; the build job
      uses `npm ci --ignore-scripts` (check `npm run package` still works: esbuild uses
      `optionalDependencies`); publishing gated on a CI check for the tagged commit
      (`workflow_run` or `gh api` status check).
- [ ] Version check across `Cargo.toml`, `editors/vscode/package.json`, root
      `package.json` if versioned, `web/engine/Cargo.toml`, the top CHANGELOG heading;
      the same check as `test/versions.test.mjs` run by CI on every push.
- [ ] ci: a `web` job (`rustup target add wasm32-unknown-unknown`, `node web/build.mjs`,
      `cargo clippy --locked` in `web/engine`, `node --test
      web/test/{engine,host,jsonld-engine,lsp}.test.mjs`, one Playwright file on bundled
      Chromium via `npx playwright install chromium`); `cargo deny check --manifest-path
      web/engine/Cargo.toml` and `npm audit --omit=dev` on the root in the audit job;
      `macos-latest` in the vscode matrix with `timeout-minutes: 20` and one leg pinned to
      `VSCODE_VERSION=1.91.0`; `timeout-minutes: 5` on the chaos step; `concurrency` with
      `cancel-in-progress`; `cache: npm`; `upload-artifact` on failure for
      `target/tmp`; `tools/check-vendor.sh` (download spareval 0.2.7, apply the patch,
      diff against `vendor/spareval/src`) run in the audit job.
- [ ] pages: deploy only after the CI `web` job passes (`workflow_run` on CI success).
- [ ] dependabot: cargo entry for `/web/engine`.
- [ ] Every new `uses:` pinned to a full SHA with a version comment;
      `persist-credentials: false` on every checkout; `permissions` least privilege per
      job.
