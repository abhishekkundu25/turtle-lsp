# 14: Fetch module and file-URL policy

**What to fix:** `src/imports.rs` (1,892 lines) holds six modules: the `Imports` state
machine, the worker, the HTTP client (`http_get`), the cache layout (`cache_dir`,
`hash_name`, `write_atomic`), catalog parsing, and RDF/XML conversion. The HTTP client and
cache are also the JSON-LD context loader's, so the JSON-LD core depends on owl:imports.
All network access goes through `http_get`, which makes it the right place for a policy
that does not exist yet:

- Redirects follow to any host and scheme including https→http; no loopback, private,
  link-local or metadata address rule; no per-pass fetch count or byte budget (one
  import → 33 requests, 32 cache files); a new `Agent` per fetch; the `Link: alternate`
  hop is not counted (`imports.rs:1097-1102, 613-643`; `jsonld/loader.rs:426-443`).
- The per-user cache is keyed by a 64-bit FNV hash of the URL and the file does not
  record its URL, so a collision (birthday ~2^32) lets one workspace plant content for
  another (`imports.rs:1168-1191, 725-730`). `write_atomic`'s temp name collides for
  `<hash>.ttl` and `<hash>.jsonld` written from two threads (`:1131-1140`).
- `inside_workspace` calls `canonicalize()` and `DocContexts::file` calls `exists()`
  **before** the roots check (`imports.rs:664-666`, `workspace.rs:187-193`). On Windows
  a `file://host/share/x` URL in `owl:imports`, a catalog `uri=`, or a JSON-LD `@context`
  therefore opens an SMB connection with the network setting off and the workspace
  untrusted. On any OS the error message reveals whether a path outside the roots exists.
- "Fetch again" clears the failure memo under the IRI but failures are stored under the
  catalog target URL (`:608-610`, `:743-747`).
- An owl:imports file is read as Turtle or RDF/XML only, by extension `ttl|turtle|nt` or
  sniffing; `Analysis::new` then picks the syntax from the import IRI. TriG, N-Quads or
  JSON-LD imports are parsed as Turtle (`imports.rs:824-897`; `analysis.rs:104-107`).
- The JSON-LD loader starts one OS thread per context URL (`loader.rs:364-383`) and its
  memo never expires (`:222, 285-301`).

**Files owned:** new `src/fetch.rs`, `src/imports.rs` → `src/imports/{mod,worker,load,
catalog,rdfxml}.rs` (moves in their own commit; keep `crate::imports::*` paths via
re-exports), `src/jsonld/loader.rs`, `src/workspace.rs` (`DocContexts::file` and
`find_prefix_file` only), `test/imports.test.mjs`, `test/jsonld.test.mjs` (network cases
only).
**Hands off:** `src/server.rs` (ticket 13 holds it), `src/settings.rs`.

**Blocked by:** ticket 13 (`src/workspace.rs` lock #1); ticket 16 (`src/lib.rs` stub
for `fetch`).

**Status:** ready-for-agent

- [ ] `src/fetch.rs`: one `Agent` per thread; `max_redirects(0)` plus a manual loop of
      ≤ 10 hops that refuses https→http and applies the address rule after DNS
      resolution on each hop (loopback, private, link-local, metadata refused unless a
      setting allows them; tests use an "allow loopback" switch); the `Link` hop goes
      through the same loop; per-pass count and byte budget reported as an unresolved
      note; SHA-256 cache names with the source URL in a header line checked on read;
      temp names unique per writer.
- [ ] `fs_policy(url, roots)`: refuse a `file:` host other than empty/`localhost`;
      refuse UNC unless a root is UNC; compare lexically (normalise `..`,
      `starts_with(root)`) before `canonicalize`; never `exists()` first; one error for
      missing and outside. Used by `load`, `parse_catalog`, `DocContexts::file`,
      `find_prefix_file`. `cfg(windows)` test with `file://example.invalid/s/x.ttl`
      asserting no file-system call.
- [ ] Memo keyed by the normalised import IRI (refetch works through a catalog mapping).
      Import format from `Syntax::from_extension(path)` first, then sniff; never from the
      IRI. Test: a `.trig` and a `.jsonld` import load with their own syntax.
- [ ] Loader: one fetcher thread with a queue (≤ 2 threads for 100 URLs), LRU memo,
      cache-file mtime re-check; guarded spawn (shared with ticket 13's helper).
- [ ] `cargo test imports::`, loader unit tests, `test/imports.test.mjs`,
      `test/resources.test.mjs`, `test/jsonld.test.mjs`, `test/prefix-import.test.mjs`
      green; new: a redirect to loopback/http is refused; a host-bearing `file:` URL is
      refused on every OS.
