//! Turtle 1.2, TriG 1.2 and N-Triples 1.2 syntax: a lexer and a resilient
//! recursive-descent parser that records the span of every term, so every
//! editor feature reads one parse.

mod iri;
pub mod lexer;
mod parser;

pub use iri::{Iri, IriKey, IriMap};
pub use parser::{parse, parse_with_base};
use rustc_hash::FxHashMap;

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The language a document is written in. One parser reads all three: TriG
/// adds graph blocks to Turtle, and N-Triples is read as Turtle with every
/// construct N-Triples forbids reported as an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Syntax {
    #[default]
    Turtle,
    TriG,
    NTriples,
}

impl Syntax {
    /// From an LSP language id (`turtle`, `trig`, `ntriples` / `n-triples`).
    pub fn from_language_id(id: &str) -> Option<Syntax> {
        match id.to_ascii_lowercase().as_str() {
            "turtle" => Some(Syntax::Turtle),
            "trig" => Some(Syntax::TriG),
            "ntriples" | "n-triples" => Some(Syntax::NTriples),
            _ => None,
        }
    }

    /// From the extension of a path or URI: `.ttl` / `.turtle`, `.trig`, `.nt`.
    pub fn from_extension(path: &str) -> Option<Syntax> {
        let name = path.rsplit(['/', '\\']).next()?;
        match name.rsplit_once('.')?.1 {
            "ttl" | "turtle" => Some(Syntax::Turtle),
            "trig" => Some(Syntax::TriG),
            "nt" => Some(Syntax::NTriples),
            _ => None,
        }
    }

    /// The language id wins when it names one of ours; else the extension;
    /// else Turtle.
    pub fn detect(language_id: Option<&str>, uri: &str) -> Syntax {
        language_id
            .and_then(Syntax::from_language_id)
            .or_else(|| Syntax::from_extension(uri))
            .unwrap_or_default()
    }

    /// Whether the document can use prefixes (N-Triples cannot).
    pub fn has_prefixes(self) -> bool {
        self != Syntax::NTriples
    }
}

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
    /// The name of a TriG graph block (`ex:g { ... }`, `GRAPH ex:g { ... }`).
    Graph,
}

impl Role {
    /// Where a resource is introduced: a top-level subject, or a graph name
    /// (the block holds the graph's triples). Definition goes here.
    pub fn is_definition(self) -> bool {
        matches!(self, Role::Subject | Role::Graph)
    }
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
    /// The TriG graph the triple is in; `None` is the default graph (and
    /// always the case in Turtle and N-Triples).
    pub graph: Option<GraphName>,
}

/// The name of a TriG graph: an IRI or a blank node. Kept apart from [`Node`]
/// because it is `Copy` and 8 bytes, and every triple carries one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GraphName {
    Iri(Iri),
    Blank(BlankId),
}

impl From<GraphName> for Node {
    fn from(g: GraphName) -> Node {
        match g {
            GraphName::Iri(i) => Node::Iri(i),
            GraphName::Blank(b) => Node::Blank(b),
        }
    }
}

/// A TriG graph block: `{ ... }`, `ex:g { ... }` or `GRAPH ex:g { ... }`.
#[derive(Clone, Debug)]
pub struct GraphBlock {
    /// From `GRAPH` or the name to the closing `}` (or the last token of an
    /// unclosed block).
    pub span: Span,
    /// `None` for a bare `{ ... }` (the default graph).
    pub name: Option<GraphName>,
    pub name_span: Option<Span>,
    /// The statements inside, as a range of [`Parsed::statements`].
    pub statements: std::ops::Range<u32>,
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
    pub syntax: Syntax,
    pub prefixes: Vec<PrefixDecl>,
    pub base: Option<String>,
    pub statements: Vec<Statement>,
    /// TriG graph blocks in text order (empty for Turtle and N-Triples).
    pub graphs: Vec<GraphBlock>,
    pub refs: Vec<Ref>,
    pub prefix_uses: Vec<PrefixUse>,
    pub triples: Vec<Triple>,
    pub errors: Vec<SyntaxError>,
    pub comments: Vec<Span>,
    pub tokens: Vec<lexer::Token>,
}

/// Prefixes visible before the first line of a document (from `prefixes.ttl`).
pub type PrefixMap = FxHashMap<String, String>;
