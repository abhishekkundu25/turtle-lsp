//! JSON-LD 1.1: a frontend that turns a `.jsonld` document into the same
//! [`Parsed`] Turtle and TriG produce, so knowledge, lints, SHACL, the CLI
//! and the browser engine need no JSON-LD-specific code.
//!
//! The pipeline, one module per stage:
//! - [`json`]: a fault-tolerant JSON parser keeping the span of every key,
//!   value and bracket ([`json::Json`]), reporting `json-syntax` errors;
//! - [`loader`]: where remote `@context` documents come from
//!   ([`ContextLoader`]);
//! - [`context`]: context processing (the active context and its term
//!   definitions), with errors on the offending definition;
//! - [`expand`] and [`to_rdf`]: expansion and deserialization to triples and
//!   named graphs, each triple keeping the spans of the key, `@id` or value
//!   that produced it;
//! - [`lint`]: JSON-LD-specific warnings (undefined keys, invalid IRIs and
//!   language tags).
//!
//! What a JSON-LD [`Parsed`] holds:
//! - `syntax` is [`Syntax::JsonLd`] and `document_base` the base passed in;
//! - `triples`: `p_span` is the key that gave the predicate, `o_span` the
//!   value that gave the object; `statement` indexes `statements`, one per
//!   node object, whose span is the object and whose `subject` is its `@id`;
//! - `graph` is the named graph (`@graph` with an `@id`), as in TriG;
//! - `refs` are the IRIs written in the document (an `@id`, a key, a type)
//!   with their roles; `errors` carry `json-syntax` and `jsonld-context`
//!   problems;
//! - `prefixes`, `prefix_uses`, `graphs` and `comments` stay empty: JSON-LD
//!   prefixes live in `@context`, and JSON has no comments or graph blocks;
//! - `tokens` stays empty: its kinds are Turtle's. Spans for highlighting
//!   come from the [`json::Json`] tree.
//!
//! Only the JSON parser is real so far: [`parse`] returns its syntax errors
//! and no triples.

pub mod context;
pub mod expand;
pub mod json;
pub mod lint;
pub mod loader;
pub mod to_rdf;

pub use loader::{ContextLoader, LoadError, NoNetwork, RemoteDocument};

use crate::syntax::{Parsed, Syntax};

/// Parse a JSON-LD document into the shared parse model.
///
/// `base` is the document's URL: relative IRIs resolve against it until an
/// `@base` replaces it (`None` leaves them unresolved, as for an untitled
/// buffer). Remote contexts come from `loader`; one it cannot supply is an
/// error on the `@context` entry that names it, never a network access the
/// loader did not allow.
pub fn parse(text: &str, base: Option<&str>, loader: &dyn ContextLoader) -> Parsed {
    // Later stages: context processing, expansion and toRdf over `doc`.
    let doc = json::parse(text);
    let _ = loader;
    Parsed {
        syntax: Syntax::JsonLd,
        document_base: base.map(str::to_string),
        errors: doc.errors,
        ..Parsed::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stub_parses_to_an_empty_jsonld_model() {
        let text = r#"{"@context": "https://schema.org/", "@id": "ex:a", "name": "A"}"#;
        let p = parse(text, Some("file:///x/a.jsonld"), &NoNetwork::default());
        assert_eq!(p.syntax, Syntax::JsonLd);
        assert_eq!(p.document_base.as_deref(), Some("file:///x/a.jsonld"));
        assert!(p.triples.is_empty() && p.refs.is_empty() && p.errors.is_empty());
        assert!(p.tokens.is_empty() && p.prefixes.is_empty());
    }

    #[test]
    fn the_turtle_entry_point_hands_jsonld_to_this_frontend() {
        // Turtle would report the strings and the colon; JSON only the
        // unclosed `{`.
        let p = crate::syntax::parse_with_base(
            "{\"@id\": \"ex:a\"",
            &crate::syntax::PrefixMap::default(),
            Syntax::JsonLd,
            None,
        );
        assert_eq!(p.syntax, Syntax::JsonLd);
        let errors: Vec<_> = p
            .errors
            .iter()
            .map(|e| (e.span, e.message.as_str()))
            .collect();
        assert_eq!(
            errors,
            [(
                crate::syntax::Span::new(0, 1),
                "Unclosed object: expected '}'"
            )]
        );
    }
}
