//! RDF/XML 1.1: a frontend that turns an RDF/XML document into the same
//! [`Parsed`] Turtle and JSON-LD produce, so knowledge, SHACL, reasoning,
//! the CLI and the browser engine need no RDF/XML-specific code. It names
//! only `syntax`.
//!
//! The stages, one module each:
//! - [`limits`]: what the text may ask of the XML reader (tag length,
//!   DOCTYPE entities), checked before it is read;
//! - [`anchor`]: the triples `oxrdfxml` reads, each placed at the XML tag
//!   it ends at ([`Form::Anchor`](crate::syntax::Form::Anchor) spans), and
//!   the first error as an `xml-syntax` error;
//! - [`xml`]: a fault-tolerant XML scanner with spans (not used yet);
//! - [`grammar`]: the RDF/XML productions over [`xml`], into [`Parsed`]
//!   (not used yet).
//!
//! [`parse`] reads the text with [`anchor`]. A text in an OWL syntax that is
//! not RDF (OWL/XML, Functional, Manchester) gives no triples and one
//! `unsupported-syntax` error that names it. What a [`Parsed`] of RDF/XML
//! holds beyond what [`anchor`] says: `syntax` is [`Syntax::RdfXml`],
//! `document_base` the base passed in; `prefixes`, `prefix_uses`, `graphs`,
//! `comments` and `tokens` stay empty (XML namespaces are not Turtle
//! prefixes, and nothing edits RDF/XML).

mod anchor;
mod grammar;
mod limits;
mod xml;

use crate::syntax::sniff::Sniffed;
use crate::syntax::{Parsed, Severity, Span, Syntax, SyntaxError};

/// The diagnostic code of a text in an OWL syntax that is not RDF.
const UNSUPPORTED_SYNTAX: &str = "unsupported-syntax";

/// Why the limits on what the XML may ask for refuse `text`, if they do:
/// [`parse`] then reads nothing, and says so in an `xml-syntax` error at
/// the place. For a caller that reports the refusal as it stands.
pub(crate) fn refused(text: &str) -> Option<String> {
    limits::refusal(text.as_bytes())
}

/// Parse an RDF/XML document into the shared parse model. `base` is the
/// document's URL, against which relative IRIs resolve until an `xml:base`
/// replaces it.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    let mut out = Parsed {
        syntax: Syntax::RdfXml,
        document_base: base.map(str::to_string),
        ..Parsed::default()
    };
    match Sniffed::of(text).unread() {
        Some(unread) => out.errors.push(SyntaxError::new(
            Span::new(0, 0),
            format!("The document {unread}"),
            Severity::Error,
            UNSUPPORTED_SYNTAX,
        )),
        None => anchor::read(text, base, &mut out),
    }
    out
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
    fn an_owl_syntax_is_named_and_not_read() {
        for (text, name) in [
            (
                "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"/>",
                "OWL/XML",
            ),
            ("Prefix(:=<http://example.org/o#>)", "OWL Functional syntax"),
            (
                "Prefix: ex: <http://example.org/o#>",
                "OWL Manchester syntax",
            ),
        ] {
            let p = parse(text, Some(URI));
            assert_eq!(p.syntax, Syntax::RdfXml);
            assert_eq!(p.document_base.as_deref(), Some(URI));
            assert!(p.triples.is_empty() && p.refs.is_empty() && p.statements.is_empty());
            assert_eq!(p.errors.len(), 1);
            let e = &p.errors[0];
            assert_eq!((e.code, e.severity), (UNSUPPORTED_SYNTAX, Severity::Error));
            // The words the import loader uses.
            let unread = Sniffed::of(text).unread().unwrap();
            assert_eq!(e.message(), format!("The document {unread}"));
            assert!(
                e.message()
                    .starts_with(&format!("The document holds {name}, "))
            );
            assert!(
                e.message()
                    .ends_with("In Protégé: File > Save as > RDF/XML or Turtle")
            );
        }
    }

    #[test]
    fn rdf_xml_is_read_with_its_base() {
        let text = "\u{feff}<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n  <ex:A rdf:about=\"#a\"/>\n</rdf:RDF>\n";
        let p = parse(text, Some("http://example.org/onto"));
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.triples.len(), 1);
        assert_eq!(
            p.triples[0].s,
            crate::syntax::Node::Iri(crate::syntax::Iri::intern("http://example.org/onto#a"))
        );
        let span = p.triples[0].p_span;
        assert_eq!(
            &text[span.start as usize..span.end as usize],
            "<ex:A rdf:about=\"#a\"/>"
        );
        // Turtle given as RDF/XML is an XML error, not a Turtle parse.
        let p = parse(TEXT, Some(URI));
        assert!(p.triples.is_empty());
        let codes: Vec<_> = p.errors.iter().map(|e| e.code).collect();
        assert_eq!(codes, ["xml-syntax"]);
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
