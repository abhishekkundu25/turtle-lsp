# Bundled JSON-LD contexts

Common `@context` documents compiled into turtle-lsp (`include_str!` in
`src/jsonld/loader.rs`), so documents that use them work offline and in the
browser. Each file is an unmodified copy, fetched on 2026-10-20 with
`Accept: application/ld+json`.

| File | Fetched from | Served for | Licence |
|---|---|---|---|
| `schema-org.jsonld` | https://schema.org/docs/jsonldcontext.jsonld (no Last-Modified; ETag `"Z73IVw"`) | `http(s)://schema.org`, `http(s)://schema.org/`, `http(s)://schema.org/docs/jsonldcontext.json(ld)` | CC BY-SA 3.0 (schema.org) |
| `activitystreams.jsonld` | https://www.w3.org/ns/activitystreams (Last-Modified 2025-09-03) | `http(s)://www.w3.org/ns/activitystreams`, `.jsonld` | W3C Software and Document License |
| `credentials-v1.jsonld` | https://www.w3.org/2018/credentials/v1 (Last-Modified 2024-07-19) | `http(s)://www.w3.org/2018/credentials/v1` | W3C Software and Document License |
| `credentials-v2.jsonld` | https://www.w3.org/ns/credentials/v2 (Last-Modified 2025-03-11) | `http(s)://www.w3.org/ns/credentials/v2` | W3C Software and Document License |
| `did-v1.jsonld` | https://www.w3.org/ns/did/v1 (Last-Modified 2025-09-08) | `http(s)://www.w3.org/ns/did/v1` | W3C Software and Document License |

Sizes: schema.org 206 KB, the others 1.5 to 10 KB each, 238 KB in all.

To refresh a file, fetch it again with the same `Accept` header, check that
it parses as JSON, and update the date and Last-Modified above.
