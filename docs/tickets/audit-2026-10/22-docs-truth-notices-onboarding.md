# 22: Docs truth, third-party notices, onboarding

**What to fix:** the documentation makes claims the code does not keep, and the
repository has no in-repo map.

- `SECURITY.md:27-39`: "makes no network requests" is false (imports, JSON-LD contexts,
  the quick fix); "only reads Turtle files inside the workspace" is partly true (JSON-LD,
  SPARQL, RDF/XML, catalogs, settings file, the user cache); "each request is isolated"
  held only for handlers until ticket 13; "a randomized test feeds every feature" omits
  JSON-LD, SPARQL features, embedded SPARQL, lenses, query, the LSP layer;
  "cargo deny / npm audit on every push" omitted `web/engine` and the root npm tree until
  ticket 08. Scope omits the CLI, the browser build and `build.lua`. The extension's
  `untrustedWorkspaces.description` says "nothing is fetched over the network".
- 10 of 18 `docs/specs/*.md` status lines are stale (cli-check, jsonld, jsonld-phase2,
  lsp-capability, workspace-query shipped; the four web specs are merged; sota-roadmap
  points at a local-only folder). `docs/research/*.md` hold present-tense claims that are
  history now.
- No `ARCHITECTURE.md`, `AGENTS.md` or `CONTRIBUTING.md`; `CONTEXT.md` is local-only and
  frozen at 2026-10-13 (6 of 20 modules). Module docs are good (81 of 85 files).
- Release artefacts ship no third-party notices: the binary embeds webpki-roots
  (CDLA-Permissive-2.0 requires the text), the schema.org context (CC BY-SA 3.0),
  `data/vocab.tsv` (per-vocabulary licences); the `.vsix` bundles vscode-languageclient;
  the Pages site drops Monaco's `LICENSE`/`ThirdPartyNotices.txt`.
- README: "84 vocabularies" (83, 20,993 terms), examples pin `v0.6.0`, the CI example
  uses tag-pinned actions against the repo's own SHA policy, "start-up does not wait"
  (line 121; true after ticket 13), the file-layer sentence at 668 (true after ticket 03),
  `jsonld_budget` "under 10 ms" (it asserts ≤ 2× Turtle), LATERAL undocumented,
  `--timeout 0` meaning undocumented, `--version` undocumented, `format --help` omits
  JSON-LD, crate doc and description undersell the scope; `deny.toml` comment says the
  RDF/XML cap runs "on a background thread" (the CLI runs it inline; cost ~0.3 s/MB);
  `CHANGELOG.md` needs an Unreleased section for this wave.

**Files owned:** `SECURITY.md`, `README.md`, `CHANGELOG.md`, `editors/vscode/README.md`,
`editors/vscode/package.json` (`capabilities.untrustedWorkspaces.description` only),
`docs/specs/*.md` (status lines), `docs/research/*.md` (a "historical" banner), new
`ARCHITECTURE.md`, new `AGENTS.md`, new `THIRD_PARTY_NOTICES.md` (crates via
`cargo about` or a hand-kept list; data: schema.org, W3C contexts, vocabulary sources;
Monaco), `editors/vscode/scripts/package.mjs` and `editors/vscode/.vscodeignore` and
`web/build.mjs` (ship the notices), `deny.toml` (comments only), `src/lib.rs` (line 1),
`src/cli/mod.rs` (usage strings), `Cargo.toml` (`description` only), `CONTEXT.md`
(retire with a pointer, or refresh).
**Hands off:** all other code.

**Blocked by:** every other wave-2 ticket (this one describes the result). **Last.**

**Status:** ready-for-agent

- [ ] SECURITY.md "What the project does" rewritten from the code: when it fetches
      (settings, `--network`, the quick fix), which file kinds it reads and where the
      cache is, the limits (size, time, redirects, address policy), what isolation
      covers, what the chaos test reaches, which manifests `cargo deny`/`npm audit`
      cover; Scope lists the CLI, the browser build, `build.lua`. Every sentence checked
      against a file:line.
- [ ] Spec status lines say "done in vX.Y" or the specs move to `docs/specs/done/`;
      research docs get a banner; `sota-roadmap.md` no longer points at `.scratch/`.
- [ ] `ARCHITECTURE.md`: module map (from the `src/lib.rs` index), the parse-once /
      analysis invariant, the seams (`batch`, `Embedded`, `ContextLoader`, `query::run`),
      where each feature family lives, the lock files. `AGENTS.md`: test commands with
      the 2-minute cap, the release checklist (version in six files, toolchain bump,
      vendored-patch check), "no network by default", fixture rules.
- [ ] `THIRD_PARTY_NOTICES.md` generated/kept and shipped in the tarball, zip, `.vsix`
      and the Pages site (`web/build.mjs` copies Monaco's notices).
- [ ] README figures derived or corrected (vocabulary count from
      `grep -c '^@ns' data/vocab.tsv`; version placeholders `vX.Y.Z`; SHA-pinned CI
      example; perf sentences regenerated from one bench run with commit and machine).
- [ ] `editors/vscode/test/manifest.test.mjs` green; `cargo deny check` green; a
      docs-only diff otherwise.
