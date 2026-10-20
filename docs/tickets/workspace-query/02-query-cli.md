# 02: `turtle-lsp query` and the batch entry point

**What to build:** `batch` query entry point (file collection, settings, prefixes shared with `analyse`), and `turtle-lsp query (FILE.rq | -e QUERY) [PATH…]` with `--format table|json|csv|tsv|turtle`, `--root`, `--timeout`, `--limit`. Exit 0 ran, 1 ASK false, 2 error. Usage and `--help`.

**Blocked by:** 01.

**Status:** ready-for-agent

- [ ] Node CLI tests: each format, `-e`, paths, `prefixes.ttl` applied to query and data, exit codes, a syntax error's position.
