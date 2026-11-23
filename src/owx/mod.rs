//! The OWL 2 XML Serialization (OWL/XML, W3C Recommendation 11 December
//! 2012): the frontend of [`Syntax::OwlXml`], a notational variant of OWL
//! Functional syntax that every feature treats as it treats Functional
//! syntax, read-only.
//!
//! It has no reader yet: [`parse`] gives the RDF/XML frontend's answer for
//! the text (for an OWL/XML text, no triples and one `unsupported-syntax`
//! error that names the syntax) under [`Syntax::OwlXml`]. Nothing leads a
//! document here yet (no extension, language id or content), so only a
//! caller that names the syntax gets it. [`CODE`] is the code of the
//! errors the reader will report, and the one the editor's note on a
//! capped list of them carries.

use crate::syntax::{Parsed, Syntax};

pub(crate) const CODE: &str = "owx-syntax";

/// Parse an OWL/XML document into the shared parse model. `base` is the
/// document's URL.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    Parsed {
        syntax: Syntax::OwlXml,
        ..crate::rdfxml::parse(text, base)
    }
}

/// What the editor features see of an OWL/XML document: nothing to
/// complete, format, colour, splice or rename, as in OWL Functional syntax.
#[cfg(test)]
mod gates {
    use super::*;

    const URI: &str = "file:///w/o.owx";
    const TEXT: &str = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration>\n</Ontology>\n";

    #[test]
    fn the_parse_is_the_rdfxml_frontends_answer_under_its_own_syntax() {
        let p = parse(TEXT, Some(URI));
        assert_eq!(p.syntax, Syntax::OwlXml);
        assert_eq!(p.document_base.as_deref(), Some(URI));
        assert!(p.triples.is_empty() && p.refs.is_empty(), "{p:?}");
        let errors: Vec<_> = p.errors.iter().map(|e| (e.code, e.span)).collect();
        assert_eq!(
            errors,
            [("unsupported-syntax", crate::syntax::Span::new(0, 0))]
        );
        assert!(p.errors[0].message().contains("OWL/XML"), "{:?}", p.errors);
    }

    #[test]
    fn owl_xml_is_read_and_never_written() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::navigation::{PrefixScope, rename};
        use crate::features::{completion, format, semantic};
        use crate::knowledge::{Knowledge, Source};
        use crate::syntax::splice::{Edit, Fallback};
        use lsp_types::{CompletionResponse, Position};
        use std::sync::Arc;
        let a = Arc::new(Analysis::with_syntax(
            URI.into(),
            TEXT,
            Arc::default(),
            true,
            Syntax::OwlXml,
        ));
        let mut k = Knowledge::default();
        k.set(URI, Source::Open, a.clone());
        assert_eq!(a.parsed.syntax, Syntax::OwlXml);
        assert!(a.tokens().is_empty() && a.tokens_or_lexed().is_empty());
        assert_eq!(Syntax::OwlXml.tokens(TEXT), []);
        assert!(!crate::features::writes(Syntax::OwlXml));
        for line in 0..5 {
            for character in [0, 1, 10, 30, 60] {
                let pos = Position::new(line, character);
                let got = completion::complete(&a, &k, Encoding::Utf8, pos);
                assert!(
                    matches!(&got, CompletionResponse::Array(v) if v.is_empty()),
                    "{pos:?} {got:?}"
                );
            }
        }
        assert_eq!(
            format::formatting(&a, Encoding::Utf8, &format::Options::default()),
            []
        );
        assert_eq!(
            format::format_analysis(&a, &format::Options::default()),
            None
        );
        assert!(semantic::full(&a, &k, Encoding::Utf8).data.is_empty());
        let text = format!(" {TEXT}");
        let spliced = a.splice(URI, &text, Edit::new(0..0, 1), &Default::default());
        assert_eq!(spliced.err(), Some(Fallback::Syntax));
        let scope = PrefixScope {
            file: None,
            file_of: &mut |_| None,
        };
        let refused = rename(&a, &k, Encoding::Utf8, Position::new(2, 30), "B", scope);
        let message = refused.unwrap_err();
        assert!(message.contains("an OWL/XML document"), "{message}");
    }
}
