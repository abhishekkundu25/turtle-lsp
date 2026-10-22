//! Turtle 1.2, TriG 1.2, N-Triples 1.2 and N-Quads 1.2 syntax: a lexer and a resilient
//! recursive-descent parser that records the span of every term, so every
//! editor feature reads one parse. JSON-LD has its own frontend, which builds
//! the same [`Parsed`]; this module does not call it.

mod iri;
pub mod lexer;
pub mod names;
mod parser;

pub use iri::{Iri, IriKey, IriMap};
pub use parser::{has_scheme, parse, parse_with_base};
use rustc_hash::FxHashMap;
use std::borrow::Cow;

pub const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
pub const RDF_TYPE: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#type";
pub const XSD: &str = "http://www.w3.org/2001/XMLSchema#";

/// The language a document is written in. One parser reads the first four:
/// TriG adds graph blocks to Turtle, N-Triples is read as Turtle with every
/// construct N-Triples forbids reported as an error, and N-Quads is N-Triples
/// with an optional graph label before each line's `.`. JSON-LD has its own
/// frontend, which builds the same [`Parsed`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Syntax {
    #[default]
    Turtle,
    TriG,
    NTriples,
    NQuads,
    /// JSON-LD 1.1 (`.jsonld`). Generic `.json` files are not JSON-LD.
    JsonLd,
}

impl Syntax {
    /// From an LSP language id (`turtle`, `trig`, `ntriples` / `n-triples`,
    /// `nquads` / `n-quads`, `jsonld`).
    pub fn from_language_id(id: &str) -> Option<Syntax> {
        match id.to_ascii_lowercase().as_str() {
            "turtle" => Some(Syntax::Turtle),
            "trig" => Some(Syntax::TriG),
            "ntriples" | "n-triples" => Some(Syntax::NTriples),
            "nquads" | "n-quads" => Some(Syntax::NQuads),
            "jsonld" => Some(Syntax::JsonLd),
            _ => None,
        }
    }

    /// From the extension of a path or URI, in any letter case (`Data.TTL` is
    /// Turtle): `.ttl` / `.turtle`, `.trig`, `.nt`, `.nq`, `.jsonld`.
    pub fn from_extension(path: &str) -> Option<Syntax> {
        let name = path.rsplit(['/', '\\']).next()?;
        let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
        match ext.as_str() {
            "ttl" | "turtle" => Some(Syntax::Turtle),
            "trig" => Some(Syntax::TriG),
            "nt" => Some(Syntax::NTriples),
            "nq" => Some(Syntax::NQuads),
            "jsonld" => Some(Syntax::JsonLd),
            _ => None,
        }
    }

    /// A `trig`, `ntriples`, `nquads` or `jsonld` language id wins; otherwise
    /// (no id, an unknown one such as `json`, or the generic `turtle`) the
    /// extension decides; else Turtle.
    pub fn detect(language_id: Option<&str>, uri: &str) -> Syntax {
        match language_id.and_then(Syntax::from_language_id) {
            // `turtle` is what editors send when they lump RDF files together:
            // a `.trig`, `.nt`, `.nq` or `.jsonld` name is the better witness then.
            Some(Syntax::Turtle) | None => Syntax::from_extension(uri).unwrap_or_default(),
            Some(specific) => specific,
        }
    }

    /// N-Triples and N-Quads: one statement per line, every term written in
    /// full, no directives, no prefixes.
    pub fn is_line_based(self) -> bool {
        matches!(self, Syntax::NTriples | Syntax::NQuads)
    }

    /// Whether the document can use `@prefix` / `PREFIX` prefixes and gets
    /// the project's (`prefixes.ttl`): Turtle and TriG. The line-based
    /// syntaxes have none, and JSON-LD defines its own in `@context`.
    pub fn has_prefixes(self) -> bool {
        matches!(self, Syntax::Turtle | Syntax::TriG)
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
    /// The name of a TriG graph block (`ex:g { ... }`, `GRAPH ex:g { ... }`)
    /// or the graph label of an N-Quads line.
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

/// The base direction of a directional language-tagged string
/// (`"text"@ar--rtl`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Direction {
    Ltr,
    Rtl,
}

impl Direction {
    /// `ltr` or `rtl`, as written after `--`.
    pub fn parse(s: &str) -> Option<Direction> {
        match s {
            "ltr" => Some(Direction::Ltr),
            "rtl" => Some(Direction::Rtl),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Ltr => "ltr",
            Direction::Rtl => "rtl",
        }
    }
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Node {
    Iri(Iri),
    Blank(BlankId),
    Literal {
        value: Box<str>,
        datatype: Iri,
        /// The bare language tag, lowercased (`en`, never `en--rtl`).
        lang: Option<Iri>,
        /// The base direction; only with a `lang` (`rdf:dirLangString`).
        dir: Option<Direction>,
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
    /// The TriG or N-Quads graph the triple is in; `None` is the default
    /// graph (and always the case in Turtle and N-Triples).
    pub graph: Option<GraphName>,
    /// A term was written as a prefixed name whose prefix is not declared.
    /// The term is kept as the name's own text (`nope:p`), which is not what
    /// the author meant, so [`crate::rdf::Graph::new`] leaves the triple out
    /// of the query and SHACL graph.
    pub unresolved: bool,
}

/// The name of a TriG or N-Quads graph: an IRI or a blank node. Kept apart from [`Node`]
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
    /// The graph label of an N-Quads line, and where it is written; `None`
    /// in the default graph and in the other syntaxes (TriG graphs are
    /// [`Parsed::graphs`]).
    pub graph: Option<(GraphName, Span)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
}

#[derive(Clone, Debug)]
pub struct SyntaxError {
    pub span: Span,
    /// Borrowed when the text is fixed, as most are.
    pub message: Cow<'static, str>,
    pub severity: Severity,
    /// The diagnostic code: `syntax` for Turtle and its relatives,
    /// `json-syntax` and `jsonld-context` for JSON-LD.
    pub code: &'static str,
}

impl SyntaxError {
    pub fn new(
        span: Span,
        message: impl Into<Cow<'static, str>>,
        severity: Severity,
        code: &'static str,
    ) -> SyntaxError {
        SyntaxError {
            span,
            message: message.into(),
            severity,
            code,
        }
    }

    /// What is wrong, as shown to the user.
    pub fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Default)]
pub struct Parsed {
    pub syntax: Syntax,
    pub prefixes: Vec<PrefixDecl>,
    pub base: Option<String>,
    /// The base the parse started from (the document's URL), if any; `base`
    /// is the last `@base` directive.
    pub document_base: Option<String>,
    pub statements: Vec<Statement>,
    /// TriG graph blocks in text order (empty in the other syntaxes).
    pub graphs: Vec<GraphBlock>,
    pub refs: Vec<Ref>,
    pub prefix_uses: Vec<PrefixUse>,
    pub triples: Vec<Triple>,
    pub errors: Vec<SyntaxError>,
    pub comments: Vec<Span>,
    pub tokens: Vec<lexer::Token>,
}

impl Parsed {
    /// The statements around `span`, innermost first. JSON-LD statements
    /// nest as node objects do (sorted by start): every one that contains
    /// `span`. The other syntaxes' statements never overlap: the last one
    /// starting at or before `span`, which contains it when any does (an
    /// unfinished statement may end before it).
    pub fn statements_around(&self, span: Span) -> impl Iterator<Item = usize> + '_ {
        let st = &self.statements;
        let i = st.partition_point(|s| s.span.start <= span.start);
        let nested = self.syntax == Syntax::JsonLd;
        (0..i)
            .rev()
            .take(if nested { i } else { 1 })
            .filter(move |&j| !nested || span.end <= st[j].span.end)
    }

    /// The innermost statement around `span` (see
    /// [`Parsed::statements_around`]).
    pub fn statement_at(&self, span: Span) -> Option<usize> {
        self.statements_around(span).next()
    }
}

/// Prefixes visible before the first line of a document (from `prefixes.ttl`).
pub type PrefixMap = FxHashMap<String, String>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_read_in_any_letter_case() {
        for (path, syntax) in [
            ("a/Data.TTL", Syntax::Turtle),
            ("x.Turtle", Syntax::Turtle),
            ("file:///w/G.TriG", Syntax::TriG),
            ("a.NT", Syntax::NTriples),
            ("a.Nq", Syntax::NQuads),
            ("C:\\w\\ctx.JSONLD", Syntax::JsonLd),
        ] {
            assert_eq!(Syntax::from_extension(path), Some(syntax), "{path}");
        }
        assert_eq!(Syntax::from_extension("a.JSON"), None);
        assert_eq!(
            Syntax::detect(Some("turtle"), "file:///W/DATA.NQ"),
            Syntax::NQuads
        );
    }

    #[test]
    fn a_node_stays_small() {
        // Every triple holds two; the base direction fits beside the tag.
        assert!(std::mem::size_of::<Node>() <= 32);
    }
}
