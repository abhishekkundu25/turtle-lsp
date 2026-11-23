//! OWL 2 Functional-Style Syntax: a frontend that checks a document
//! against the grammar of the OWL 2 Structural Specification and
//! Functional-Style Syntax (Second Edition, W3C Recommendation 11 December
//! 2012, sections 2 to 13) and reports every error at its exact span. It
//! names only `syntax`.
//!
//! The stages, one module each:
//! - [`lexer`]: the terminal symbols, white space and comments (section
//!   2.2), with the span of each;
//! - [`grammar`]: the productions (section 13) over [`lexer`], into a
//!   [`tree::Document`], read on past each error to the next axiom;
//! - [`tree`]: the document's constructs as a flat tree with exact spans.
//!
//! [`parse`] gives the shared [`Parsed`] with the errors and nothing else:
//! no triples, references, statements or prefixes, so the workspace,
//! knowledge, SHACL, reasoning and every editor feature see no content.
//! Mapping the axioms to RDF is still to come; until then a Functional
//! document is checked for errors and its content is not read into the
//! workspace. Every error has the code [`CODE`].
//!
//! Where it takes what the grammar lacks, as OWL API (and so ROBOT and
//! Protégé) writes it:
//! - `Prefix(rdf:=…)`, `rdfs:`, `xsd:` and `owl:` (Table 2) may be
//!   declared, and the declaration binds the name (section 3.7 says they
//!   MUST NOT be declared). OWL API writes them with their own IRIs, and
//!   the approved W3C test `FS2RDF-no-builtin-prefixes-ar` binds all four
//!   to another IRI. A name declared again with the same IRI is no error
//!   either (section 3.7: at most once).
//! - SWRL rules, as OWL API writes them: `DLSafeRule( {Annotation}
//!   Body( {Atom} ) Head( {Atom} ) )`, the atoms `ClassAtom( CE IArg )`,
//!   `DataRangeAtom( DR DArg )`, `ObjectPropertyAtom( OPE IArg IArg )`,
//!   `DataPropertyAtom( DP IArg DArg )`, `BuiltInAtom( IRI DArg { DArg } )`,
//!   `SameIndividualAtom( IArg IArg )` and `DifferentIndividualsAtom( IArg
//!   IArg )`, an `IArg` being `Variable( IRI )` or an individual and a
//!   `DArg` `Variable( IRI )` or a literal. `ro-edit.owl` has rules with
//!   `ClassAtom` and `ObjectPropertyAtom`; ROBOT 1.9.11 writes all seven
//!   kinds so (a unit test holds its output).
//! - A local name is SPARQL 1.1's `PN_LOCAL` without backslash escapes
//!   (`:` and `%XX` inside), a superset of the SPARQL 1.0 one the grammar
//!   cites.
//! - A byte order mark at the start is skipped, as in the other frontends
//!   (the specification names none); spans stay offsets of the text as
//!   given.
//!
//! Also decided here, where the specification leaves room: a relative IRI
//! is an error in a `Prefix` declaration too (section 2.4: each IRI is
//! absolute), and an empty document is an error (`ontologyDocument` holds
//! an `Ontology`).
//!
//! Limits: constructs nest at most [`grammar::MAX_NESTING`] deep (deeper is
//! an error, never a deep stack), and at most [`MAX_ERRORS`] errors are
//! reported, the last saying so. Time is linear in the text.

mod grammar;
mod lexer;
#[cfg(test)]
mod tests;
mod tree;

use crate::syntax::{Parsed, Severity, Span, Syntax, SyntaxError};
use std::borrow::Cow;

/// The diagnostic code of every error in a Functional document.
pub(crate) const CODE: &str = "ofn-syntax";

/// The most errors one document reports.
pub(crate) const MAX_ERRORS: usize = 10_000;

/// Parse an OWL Functional document into the shared parse model: its
/// errors, and nothing it says. `base` is the document's URL.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    Parsed {
        syntax: Syntax::OwlFunctional,
        document_base: base.map(str::to_string),
        errors: grammar::parse(text).errors,
        ..Parsed::default()
    }
}

/// The errors of a parse, at most [`MAX_ERRORS`].
#[derive(Default)]
pub(crate) struct Errors {
    errors: Vec<SyntaxError>,
    /// Where the first error past the limit is.
    dropped: Option<Span>,
}

impl Errors {
    pub(crate) fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        if self.errors.len() < MAX_ERRORS {
            self.errors
                .push(SyntaxError::new(span, message, Severity::Error, CODE));
        } else if self.dropped.is_none() {
            self.dropped = Some(span);
        }
    }

    /// The errors in text order, and the one that says more were found.
    pub(crate) fn into_vec(self) -> Vec<SyntaxError> {
        let mut errors = self.errors;
        // The lexer runs a token or two ahead of the grammar.
        errors.sort_by_key(|e| e.span.start);
        if let Some(span) = self.dropped {
            errors.push(SyntaxError::new(
                span,
                format!("More than {MAX_ERRORS} errors; the rest are not reported"),
                Severity::Error,
                CODE,
            ));
        }
        errors
    }
}

/// `s` as an error quotes it: at most 60 characters.
pub(crate) fn shown(s: &str) -> Cow<'_, str> {
    const MAX: usize = 60;
    match s.char_indices().nth(MAX) {
        None => Cow::Borrowed(s),
        Some((i, _)) => Cow::Owned(format!("{}…", &s[..i])),
    }
}

/// What the editor features see of a Functional document: its errors, and
/// nothing to complete, hover, format, colour or fold.
#[cfg(test)]
mod gates {
    use super::*;

    /// The editor shows the first errors of a document with many, and a
    /// note with the document's code, so that ignoring `ofn-syntax` hides
    /// all of them.
    #[test]
    fn the_editor_caps_the_errors_under_their_code() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::diagnostics;
        use crate::knowledge::Knowledge;
        use std::sync::Arc;
        let body = "SubClassOf(nope:A nope:B)\n".repeat(diagnostics::MAX_SYNTAX_ERRORS);
        let text = format!("Ontology({body})");
        let a = Analysis::new("file:///w/o.ofn".into(), &text, Arc::default(), true);
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
    fn an_analysis_of_a_functional_document_comes_from_this_frontend() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::{completion, diagnostics, format, hover, semantic, symbols};
        use crate::knowledge::Knowledge;
        use lsp_types::{CompletionResponse, Position};
        use std::sync::Arc;
        let text = "Prefix(:=<http://example.org/o#>)\nPrefix(ex:=<http://example.org/>)\nOntology(<http://example.org/o>\nDeclaration(Class(:A))\nSubClassOf(:A nope:B)\nSubClassOf(:A)\n)\n";
        let k = Knowledge::default();
        for uri in ["file:///w/o.ofn", "file:///w/o.owl", "file:///w/O.OFN"] {
            let a = Analysis::new(uri.into(), text, Arc::default(), true);
            assert_eq!(a.parsed.syntax, Syntax::OwlFunctional, "{uri}");
            assert_eq!(a.parsed, parse(text, Some(uri)));
            assert!(a.tokens().is_empty() && a.tokens_or_lexed().is_empty());
            // Its errors and nothing else, at their spans.
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
                        "ofn-syntax",
                        Position::new(4, 14),
                        "Undeclared prefix 'nope:'"
                    ),
                    (
                        "ofn-syntax",
                        Position::new(5, 13),
                        "Expected a class expression, found ')'"
                    ),
                ],
                "{uri}"
            );
            // Nothing is offered, formatted, coloured or named.
            assert!(!crate::features::writes(a.parsed.syntax));
            for line in 0..7 {
                for character in [0, 5, 12, 15] {
                    let pos = Position::new(line, character);
                    let got = completion::complete(&a, &k, Encoding::Utf8, pos);
                    assert!(
                        matches!(&got, CompletionResponse::Array(v) if v.is_empty()),
                        "{got:?}"
                    );
                    assert_eq!(hover::hover(&a, &k, Encoding::Utf8, pos), None);
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
            assert!(symbols::folding_ranges(&a).is_empty());
        }
    }
}
