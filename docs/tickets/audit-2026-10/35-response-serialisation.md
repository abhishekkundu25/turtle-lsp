# 35: Response serialisation and RSS

**What to build:** every response goes through a `serde_json::Value` tree before it
becomes text. `Server::request` returns `Result<Value, _>`, the arms call
`serde_json::to_value` (34 sites in `src/server.rs` and `src/server/*` before 28a-c),
and `handle` wraps the value in `lsp_server::Response`, whose `result` is a `Value`
(`src/server.rs:907-926`). The connection is `lsp_server::Connection::stdio()`
(`:489`), whose writer thread serialises each `Message`. So a large answer exists
three times: the Rust structs, the `Value` tree and the JSON text. mimalloc does not
return the freed pages, so RSS ratchets up to the largest response seen (audit X5-05,
measurement M14). After 28a-c each arm is decode, call, encode, so there is one place
to change. Encode a request's result straight to text (`serde_json::to_string` on the
`Session` result). Send it through an outbound path that writes the LSP frame
(`Content-Length` header, then `{"jsonrpc":"2.0","id":…,"result":…}`) byte for byte as
`lsp_server::Message::write` writes it. That needs a small stdio transport of our own
in `src/server/wire.rs`: a reader thread on `lsp_server::Message::read`, and one
writer thread fed by one ordered channel of `Out::Message(Message)` and
`Out::Result { id, json }`, so a response can never pass a notification sent before it.
The initialize handshake behaves as `Connection::initialize_start`/`initialize_finish`
do: a request before `initialize` gets `ServerNotInitialized`, other notifications are
dropped, `exit` ends the session, and the server waits for `initialized`. `Server`
sends through this outbox instead of `conn.sender` (17 sites in `src/server.rs`, 3 in
`src/server/run_query.rs`, before 29a moved some into the state groups). `Embedded`
keeps its interface and keeps returning `lsp_server::Message`: its outbox parses a raw
result back into a `Value` (owner decision 11). The per-location URI parse that X5-14
also named is already fixed (`DocLocs`, `src/features/navigation.rs:72-95`, with a
test counting parses at `:691-697`) and is not part of this ticket. If raw
serialisation alone leaves RSS over the budget, the server may ask mimalloc to return
freed pages (`mi_collect`) after a response over 1 MB. That goes through an
`extern "C"` declaration in `src/main.rs`, passed to `server::run` as a hook, so no
crate is added (owner decision 11). **Behaviour:** identical bytes on the wire.

**Budget (pass/fail):** in `bench/perf.mjs`, scenario `document-symbol-rss`: the row
"documentSymbol on a 100k-triple file: RSS after 20 requests" (`rss-symbols-after`) is
**at most 1.3 times** the row "RSS before the first request" (`rss-symbols-before`,
RSS after open). Run `node bench/perf.mjs --only document-symbol-rss --json
$TMPDIR/d10.json`, with `TMPDIR` at its default. Baseline on 2026-10-22 at 8e3f8d7,
macOS arm64: 130 MB, then 367 MB (2.82 times; 5.5 MB response, 25,001 symbols). The
budget is at most 169 MB on that baseline. Ticket 36 reruns it on a quiet machine.

**Files owned:** `src/server.rs` (`run`, `configure`'s connection, `handle`, the
encode helper in `request`, every `conn.sender` site), new `src/server/wire.rs`,
`src/server/run_query.rs` and the `src/server/*` state-group files (their send sites),
`src/main.rs` (the hook only, if used), the server test helpers that build a
`Connection::memory()`.
**Hands off:** `src/ide/**` and `src/features/**` (what is computed does not change),
`web/engine/**` (the `Embedded` interface is unchanged), `src/lib.rs` (32a holds it in
this wave), `Cargo.toml` (no new crate and no new feature).

**Blocked by:** 29b (`src/server.rs` lock, the last state-group change), and 28c (the
response path goes through `Session`). Runs in the same wave as 32a, with disjoint
files (see 32a).

**Status:** done. RSS after 20 documentSymbol requests on the 100k-triple file: 1.05x RSS before (109 -> 114 MB), baseline binary 3.12x; budget 1.3x. No mi_collect hook needed. Measured under load; re-measured quietly in 36.

**Rules:** each test run stays under 2 minutes (`timeout 120`, one test binary or Node
file at a time). Leave `TMPDIR` at its default, outside the repository: the CLI tests
run `check --fix`. The only performance measurement allowed is the budget above, and
it is rerun later on a quiet machine. No new dependencies. Leave the changes
uncommitted. Stop every server you start, by its PID; leave none running.

- [ ] `rg -n "to_value" src/server.rs src/server` finds no use on a request's result,
      and `rg -n "Connection::stdio" src` finds nothing.
- [ ] Unit tests in `src/server/wire.rs`: the frame of `Out::Result` equals
      `Message::write` of the same `Response` byte for byte (numeric and string ids,
      `null` result); an `Out::Message` sent before an `Out::Result` is written before
      it; the handshake answers a request sent before `initialize` with
      `ServerNotInitialized` and ends on `exit`.
- [ ] **Budget:** `rss-symbols-after` is at most 1.3 times `rss-symbols-before`. Report
      both, the ratio, and whether the `mi_collect` hook was needed.
- [ ] Green, identical on the wire: `cargo test --release --locked --lib`,
      `--test server_lifecycle`, `--test diagnostics_agree`; the whole Node suite file
      by file (each under `timeout 120`), including `test/protocol.test.mjs`,
      `test/protocol-errors.test.mjs`, `test/resources.test.mjs` (exit paths, idle
      CPU), `test/lsp.test.mjs`, `test/query-lsp.test.mjs`, and
      `timeout 120 npm run test:latency`; `web/test` after `node web/build.mjs`;
      `cargo fmt --check`, clippy.
