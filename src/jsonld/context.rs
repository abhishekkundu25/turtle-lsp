//! JSON-LD 1.1 context processing: the active context and its term
//! definitions (`@vocab`, `@base`, `@language`, `@direction`, `@protected`,
//! `@propagate`, `@import`, scoped contexts, keyword aliases), read from
//! [`super::json::Json`] with remote contexts from a
//! [`super::ContextLoader`]. Errors are `jsonld-context` diagnostics on the
//! offending definition. Not yet implemented.
