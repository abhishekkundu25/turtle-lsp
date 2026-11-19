//! RDF/XML 1.1: a frontend that turns an RDF/XML document into the same
//! [`Parsed`] Turtle and JSON-LD produce, so knowledge, SHACL, reasoning,
//! the CLI and the browser engine need no RDF/XML-specific code. It names
//! only `syntax`.
//!
//! The stages, one module each:
//! - [`anchor`]: the triples `oxrdfxml` reads, each placed at the XML tag
//!   it ends at ([`Form::Anchor`](crate::syntax::Form::Anchor) spans);
//! - [`xml`]: a fault-tolerant XML scanner with spans;
//! - [`grammar`]: the RDF/XML productions over [`xml`], into [`Parsed`].
//!
//! None reads a text yet: [`parse`] returns no triples and one
//! `unsupported-syntax` error. Nothing gives a document [`Syntax::RdfXml`]
//! yet either.

mod anchor;
mod grammar;
mod xml;

use crate::syntax::{Parsed, Severity, Span, Syntax, SyntaxError};

/// Parse an RDF/XML document into the shared parse model. `base` is the
/// document's URL, against which relative IRIs resolve until an `xml:base`
/// replaces it.
pub(crate) fn parse(_text: &str, base: Option<&str>) -> Parsed {
    Parsed {
        syntax: Syntax::RdfXml,
        document_base: base.map(str::to_string),
        errors: vec![SyntaxError::new(
            Span::new(0, 0),
            "RDF/XML is not read yet",
            Severity::Error,
            "unsupported-syntax",
        )],
        ..Parsed::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::{Analysis, Encoding};
    use crate::features::{completion, format, semantic};
    use crate::knowledge::Knowledge;
    use crate::syntax::splice::{Edit, Fallback};
    use lsp_types::{CompletionResponse, Position};
    use std::sync::Arc;

    const URI: &str = "file:///w/onto.owl";
    const TEXT: &str = "<http://example.org/a> <http://example.org/p> <http://example.org/b> .\n";

    #[test]
    fn nothing_is_read_yet_and_the_error_says_so() {
        let p = parse("<rdf:RDF/>", Some(URI));
        assert_eq!(p.syntax, Syntax::RdfXml);
        assert_eq!(p.document_base.as_deref(), Some(URI));
        assert!(p.triples.is_empty() && p.refs.is_empty() && p.statements.is_empty());
        let codes: Vec<_> = p.errors.iter().map(|e| (e.code, e.severity)).collect();
        assert_eq!(codes, [("unsupported-syntax", Severity::Error)]);
    }

    #[test]
    fn an_rdfxml_analysis_comes_from_this_frontend() {
        let a = Analysis::with_syntax(URI.into(), TEXT, Arc::default(), true, Syntax::RdfXml);
        assert_eq!(a.parsed, parse(TEXT, Some(URI)));
        assert!(a.tokens().is_empty());
    }

    /// An RDF/XML analysis with triples, references and statements, and no
    /// error: what would be laid out, spliced or highlighted if the syntax
    /// did not stop it.
    fn rdfxml() -> Analysis {
        let a = Analysis::anchored(URI, TEXT, Syntax::RdfXml);
        assert!(a.parsed.errors.is_empty() && !a.parsed.statements.is_empty());
        a
    }

    #[test]
    fn rdfxml_has_no_tokens_no_formatting_and_no_splice() {
        let a = rdfxml();
        assert!(a.tokens().is_empty());
        assert!(a.tokens_or_lexed().is_empty());
        assert_eq!(Syntax::RdfXml.tokens(TEXT), []);
        assert_eq!(
            format::formatting(&a, Encoding::Utf8, &format::Options::default()),
            []
        );
        assert_eq!(
            format::format_analysis(&a, &format::Options::default()),
            None
        );
        let mut text = TEXT.to_string();
        text.insert(0, ' ');
        let spliced = a.splice(URI, &text, Edit::new(0..0, 1), &Default::default());
        assert_eq!(spliced.err(), Some(Fallback::Syntax));
    }

    #[test]
    fn rdfxml_has_no_semantic_tokens_and_no_completion() {
        let a = rdfxml();
        let k = Knowledge::default();
        assert!(semantic::full(&a, &k, Encoding::Utf8).data.is_empty());
        for character in [0, 1, 25, 50] {
            let pos = Position::new(0, character);
            let got = completion::complete(&a, &k, Encoding::Utf8, pos);
            assert!(
                matches!(&got, CompletionResponse::Array(v) if v.is_empty()),
                "{character}: {got:?}"
            );
        }
    }
}
