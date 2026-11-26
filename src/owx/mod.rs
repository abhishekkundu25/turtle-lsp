//! The OWL 2 XML Serialization (OWL/XML, W3C Recommendation 11 December
//! 2012): the frontend of [`Syntax::OwlXml`], a notational variant of OWL
//! Functional syntax that every feature treats as it treats Functional
//! syntax, read-only but for completion.
//!
//! A `.owx` name leads here, and so does a `.owl` or `.rdf` text that
//! sniffs as OWL/XML ([`Syntax::of`]), and an import in OWL/XML. The stages:
//! - [`crate::rdfxml::xml`], the XML scanner of the RDF/XML frontend, with
//!   its spans, namespaces and limits on nesting and entities;
//! - [`reader`]: the Recommendation's schema and prose, checked in one walk
//!   driven by the signature table ([`crate::ofn::signature`]), into the
//!   tree the Functional grammar builds ([`crate::ofn::tree::Document`]);
//! - [`source`]: what the mapping asks of each node (its IRI value, its
//!   reference form, its literal), answered from the text and the reader's
//!   terms;
//! - the mapping of Functional syntax ([`crate::ofn::rdf::map_with`]), into
//!   the shared [`Parsed`], so no feature but completion has OWL/XML-specific
//!   code;
//! - [`expected`], for completion (`features::completion::owx`): what may
//!   come at an offset, read with the XML cursor and the slot machine of
//!   Functional syntax ([`crate::ofn::expected`]), and where a `<Prefix>`
//!   goes.
//!
//! It names only `syntax`, `ofn` and `rdfxml::xml` (the `boundaries` test
//! in `src/lib.rs`). [`CODE`] is the code of every error but the scanner's
//! (`xml-syntax`), and the one the editor's note on a capped list carries.
//! An item with an error gives nothing; the rest is read.
//!
//! Where it takes what the Recommendation does not settle, as OWL API (and
//! so ROBOT and Protégé) writes it, each held by a test on the output of
//! ROBOT 1.9.11 (OWL API 4.5.29): `tests/owx/forms.robot-1.9.11.owx` and
//! the rules of `rule_tests`:
//! - SWRL rules, for which the schema has no element: `DLSafeRule`, its
//!   `Annotation`s, `Body` and `Head` (each may be empty, `<Body/>`), and
//!   the atoms `ClassAtom`, `DataRangeAtom`, `ObjectPropertyAtom`,
//!   `DataPropertyAtom`, `BuiltInAtom`, `SameIndividualAtom` and
//!   `DifferentIndividualsAtom`, with the operands of their Functional
//!   forms as child elements, but that a built-in's IRI is the `IRI` or
//!   `abbreviatedIRI` of its `BuiltInAtom`. A variable is `<Variable
//!   IRI="…"/>` or `<Variable abbreviatedIRI="…"/>`, an individual argument
//!   a variable or a `NamedIndividual`, a data argument a variable or a
//!   `Literal`. A rule reads to the tree the Functional grammar builds for
//!   it, so it maps to the same SWRL triples. Not read, as no ROBOT output
//!   shows it: an `AnonymousIndividual` as an argument (OWL API does not
//!   read one in Functional syntax).
//! - A `nodeID` is an `NCName`, or `_:` and an `NCName` as OWL API writes
//!   it (`_:genid2147483648`). The whole value is the label: `x` and `_:x`
//!   are two individuals.
//! - Literals, where the Recommendation does not say how `datatypeIRI` and
//!   `xml:lang` combine: ROBOT writes `<Literal>` with neither for a string
//!   (`xsd:string`, and `"…@"^^rdf:PlainLiteral`), `xml:lang` and no
//!   `datatypeIRI` for a language string (`"…@tag"^^rdf:PlainLiteral`
//!   too), and `datatypeIRI` for any other datatype, as the reader takes
//!   them. It never writes `datatypeIRI` for `rdf:PlainLiteral`, so that
//!   form is read as in Functional syntax: without an `@` or an
//!   `xml:lang`, a plain string with a warning, as OWL API reads it.

#[cfg(test)]
mod annotation_tests;
#[cfg(test)]
mod axiom_tests;
pub(crate) mod expected;
#[cfg(test)]
mod printer;
mod reader;
#[cfg(test)]
mod rule_tests;
mod source;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod timing;

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

/// What the editor features see of an OWL/XML document: its errors, its
/// prefixes and its completion, and nothing to format, colour, splice or
/// rename, as in OWL Functional syntax.
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
        use crate::features::navigation::{PrefixScope, RdfXml, rename};
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
        // Completion (`completion::owx`) and rename are the edits; nothing
        // is formatted, coloured or spliced.
        assert!(!crate::features::writes(Syntax::OwlXml));
        assert!(crate::features::completes(Syntax::OwlXml));
        let got = completion::complete(&a, &k, Encoding::Utf8, Position::new(4, 39));
        let CompletionResponse::List(list) = got else {
            panic!("{got:?}")
        };
        assert!(list.items.iter().any(|i| i.label == ":A"), "{list:?}");
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
        let at = Position::new(4, 39);
        let refused = rename(&a, &k, Encoding::Utf8, at, "B", scope, RdfXml::Edit);
        let message = refused.unwrap_err();
        assert!(message.contains("an OWL/XML document"), "{message}");
    }
}
