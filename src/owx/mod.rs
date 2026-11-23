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

#[cfg(test)]
mod annotation_tests;
#[cfg(test)]
mod axiom_tests;
#[cfg(test)]
mod printer;
mod reader;
mod source;
#[cfg(test)]
mod tests;

use crate::syntax::Parsed;
#[cfg(test)]
use crate::syntax::Syntax;

pub(crate) const CODE: &str = "owx-syntax";

/// Parse an OWL/XML document into the shared parse model: the reader's
/// tree, mapped as a Functional document's is. `base` is the document's
/// URL, against which relative IRIs resolve until an `xml:base` replaces
/// it.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    let (doc, terms) = reader::read(text, base);
    crate::ofn::rdf::map_with(&source::OwlXml::new(text, &terms), doc, base)
}

/// What the editor features see of an OWL/XML document: its errors and
/// its prefixes, and nothing to complete, format, colour, splice or rename,
/// as in OWL Functional syntax.
#[cfg(test)]
mod gates {
    use super::*;

    const URI: &str = "file:///w/o.owx";
    const TEXT: &str = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n  <Declaration><Class abbreviatedIRI=\":A\"/></Declaration>\n  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"nope:B\"/></SubClassOf>\n  <SubClassOf><Class abbreviatedIRI=\":A\"/></SubClassOf>\n</Ontology>\n";

    /// The editor shows the first errors of a document with many, and a
    /// note with the document's code, so that ignoring `owx-syntax` hides
    /// all of them.
    #[test]
    fn the_editor_caps_the_errors_under_their_code() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::diagnostics;
        use crate::knowledge::Knowledge;
        use std::sync::Arc;
        let body = "<SubClassOf><Class abbreviatedIRI=\"nope:A\"/><Class abbreviatedIRI=\"nope:B\"/></SubClassOf>\n"
            .repeat(2 * diagnostics::MAX_SYNTAX_ERRORS);
        let text = format!("<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">{body}</Ontology>");
        let a = Analysis::with_syntax(URI.into(), &text, Arc::default(), true, Syntax::OwlXml);
        let opts = diagnostics::Options {
            max_syntax_errors: Some(diagnostics::MAX_SYNTAX_ERRORS),
            ..Default::default()
        };
        let d = diagnostics::diagnostics_with(&a, &Knowledge::default(), Encoding::Utf8, &opts);
        assert_eq!(d.len(), diagnostics::MAX_SYNTAX_ERRORS + 1);
        let code = |d: &lsp_types::Diagnostic| match &d.code {
            Some(lsp_types::NumberOrString::String(c)) => c.clone(),
            _ => String::new(),
        };
        assert!(d.iter().all(|d| code(d) == CODE), "{:?}", d.last());
        assert!(d.last().unwrap().message.starts_with("More than"));
    }

    #[test]
    fn owl_xml_is_read_and_never_written() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::navigation::{PrefixScope, rename};
        use crate::features::{completion, diagnostics, format, semantic, symbols};
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
        assert_eq!(a.parsed, parse(TEXT, Some(URI)));
        assert_eq!(a.parsed.document_base.as_deref(), Some(URI));
        assert!(a.tokens().is_empty() && a.tokens_or_lexed().is_empty());
        assert_eq!(Syntax::OwlXml.tokens(TEXT), []);
        // Its errors, at their spans.
        let d = diagnostics::diagnostics_with(&a, &k, Encoding::Utf8, &Default::default());
        let shown: Vec<_> = d
            .iter()
            .map(|d| {
                let code = match &d.code {
                    Some(lsp_types::NumberOrString::String(c)) => c.as_str(),
                    _ => "",
                };
                (code, d.range.start, d.message.as_str())
            })
            .collect();
        assert_eq!(
            shown,
            [
                (
                    CODE,
                    Position::new(5, 65),
                    "Undeclared prefix 'nope:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"nope\" IRI=\"…\"/>"
                ),
                (
                    CODE,
                    Position::new(6, 42),
                    "Expected a class expression, found </SubClassOf>"
                ),
            ]
        );
        // The prefixes fold, as in Turtle.
        let folds: Vec<_> = symbols::folding_ranges(&a)
            .iter()
            .map(|f| (f.start_line, f.end_line))
            .collect();
        assert_eq!(folds, [(2, 3)]);
        // Nothing is offered, formatted, coloured, spliced or renamed.
        assert!(!crate::features::writes(Syntax::OwlXml));
        for line in 0..8 {
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
        let refused = rename(&a, &k, Encoding::Utf8, Position::new(4, 39), "B", scope);
        let message = refused.unwrap_err();
        assert!(message.contains("an OWL/XML document"), "{message}");
    }
}
