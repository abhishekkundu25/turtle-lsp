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
//! So far [`parse`] runs the JSON parser and processes each top-level
//! `@context` (of the root object, or of each object in a root array) from
//! the initial context, reporting `json-syntax` and `jsonld-context` errors;
//! it produces no triples yet.

pub mod context;
pub mod expand;
pub mod json;
pub mod lint;
pub mod loader;
pub mod to_rdf;

pub use loader::{ContextLoader, LoadError, NoNetwork, RemoteDocument};

use crate::syntax::{Parsed, Syntax, SyntaxError};

/// Parse a JSON-LD document into the shared parse model.
///
/// `base` is the document's URL: relative IRIs resolve against it until an
/// `@base` replaces it (`None` leaves them unresolved, as for an untitled
/// buffer). Remote contexts come from `loader`; one it cannot supply is an
/// error on the `@context` entry that names it, never a network access the
/// loader did not allow.
pub fn parse(text: &str, base: Option<&str>, loader: &dyn ContextLoader) -> Parsed {
    // Later stages: expansion and toRdf over `doc`.
    let doc = json::parse(text);
    let mut errors = doc.errors;
    if let Some(root) = &doc.root {
        check_top_level_contexts(root, base, loader, &mut errors);
    }
    Parsed {
        syntax: Syntax::JsonLd,
        document_base: base.map(str::to_string),
        errors,
        ..Parsed::default()
    }
}

/// Until expansion applies contexts where they occur: process each
/// top-level `@context` from the initial context, so its errors show.
fn check_top_level_contexts(
    root: &json::Json,
    base: Option<&str>,
    loader: &dyn ContextLoader,
    errors: &mut Vec<SyntaxError>,
) {
    let tops: &[json::Json] = match &root.value {
        json::Value::Array(a) => &a.items,
        _ => std::slice::from_ref(root),
    };
    let mut processor = context::Processor::new(loader);
    let initial = context::ActiveContext::new(base);
    for top in tops {
        let Some(m) = top.as_object().and_then(|o| o.get("@context")) else {
            continue;
        };
        if let Err(e) = processor.process(&initial, &m.value, base, context::Options::default()) {
            errors.push(e.to_syntax_error());
        }
    }
    errors.extend(processor.take_warnings());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_document_parses_to_an_empty_jsonld_model_for_now() {
        let text = r#"{"@context": "https://schema.org/", "@id": "ex:a", "name": "A"}"#;
        let p = parse(text, Some("file:///x/a.jsonld"), &loader::Bundled);
        assert_eq!(p.syntax, Syntax::JsonLd);
        assert_eq!(p.document_base.as_deref(), Some("file:///x/a.jsonld"));
        assert!(p.triples.is_empty() && p.refs.is_empty() && p.errors.is_empty());
        assert!(p.tokens.is_empty() && p.prefixes.is_empty());
    }

    #[test]
    fn top_level_context_errors_are_diagnostics() {
        let text = r#"[{"@context": {"ok": "http://ex.org/"}}, {"@context": {"a": 5, "@b": "http://x/"}}, {"@context": "https://example.org/ctx"}]"#;
        let p = parse(text, Some("file:///x/a.jsonld"), &NoNetwork::default());
        let got: Vec<_> = p
            .errors
            .iter()
            .map(|e| {
                let at = &text[e.span.start as usize..e.span.end as usize];
                (e.code, e.severity, at, e.message.split(':').next().unwrap())
            })
            .collect();
        use crate::syntax::Severity::*;
        assert_eq!(
            got,
            [
                ("jsonld-context", Error, "5", "invalid term definition"),
                (
                    "jsonld-context",
                    Error,
                    "\"https://example.org/ctx\"",
                    "loading remote context failed"
                ),
            ]
        );
        assert!(p.errors[1].message.contains("imports.network"));
        // Processing stopped at `"a": 5` before `"@b"`; alone, `"@b"` is a
        // warning.
        let text = r#"{"@context": {"@b": "http://x/"}}"#;
        let p = parse(text, None, &NoNetwork::default());
        assert_eq!(p.errors.len(), 1);
        assert_eq!(
            (p.errors[0].severity, p.errors[0].span),
            (Warning, crate::syntax::Span::new(14, 18))
        );
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
