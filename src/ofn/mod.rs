//! OWL 2 Functional-Style Syntax: a frontend that checks a document
//! against the grammar of the OWL 2 Structural Specification and
//! Functional-Style Syntax (Second Edition, W3C Recommendation 11 December
//! 2012, sections 2 to 13), reports every error at its exact span, and maps
//! its axioms to RDF (OWL 2 Mapping to RDF Graphs, section 2). It names
//! only `syntax`.
//!
//! The stages, one module each:
//! - [`lexer`]: the terminal symbols, white space and comments (section
//!   2.2), with the span of each;
//! - [`grammar`]: the productions (section 13) over [`lexer`], into a
//!   [`tree::Document`], read on past each error to the next axiom;
//! - [`signature`]: the operands of each construct, which the grammar
//!   reads its productions from;
//! - [`tree`]: the document's constructs as a flat tree with exact spans;
//! - [`rdf`]: the mapping of the tree to the triples of the W3C mapping,
//!   with the references, statements and prefixes of the shared parse; it
//!   asks a source ([`rdf::Source`]) how the text spells each term.
//!
//! [`parse`] gives the shared [`Parsed`], as Turtle, JSON-LD and RDF/XML
//! do, so the workspace, knowledge, SHACL, reasoning, the query graph and
//! the editor features that read need no Functional-specific code. The
//! OWL/XML frontend (`owx`) builds the same tree, checked against the same
//! [`signature`] table, and maps it with [`rdf`] through a source of its
//! own; it is the one frontend that names this one, and besides it only
//! `analysis`, `imports` and `features` (completion, the error code and
//! the rename planner `features::navigation::owl`) do. An item
//! with an error gives no triples; what the mapping takes as a definition,
//! and its own departures (SWRL), are in [`rdf`]. Every error has the code
//! [`CODE`].
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
//! reported, then one more error that says so, and apart from them at
//! most [`MAX_WARNINGS`] warnings, then one more warning. Time is linear
//! in the text.

pub(crate) mod expected;
mod grammar;
mod lexer;
pub(crate) mod rdf;
#[cfg(test)]
mod rdf_tests;
pub(crate) mod signature;
#[cfg(test)]
mod tests;
pub(crate) mod tree;

#[cfg(test)]
use crate::syntax::Syntax;
pub(crate) use crate::syntax::shown;
use crate::syntax::{Parsed, Severity, Span, SyntaxError};
use std::borrow::Cow;

/// The prefix names of Table 2, which every document has.
pub(crate) const STANDARD: [(&str, &str); 4] = [
    ("rdf", "http://www.w3.org/1999/02/22-rdf-syntax-ns#"),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", "http://www.w3.org/2002/07/owl#"),
];

/// The diagnostic code of every error in a Functional document.
pub(crate) const CODE: &str = "ofn-syntax";

/// The most errors one document reports.
pub(crate) const MAX_ERRORS: usize = 10_000;

/// The most warnings one document reports, apart from its errors: past
/// it, one warning says so ([`more_warnings`]).
pub(crate) const MAX_WARNINGS: usize = 10_000;

/// The warning that says the warnings past [`MAX_WARNINGS`] are not
/// reported, at the first of them.
pub(crate) fn more_warnings(span: Span, code: &'static str) -> SyntaxError {
    SyntaxError::new(
        span,
        format!("More than {MAX_WARNINGS} warnings; the rest are not reported"),
        Severity::Warning,
        code,
    )
}

/// Parse an OWL Functional document into the shared parse model: its
/// axioms as RDF triples ([`rdf`]), with their references, statements and
/// prefixes, and its errors. `base` is the document's URL (every IRI in
/// the syntax is absolute: nothing resolves against it).
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    rdf::map(text, grammar::parse(text), base)
}

/// The grammar's tree of `text`, for the tests of another producer.
#[cfg(test)]
pub(crate) fn tree(text: &str) -> tree::Document {
    grammar::parse(text)
}

/// The errors of a parse, at most [`MAX_ERRORS`].
pub(crate) struct Errors {
    errors: Vec<SyntaxError>,
    /// Where the first error past the limit is.
    dropped: Option<Span>,
    /// The least start of the errors reported since [`Errors::restart`],
    /// those past the limit too; `u32::MAX` if none.
    least: u32,
}

impl Default for Errors {
    fn default() -> Errors {
        Errors {
            errors: Vec::new(),
            dropped: None,
            least: u32::MAX,
        }
    }
}

impl Errors {
    pub(crate) fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        self.least = self.least.min(span.start);
        if self.errors.len() < MAX_ERRORS {
            self.errors
                .push(SyntaxError::new(span, message, Severity::Error, CODE));
        } else if self.dropped.is_none() {
            self.dropped = Some(span);
        }
    }

    /// The least start of the errors reported since the last restart, and
    /// a new count from here.
    pub(crate) fn restart(&mut self) -> u32 {
        std::mem::replace(&mut self.least, u32::MAX)
    }

    /// Go on counting from `least`, as [`Errors::restart`] returned it.
    pub(crate) fn resume(&mut self, least: u32) {
        self.least = least;
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

/// What the editor features see of a Functional document: its errors, its
/// terms to hover, follow and list, as in RDF/XML, its keywords and terms
/// to complete, and nothing to format or colour.
#[cfg(test)]
mod gates {
    use super::*;
    use crate::syntax::Syntax;

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
        use crate::knowledge::{Knowledge, Source};
        use lsp_types::{CompletionResponse, Position};
        use std::sync::Arc;
        let text = "Prefix(:=<http://example.org/o#>)\nPrefix(ex:=<http://example.org/>)\nOntology(<http://example.org/o>\nDeclaration(Class(:A))\nSubClassOf(:A nope:B)\nSubClassOf(:A)\nAnnotationAssertion(rdfs:label :A \"Apple\")\n)\n";
        for uri in ["file:///w/o.ofn", "file:///w/o.owl", "file:///w/O.OFN"] {
            let a = Arc::new(Analysis::new(uri.into(), text, Arc::default(), true));
            let mut k = Knowledge::default();
            k.set(uri, Source::Open, a.clone());
            assert_eq!(a.parsed.syntax, Syntax::OwlFunctional, "{uri}");
            assert_eq!(a.parsed, parse(text, Some(uri)));
            assert!(a.tokens().is_empty() && a.tokens_or_lexed().is_empty());
            // Its errors, at their spans; its terms are known.
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
            // A term hovers with its label, at the text of its IRI.
            let h = hover::hover(&a, &k, Encoding::Utf8, Position::new(3, 19)).unwrap();
            let lsp_types::HoverContents::Markup(m) = &h.contents else {
                panic!("{h:?}")
            };
            assert!(m.value.contains("Apple"), "{}", m.value);
            assert_eq!(
                h.range,
                Some(lsp_types::Range::new(
                    Position::new(3, 18),
                    Position::new(3, 20)
                ))
            );
            // Its outline: the prefixes, the ontology and the class, once.
            let names: Vec<_> = symbols::document_symbols(&a, Encoding::Utf8)
                .into_iter()
                .map(|s| s.name)
                .collect();
            assert_eq!(names, ["prefixes", "ex:o", ":A"]);
            // Completion offers the keywords of the grammar and the terms of
            // a slot's kind, and nothing in a string; nothing is formatted or
            // coloured.
            assert!(!crate::features::writes(a.parsed.syntax));
            assert!(crate::features::completes(a.parsed.syntax));
            let labels = |line, character| -> Vec<String> {
                let pos = Position::new(line, character);
                match completion::complete(&a, &k, Encoding::Utf8, pos) {
                    CompletionResponse::Array(v) => v,
                    CompletionResponse::List(l) => l.items,
                }
                .into_iter()
                .map(|i| i.label)
                .collect()
            };
            let at_axiom = labels(3, 0);
            assert!(at_axiom.iter().any(|l| l == "SubClassOf"), "{at_axiom:?}");
            let in_class = labels(5, 13);
            assert!(in_class.iter().any(|l| l == ":A"), "{in_class:?}");
            assert_eq!(labels(6, 37), Vec::<String>::new());
            assert_eq!(
                format::formatting(&a, Encoding::Utf8, &format::Options::default()),
                []
            );
            assert_eq!(
                format::format_analysis(&a, &format::Options::default()),
                None
            );
            assert!(semantic::full(&a, &k, Encoding::Utf8).data.is_empty());
            // The prefixes fold, as in Turtle.
            let folds: Vec<_> = symbols::folding_ranges(&a)
                .iter()
                .map(|f| (f.start_line, f.end_line))
                .collect();
            assert_eq!(folds, [(0, 1)]);
        }
    }

    /// A term's own axioms say something about it; they are not uses of it.
    /// A deprecated class is reported where another axiom uses it, not at its
    /// label or its superclass axiom, as a subject in Turtle or RDF/XML is
    /// not; and it is described, by its declaration.
    #[test]
    fn a_terms_own_axioms_are_not_uses_of_it() {
        use crate::analysis::{Analysis, Encoding};
        use crate::features::diagnostics;
        use crate::knowledge::{Knowledge, Source};
        use std::sync::Arc;
        let text = "Prefix(:=<http://example.org/o#>)\nOntology(<http://example.org/o>\nDeclaration(Class(:Old))\nAnnotationAssertion(owl:deprecated :Old \"true\"^^xsd:boolean)\nAnnotationAssertion(rdfs:label :Old \"old\")\nSubClassOf(:Old :Thing)\nSubClassOf(:New :Old)\nAnnotationAssertion(rdfs:label :Undeclared \"u\")\nAnnotationAssertion(rdfs:comment :Undeclared \"c\")\n)\n";
        let uri = "file:///w/o.ofn";
        let a = Arc::new(Analysis::new(uri.into(), text, Arc::default(), true));
        let roles: Vec<_> = a
            .parsed
            .refs
            .iter()
            .filter(|r| r.iri.as_str().ends_with("#Old") || r.iri.as_str().ends_with("#Undeclared"))
            .map(|r| (r.span.start, r.role))
            .collect();
        use crate::syntax::Role::*;
        assert_eq!(
            roles.iter().map(|r| r.1).collect::<Vec<_>>(),
            [Subject, About, About, About, Object, Subject, About]
        );
        assert!(text[roles[4].0 as usize..].starts_with(":Old)"));
        let mut k = Knowledge::default();
        k.set(uri, Source::Open, a.clone());
        let d = diagnostics::diagnostics_with(&a, &k, Encoding::Utf8, &Default::default());
        let found: Vec<_> = d
            .iter()
            .map(|d| match &d.code {
                Some(lsp_types::NumberOrString::String(c)) => (c.as_str(), d.range.start.line),
                _ => ("", 0),
            })
            .collect();
        // Only the use in `SubClassOf(:New :Old)`, on line 7.
        assert_eq!(found, [("deprecated-term", 6)], "{d:?}");
    }
}
