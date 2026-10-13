//! Turtle 1.2 syntax: a lexer and a resilient recursive-descent parser that
//! records the span of every term, so every editor feature reads one parse.

mod iri;
pub mod lexer;
mod parser;

pub use iri::{Iri, IriKey, IriMap};
pub use parser::parse;
use rustc_hash::FxHashMap;

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// Byte range into the document text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

impl Span {
    pub fn new(start: u32, end: u32) -> Self {
        Span { start, end }
    }
    pub fn contains(&self, offset: u32) -> bool {
        self.start <= offset && offset <= self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Subject of a top-level statement: this is where a resource is described.
    Subject,
    Predicate,
    Object,
    Datatype,
    /// Inside a triple term or reified triple.
    Quoted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// `<http://...>`
    Iri,
    /// `prefix:local`; `prefix_len` is the byte length of `prefix` (before the colon).
    PName { prefix_len: u32 },
    /// The `a` keyword.
    A,
}

/// One occurrence of an IRI in the text.
#[derive(Clone, Debug)]
pub struct Ref {
    pub span: Span,
    pub iri: Iri,
    pub role: Role,
    pub form: Form,
}

#[derive(Clone, Debug)]
pub struct PrefixDecl {
    pub name: String,
    pub iri: Iri,
    /// Whole directive including the trailing `.` if any.
    pub span: Span,
    pub name_span: Span,
    pub iri_span: Span,
}

/// A use of a prefix name (`ex` in `ex:a`), resolved or not.
#[derive(Clone, Debug)]
pub struct PrefixUse {
    /// The prefix name, without the colon.
    pub name: Iri,
    /// Span of the whole prefixed name.
    pub span: Span,
    pub resolved: bool,
}

/// A blank node, numbered per document. Labelled nodes (`_:b1`) and the
/// nodes the parser generates (`[ ]`, collections, reifiers) share one
/// numbering, so they never collide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlankId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Iri(Iri),
    Blank(BlankId),
    Literal {
        value: Box<str>,
        datatype: Iri,
        lang: Option<Iri>,
    },
    Triple(Box<(Node, Iri, Node)>),
}

#[derive(Clone, Debug)]
pub struct Triple {
    pub s: Node,
    pub p: Iri,
    pub o: Node,
    /// The predicate as written (`a`, `ex:p`, `<...>`). Generated triples
    /// (`rdf:first`, `rdf:rest`, `rdf:reifies`) use the span of the item,
    /// triple or annotated object they come from.
    pub p_span: Span,
    /// The whole object term, including nested `[ ]`, `( )` and `<< >>`.
    pub o_span: Span,
    /// Index into [`Parsed::statements`]; the subject is that statement's.
    pub statement: u32,
}

#[derive(Clone, Debug)]
pub struct Statement {
    pub span: Span,
    /// The top-level subject, if it is an IRI.
    pub subject: Option<(Iri, Span)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct SyntaxError {
    pub span: Span,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub prefixes: Vec<PrefixDecl>,
    pub base: Option<String>,
    pub statements: Vec<Statement>,
    pub refs: Vec<Ref>,
    pub prefix_uses: Vec<PrefixUse>,
    pub triples: Vec<Triple>,
    pub errors: Vec<SyntaxError>,
    pub comments: Vec<Span>,
    pub tokens: Vec<lexer::Token>,
}

/// Prefixes visible before the first line of a document (from `prefixes.ttl`).
pub type PrefixMap = FxHashMap<String, String>;
