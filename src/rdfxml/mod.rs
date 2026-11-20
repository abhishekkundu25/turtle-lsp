//! RDF/XML 1.1: a frontend that turns an RDF/XML document into the same
//! [`Parsed`] Turtle and JSON-LD produce, so knowledge, SHACL, reasoning,
//! the CLI and the browser engine need no RDF/XML-specific code. It names
//! only `syntax`.
//!
//! The stages, one module each:
//! - [`xml`]: a fault-tolerant XML scanner that keeps the span of every
//!   name, attribute value, text and entity reference, with its own limits
//!   (nesting, DOCTYPE entities, entity expansion, errors);
//! - [`grammar`]: the RDF/XML productions over [`xml`], into [`Parsed`],
//!   with the exact span of every term, read on past each error;
//! - [`rename`]: the edits that rename an IRI in a document, placed at the
//!   spans the grammar reads, or why they cannot be made.
//!
//! [`parse`] reads the text with [`grammar`]. A text in an OWL syntax that is
//! not RDF (OWL/XML, Functional, Manchester) gives no triples and one
//! `unsupported-syntax` error that names it. What a [`Parsed`] of RDF/XML
//! holds is in [`grammar`]; beyond it, `syntax` is [`Syntax::RdfXml`],
//! `document_base` the base passed in, and `prefixes`, `graphs` and
//! `tokens` stay empty (XML namespaces are scoped, not Turtle prefixes, and
//! no formatter edits RDF/XML; [`rename`] parses the text again for the
//! spans it needs). References are exact ([`Form::Iri`],
//! [`Form::PName`]) where the text spells the IRI, and
//! [`Form::Anchor`] where it does not (an entity, `rdf:ID`, `rdf:li`, a
//! name in the default namespace).
//!
//! [`Form::Iri`]: crate::syntax::Form::Iri
//! [`Form::PName`]: crate::syntax::Form::PName
//! [`Form::Anchor`]: crate::syntax::Form::Anchor

mod grammar;
pub(crate) mod rename;
mod xml;

use crate::syntax::sniff::Sniffed;
use crate::syntax::{Parsed, Severity, Span, Syntax, SyntaxError};

/// The diagnostic code of a text in an OWL syntax that is not RDF.
const UNSUPPORTED_SYNTAX: &str = "unsupported-syntax";

/// Parse an RDF/XML document into the shared parse model. `base` is the
/// document's URL, against which relative IRIs resolve until an `xml:base`
/// replaces it.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    let Some(unread) = Sniffed::of(text).unread() else {
        return grammar::parse(text, base);
    };
    Parsed {
        syntax: Syntax::RdfXml,
        document_base: base.map(str::to_string),
        errors: vec![SyntaxError::new(
            Span::new(0, 0),
            format!("The document {unread}"),
            Severity::Error,
            UNSUPPORTED_SYNTAX,
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
        // The type at the element's name, the subject at its value.
        let at = |span: Span| &text[span.start as usize..span.end as usize];
        assert_eq!(at(p.triples[0].p_span), "ex:A");
        let forms: Vec<_> = p.refs.iter().map(|r| (at(r.span), r.form)).collect();
        assert_eq!(
            forms,
            [
                ("ex:A", crate::syntax::Form::PName { prefix_len: 2 }),
                ("#a", crate::syntax::Form::Iri)
            ]
        );
        // Turtle given as RDF/XML is an XML error, not a Turtle parse.
        let p = parse(TEXT, Some(URI));
        assert!(p.triples.is_empty());
        assert!(!p.errors.is_empty());
        assert!(
            p.errors
                .iter()
                .all(|e| matches!(e.code, "xml-syntax" | "rdfxml-syntax")),
            "{:?}",
            p.errors
        );
    }

    /// After a byte order mark, spans are offsets of the text as given.
    #[test]
    fn a_byte_order_mark_keeps_the_offsets_of_the_text() {
        for bom in ["", "\u{feff}"] {
            let text = format!(
                "{bom}<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n  <ex:A rdf:about=\"http://example.org/a\"/>\n</rdf:RDF>\n"
            );
            let p = parse(&text, Some(URI));
            assert!(p.errors.is_empty(), "{:?}", p.errors);
            let at = |s: Span| &text[s.start as usize..s.end as usize];
            let spans: Vec<_> = p.refs.iter().map(|r| at(r.span)).collect();
            assert_eq!(spans, ["ex:A", "http://example.org/a"]);
        }
    }

    /// Nesting is bounded by the scanner ([`xml::MAX_DEPTH`]): triple terms
    /// nested far deeper, and elements nested deeper still, are read on the
    /// 2 MiB stack of the smallest host, through the analysis, its
    /// diagnostics, the index and the import converter, which recurse on
    /// triple terms.
    #[test]
    fn deep_nesting_is_survived_on_a_small_stack() {
        const HEAD: &str = "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\" rdf:version=\"1.2\">\n<rdf:Description rdf:about=\"http://example.org/s\">";
        const TAIL: &str = "</rdf:Description></rdf:RDF>\n";
        let terms = |n: usize| {
            let open = "<ex:p rdf:parseType=\"Triple\"><rdf:Description rdf:about=\"http://example.org/s\">";
            let close = "</rdf:Description></ex:p>";
            format!(
                "{HEAD}{}<ex:p rdf:resource=\"http://example.org/o\"/>{}{TAIL}",
                open.repeat(n),
                close.repeat(n)
            )
        };
        let elements = |n: usize| {
            format!(
                "{HEAD}{}<ex:p>x</ex:p>{}{TAIL}",
                "<ex:p rdf:parseType=\"Resource\">".repeat(n),
                "</ex:p>".repeat(n)
            )
        };
        fn depth(n: &crate::syntax::Node) -> usize {
            match n {
                crate::syntax::Node::Triple(t) => 1 + depth(&t.0).max(depth(&t.2)),
                _ => 0,
            }
        }
        for text in [terms(50), terms(20_000), elements(100_000)] {
            let deepest = std::thread::Builder::new()
                .stack_size(2 << 20)
                .spawn(move || {
                    let a = Arc::new(Analysis::with_syntax(
                        URI.into(),
                        &text,
                        Arc::default(),
                        true,
                        Syntax::RdfXml,
                    ));
                    let mut k = Knowledge::default();
                    k.set(URI, crate::knowledge::Source::Open, a.clone());
                    let opts = crate::features::diagnostics::Options::default();
                    crate::features::diagnostics::diagnostics_with(&a, &k, Encoding::Utf8, &opts);
                    let _ = crate::imports::rdf_xml_to_ntriples(text.as_bytes(), URI);
                    a.parsed.triples.iter().map(|t| depth(&t.o)).max()
                })
                .unwrap()
                .join()
                .unwrap();
            assert!(deepest.unwrap_or(0) <= xml::MAX_DEPTH / 2, "{deepest:?}");
        }
        let p = parse(&terms(50), Some(URI));
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.triples.iter().map(|t| depth(&t.o)).max(), Some(50));
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

    /// An empty `.owl` or `.rdf` file, or one of white space only, is
    /// RDF/XML: no Turtle keyword is offered, nothing is formatted.
    #[test]
    fn an_empty_owl_file_is_rdfxml() {
        let k = Knowledge::default();
        for text in ["", "\n", "  \n\t\n"] {
            for uri in [URI, "file:///w/data.rdf"] {
                let a = Analysis::new(uri.into(), text, Arc::default(), true);
                assert_eq!(a.parsed.syntax, Syntax::RdfXml, "{uri} {text:?}");
                for line in [0, 1] {
                    let pos = Position::new(line, 0);
                    let got = completion::complete(&a, &k, Encoding::Utf8, pos);
                    assert!(
                        matches!(&got, CompletionResponse::Array(v) if v.is_empty()),
                        "{uri} {text:?}: {got:?}"
                    );
                }
                assert_eq!(
                    format::formatting(&a, Encoding::Utf8, &format::Options::default()),
                    []
                );
            }
        }
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
