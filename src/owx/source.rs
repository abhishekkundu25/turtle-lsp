//! The source the mapping asks for the terms of a tree the reader built
//! ([`crate::ofn::rdf::Source`]): each answer comes from the node's
//! [`Term`], over the text and the reader's arena.
//!
//! A key tells apart what one written text means: an `IRI` value as it is
//! (scope 0), an abbreviated one (scope 1: prefixes hold for the whole
//! document), and a reference resolved against base `b` (scope `b + 2`).

use super::CODE;
use super::reader::{Term, Terms, Value};
use crate::ofn::rdf::{IriValue, Key, Literal, Source};
use crate::ofn::tree::Document;
use crate::syntax::{Form, Span, Syntax};
use std::borrow::Cow;

pub(super) struct OwlXml<'a> {
    text: &'a str,
    terms: &'a Terms,
}

impl<'a> OwlXml<'a> {
    pub(super) fn new(text: &'a str, terms: &'a Terms) -> OwlXml<'a> {
        OwlXml { text, terms }
    }

    fn iri(&self, i: usize) -> (Value, Form) {
        match self.terms.terms[i] {
            Term::Iri { value, form } => (value, form),
            Term::Construct { .. }
            | Term::Label(_)
            | Term::Literal { .. }
            | Term::Integer(_)
            | Term::Group => unreachable!("node {i} is no IRI"),
        }
    }

    fn str(&self, s: super::reader::Str) -> &'a str {
        self.terms.str(self.text, s)
    }
}

impl<'a> Source<'a> for OwlXml<'a> {
    const SYNTAX: Syntax = Syntax::OwlXml;
    const CODE: &'static str = CODE;

    fn len(&self) -> usize {
        self.text.len()
    }

    fn key(&self, _: &Document, i: usize) -> Key<'a> {
        let (scope, written) = match self.iri(i).0 {
            Value::Full(s) => (0, s),
            Value::Abbreviated { written, .. } => (1, written),
            Value::Based { base, reference } => (base + 2, reference),
        };
        Key {
            scope,
            written: self.str(written),
        }
    }

    fn value(&self, _: &Document, i: usize) -> IriValue<'a> {
        match self.iri(i).0 {
            Value::Full(s) => IriValue::Full(self.str(s)),
            Value::Abbreviated {
                written,
                colon,
                prefix,
            } => IriValue::Prefixed {
                prefix: self.str(self.terms.prefixes[prefix as usize]),
                local: &self.str(written)[colon as usize + 1..],
            },
            Value::Based { base, reference } => IriValue::Based {
                base: self.str(self.terms.bases[base as usize]),
                reference: self.str(reference),
            },
        }
    }

    fn form(&self, _: &Document, i: usize) -> Form {
        self.iri(i).1
    }

    /// The `nodeID` value.
    fn label(&self, _: &Document, i: usize) -> &'a str {
        match self.terms.terms[i] {
            Term::Label(s) => self.str(s),
            Term::Construct { .. }
            | Term::Iri { .. }
            | Term::Literal { .. }
            | Term::Integer(_)
            | Term::Group => unreachable!("node {i} is no anonymous individual"),
        }
    }

    /// The element's content whole, and the `xml:lang` in scope.
    fn literal(&self, _: &Document, i: usize) -> Literal<'a> {
        match self.terms.terms[i] {
            Term::Literal {
                lexical,
                lang,
                string,
            } => Literal {
                lexical: Cow::Borrowed(self.str(lexical)),
                lang: lang.map(|s| self.str(s)),
                string,
            },
            Term::Construct { .. }
            | Term::Iri { .. }
            | Term::Label(_)
            | Term::Integer(_)
            | Term::Group => unreachable!("node {i} is no literal"),
        }
    }

    /// The `cardinality` value, its white space collapsed.
    fn integer(&self, _: &Document, i: usize) -> &'a str {
        match self.terms.terms[i] {
            Term::Integer(s) => self.str(s),
            Term::Construct { .. }
            | Term::Iri { .. }
            | Term::Label(_)
            | Term::Literal { .. }
            | Term::Group => unreachable!("node {i} is no integer"),
        }
    }

    /// The element's name as written.
    fn keyword(&self, _: &Document, i: usize) -> Span {
        match self.terms.terms[i] {
            Term::Construct { name } => name,
            Term::Iri { .. }
            | Term::Label(_)
            | Term::Literal { .. }
            | Term::Integer(_)
            | Term::Group => unreachable!("node {i} is no construct"),
        }
    }

    fn ontology_keyword(&self, _: Span) -> Span {
        self.terms.ontology_name
    }
}
