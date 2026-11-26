//! The RDF/XML productions (RDF 1.1 XML Syntax, section 7) over the XML
//! scanner's elements, into a [`Parsed`](crate::syntax::Parsed) with the
//! exact span of every term.
//!
//! [`parse`] reads the events of [`super::xml::Scanner`] with a stack of
//! states, one per open element, as `oxrdfxml` 0.2 (feature `rdf-12`)
//! does, and gives the triples it gives: on every text it accepts, the two
//! graphs are isomorphic (the oracle tests below). That covers node and
//! property elements, `rdf:about`, `rdf:ID` (and reification by it),
//! `rdf:nodeID`, `rdf:resource`, `rdf:datatype`, `rdf:parseType`
//! `Literal`, `Resource`, `Collection` and `Triple`, property attributes,
//! `rdf:li`, `xml:base`, `xml:lang`, and the RDF 1.2 terms `rdf:version`,
//! `rdf:annotation`, `rdf:annotationNodeID` and `its:dir`:
//! - `its:dir` is read only under `rdf:version="1.2"` or `"1.2-basic"`, as
//!   RDF 1.2 XML Syntax 5.1.2 says and `oxrdfxml` does: on a property
//!   element it gives its text that direction; on any element it gives the
//!   direction of the property attributes below it;
//! - `rdf:annotation`, `rdf:annotationNodeID` and `rdf:parseType="Triple"`
//!   give triple terms under any version, or none. Their productions
//!   (6.2.15 to 6.2.22) do not depend on it, and RDF 1.2 Concepts (1.10,
//!   2.1) makes the version an announcement that data may contradict.
//!   `oxrdfxml` reads annotations so, but drops a `parseType="Triple"`
//!   element without a word unless the version is 1.2; this parser reads
//!   both alike;
//! - a property element takes one of `rdf:annotation` and
//!   `rdf:annotationNodeID`, and a `parseType="Collection"` one neither
//!   (6.2.20); the withdrawn `rdf:bagID`, `rdf:aboutEach` and
//!   `rdf:aboutEachPrefix` (6.2.4, `oldTerms`) are no attributes.
//!
//! Where the two depart, this parser follows the specifications. On these,
//! `oxrdfxml` rejects the text:
//! - `xml:lang=""` means no language (RDF/XML 2.17); a relative `xml:base`
//!   resolves against the base in scope (XML Base);
//! - an empty `rdf:parseType="Literal"` is the empty XML literal;
//! - text split by a comment or a CDATA section is one literal;
//! - an entity whose replacement text refers to an entity declared after
//!   it, or holds markup (XML 1.0, 4.4.2 and 4.5; markup in content is
//!   read as markup).
//!
//! On these, `oxrdfxml` reads the text and gives another graph (each is
//! pinned by `oxrdfxml_departures` and the W3C runner below):
//! - an XML literal is in exclusive canonical form, as the W3C suite
//!   expects; `oxrdfxml` writes every namespace in scope on its first
//!   element;
//! - whitespace, then a CDATA section or a comment, then text: the literal
//!   is all of it; `oxrdfxml` drops the leading whitespace;
//! - an entity declared twice: the first declaration binds (XML 1.0, 4.2);
//!   `oxrdfxml` takes the last;
//! - a parameter entity (`<!ENTITY % e ...>`) is no general entity, so
//!   `&e;` is undeclared (reported);
//! - a `<` in an attribute value is not well-formed XML: reported, and the
//!   value and its tag end there;
//! - an empty property element with `rdf:type` and no other attribute has
//!   a blank node object of that type (7.2.21); `oxrdfxml` gives the empty
//!   literal;
//! - an unknown `rdf:parseType` reads as `"Literal"` (7.2.20); `oxrdfxml`
//!   gives no triple;
//! - `rdf:parseType="Triple"` without `rdf:version="1.2"` gives its triple
//!   term (see above); `oxrdfxml` gives no triple;
//! - line ends are normalized (XML 1.0, 2.11) and so are attribute values
//!   (3.3.3: white space becomes a space); `oxrdfxml` keeps `\r\n` in text
//!   and line ends and tabs in attribute values. FOAF has two such values
//!   (`CORPUS_KNOWN`).
//!
//! On these, `oxrdfxml` reads the text without error and this parser
//! reports one (pinned by `oxrdfxml_reads_what_this_parser_reports`):
//! `rdf:bagID`; `rdf:annotation` with `rdf:annotationNodeID`, or either on
//! `parseType="Collection"`; an empty prefixed namespace in XML 1.0; an
//! XML declaration after white space.
//!
//! Recovery: an RDF/XML error (code `rdfxml-syntax`) is reported at the
//! attribute or name that causes it, and the element is read as far as it
//! can be: an element whose name is no IRI gives no triple of its own but
//! its content is read; of two attributes that cannot go together, the
//! first in the order `rdf:about`, `rdf:ID`, `rdf:nodeID` (`rdf:resource`,
//! `rdf:nodeID`, and `rdf:annotation`, `rdf:annotationNodeID`, on a
//! property element) is kept; an IRI that does not
//! resolve, or is not absolute, is kept as written in its reference, and a
//! triple that names it (as subject, predicate, object or datatype) is
//! dropped, as Turtle and `oxrdfxml` drop it. The IRIs one parse makes are
//! bounded ([`IRI_BYTES_PER_BYTE`]); past that, new ones give no triple. So
//! are the bytes its XML literals write ([`LITERAL_BYTES_PER_BYTE`]); past
//! that, the literal being written and every later one give no triple, and
//! nothing more is written. An element the XML scanner closes for it (an
//! [`End::implied`] end) gives no triple at its end: a property element
//! that is not closed has no object yet, which is also what `oxrdfxml`
//! reads from a text cut short.
//!
//! What the parse holds:
//! - a [`Triple`] per triple, with `p_span` at the name that gives the
//!   predicate (the property element or attribute, the typed node element,
//!   `rdf:type`) and `o_span` at the object: an attribute value, a nested
//!   node element, or the content of a property element. Generated triples
//!   (reification, `rdf:first`/`rdf:rest`, `rdf:reifies`) use the span of
//!   the attribute or item they come from;
//! - a [`Statement`] per top-level node element (a child of `rdf:RDF`, or
//!   the root), from its start tag to its end tag. Nested node elements
//!   belong to it, as nested blank nodes do in Turtle, so statements never
//!   overlap. The node elements of one entity reference share its span and
//!   one statement;
//! - a [`Ref`] per IRI written in the text: an attribute value is
//!   [`Form::Iri`] (its value span, without the quotes), a prefixed element
//!   or attribute name [`Form::PName`] with a [`PrefixUse`]. Where the text
//!   is not the IRI, the span is [`Form::Anchor`]: a value with an entity or
//!   character reference, `rdf:ID` (a fragment), a name in the default
//!   namespace (no colon), and `rdf:li` (`rdf:_n`). `rdf:about` and `rdf:ID`
//!   are [`Role::Subject`], but for the `rdf:about` of a nested node element
//!   that gives no triple of its own (a collection item that only names a
//!   class): [`Role::Object`], as that IRI is in Turtle. Every reference
//!   inside a triple term is [`Role::Quoted`]. Everything read from one
//!   entity reference has its span, and the first reference stands for
//!   them all, so references never overlap; the others are only in the
//!   triples, which is where a rename looks for the IRIs an RDF/XML
//!   document uses. The name in an element's end tag repeats its start
//!   tag's reference (IRI, role and form; no [`PrefixUse`]), unless the
//!   element is empty, its end is implied by an error, or its name comes
//!   from an entity;
//! - the comments. No [`PrefixDecl`](crate::syntax::PrefixDecl): XML namespaces are scoped, and a
//!   redeclared prefix is no mistake there;
//! - blank nodes numbered in the order they first appear, so two parses of
//!   one text are equal.

use super::xml::{self, Attr, End, Event, Name, Ns, Refs, Scanner, Tag};
use crate::syntax::{
    BlankId, Direction, Form, Iri, Node, Parsed, PrefixUse, RDF, Ref, Role, Severity, Span,
    Statement, Syntax, SyntaxError, Triple, XSD, has_scheme,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::rc::Rc;

const ITS: &str = "http://www.w3.org/2005/11/its";

/// The IRIs a parse makes (a namespace and a local name, a reference
/// resolved against the base) total at most this many bytes per byte of
/// text, plus [`MIN_IRI_BYTES`]: the interner keeps every IRI for good, and
/// a long namespace or base used by many short names would otherwise make
/// it keep far more than the text.
const IRI_BYTES_PER_BYTE: usize = 16;
const MIN_IRI_BYTES: usize = 1 << 20;

/// The XML literals of a parse (`rdf:parseType="Literal"`, written as
/// exclusive canonical XML) total at most this many bytes per byte of
/// text, plus [`MIN_LITERAL_BYTES`]. Every byte appended to a literal is
/// charged, over the whole parse, so many literals share one budget.
///
/// What a document that is not crafted writes, per byte it is read from:
/// text and attribute values at most 6 (`"` read as one byte in a value
/// quoted with `'` is written `&quot;`; `>` is `&gt;`); an empty element
/// under 2 (`<b/>` is `<b></b>`); depth nothing more, since a namespace
/// is written once, on the outermost element inside the literal that
/// uses it. What is not in proportion is that declaration: each element
/// writes the namespaces it uses that no open ancestor inside the literal
/// wrote, so each top-level sibling writes them again. An XHTML `<br/>`
/// (5 bytes) is `<br xmlns="http://www.w3.org/1999/xhtml"></br>` (46
/// bytes, 9.2 per byte), and a sibling `<b/>` in a namespace of n bytes
/// writes n + 16 bytes for 4. Past 16 per byte only for n > 48, and past
/// the floor only with over 65,536 such siblings in a row (n = 64: 80
/// bytes written for 4 read, 16 over the ratio each). A sibling that uses
/// another prefix per attribute writes a declaration of at least
/// 10 + |prefix| + n bytes for each attribute of at least 6 (`a:x=""`).
///
/// The cost under the budget: 16 bytes of literal per byte of text plus
/// 1 MiB, and the event that crosses it (one tag, text run or processing
/// instruction, bounded by the text and the entity budget). While a
/// literal is written its buffer may hold twice its length (a `String`
/// doubles), and the triple copies it once more: about 3 × (16 × text +
/// 1 MiB) at the peak, 2.4 GB for the 50 MB workspace limit, where the
/// unbounded literal grows with siblings × namespace length.
const LITERAL_BYTES_PER_BYTE: usize = 16;
const MIN_LITERAL_BYTES: usize = 1 << 20;

/// Parse an RDF/XML document. `base` is the document's URL, against which
/// relative IRIs resolve until an `xml:base` replaces it.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    read(text, base, None).0
}

/// [`parse`], with the [`Use`]s of the IRIs `want` asks for.
pub(super) fn uses(text: &str, base: Option<&str>, want: Want) -> (Parsed, Vec<Use>) {
    let recorder = Recorder {
        want,
        uses: Vec::new(),
        scopes: Vec::new(),
        ends: Vec::new(),
        element: None,
    };
    read(text, base, Some(recorder))
}

fn read(text: &str, base: Option<&str>, rec: Option<Recorder>) -> (Parsed, Vec<Use>) {
    let mut g = Grammar::new(text, base, rec);
    let prolog = xml::Prolog::read(text);
    let mut scanner = Scanner::new(text, &prolog);
    loop {
        match scanner.next() {
            Event::Eof => break,
            Event::Start(tag) => {
                g.open_scope(&tag);
                g.start(&tag);
            }
            Event::End(end) => {
                g.end(end);
                if let Some(r) = &mut g.rec {
                    r.scopes.pop();
                }
            }
            Event::Text { span, text } => g.text(span, &text),
            Event::Pi(span, text) => g.pi(span, text),
        }
    }
    let (errors, comments) = scanner.finish();
    let uses = g.rec.as_mut().map(|r| std::mem::take(&mut r.uses));
    (g.finish(errors, comments), uses.unwrap_or_default())
}

pub(super) type Base = Option<Rc<oxiri::Iri<String>>>;

/// Which uses [`uses`] records.
#[derive(Clone, Copy)]
pub(super) enum Want {
    /// Those of this IRI.
    Iri(Iri),
    /// Those whose span holds this offset, and the `xmlns` declaration whose
    /// name does.
    At(u32),
}

/// A place where the text names an IRI, as the grammar reads it: what a
/// rename edits there, or why it cannot.
pub(super) struct Use {
    pub(super) iri: Iri,
    /// The value (inside its quotes) or the name.
    pub(super) span: Span,
    pub(super) kind: UseKind,
    /// The namespaces in scope there, by prefix (`""` for the default
    /// namespace), sorted by prefix.
    pub(super) scope: Vec<(String, Rc<str>)>,
}

pub(super) enum UseKind {
    /// An attribute value read as an IRI: `rdf:about`, `rdf:resource`,
    /// `rdf:datatype`, `rdf:type`, `rdf:annotation`.
    Value {
        base: Base,
        /// The value, its references expanded.
        text: String,
        refs: Refs,
        lead: Option<(Span, u32)>,
        /// Read from an entity's replacement text: `span` is the reference's.
        entity: bool,
    },
    /// `rdf:ID`.
    Id {
        base: Base,
        refs: Refs,
        entity: bool,
    },
    /// The name of a typed node element or a property element.
    Element {
        /// The namespace of the name.
        ns: Rc<str>,
        prefix_len: Option<u32>,
        entity: bool,
        /// Its end, once read.
        end: Option<End>,
    },
    /// The name of a property attribute.
    Attr {
        ns: Rc<str>,
        prefix_len: Option<u32>,
        entity: bool,
    },
    /// `rdf:li`, which stands for `rdf:_n`.
    Li,
    /// The name of an `rdf:type` attribute.
    TypeAttr,
    /// The name of an `xmlns` or `xmlns:p` attribute (only for
    /// [`Want::At`]).
    Namespace,
}

/// What [`uses`] keeps while it reads.
struct Recorder {
    want: Want,
    uses: Vec<Use>,
    /// The namespace declarations of each open element.
    scopes: Vec<Vec<(String, Rc<str>)>>,
    /// The [`UseKind::Element`] of each state on the grammar's stack.
    ends: Vec<Option<usize>>,
    /// The element use the start tag being read records.
    element: Option<usize>,
}

impl Recorder {
    fn wants(&self, iri: Iri, span: Span) -> bool {
        match self.want {
            Want::Iri(i) => i == iri,
            Want::At(offset) => span.contains(offset),
        }
    }

    /// The namespaces in scope, by prefix; an empty one unbinds its prefix.
    fn scope(&self) -> Vec<(String, Rc<str>)> {
        let mut bound: FxHashMap<&str, &Rc<str>> = FxHashMap::default();
        for (prefix, ns) in self.scopes.iter().flatten() {
            bound.insert(prefix.as_str(), ns);
        }
        let mut out: Vec<(String, Rc<str>)> = bound
            .into_iter()
            .filter(|(_, ns)| !ns.is_empty())
            .map(|(p, ns)| (p.to_string(), ns.clone()))
            .collect();
        out.sort();
        out
    }
}

/// How an IRI attribute value reads `raw`: resolved against `base`, or as
/// it is when it has a scheme and no dot segment.
pub(super) fn resolves(base: &Base, raw: &str) -> bool {
    base.is_some() && (!has_scheme(raw) || raw.contains("/.") || raw.contains(":."))
}

/// The IRI an IRI attribute value `raw` names under `base`; `None` when it
/// does not resolve.
pub(super) fn read_iri(base: &Base, raw: &str) -> Option<String> {
    match base {
        Some(b) if resolves(base, raw) => resolve(b, raw).ok().map(|i| i.into_inner()),
        _ => Some(raw.to_string()),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Version {
    V11,
    V12,
    V12Basic,
}

/// What an element passes to its content.
#[derive(Clone)]
struct Env {
    base: Base,
    lang: Option<Iri>,
    /// Applies to property attributes below.
    dir: Option<Direction>,
    version: Version,
}

/// What every kind of property element keeps until its end.
struct Prop {
    /// The predicate and the span of its name; `None` when the name is no
    /// IRI (reported), which gives no triple.
    p: Option<(Iri, Span)>,
    subject: Node,
    id: Option<(Iri, Span)>,
    annotation: Option<(Iri, Span)>,
    annotation_node: Option<(BlankId, Span)>,
    tag: Span,
}

enum Object {
    None,
    Text(String),
    Node(Node, Span),
}

enum State {
    Rdf {
        env: Env,
    },
    Node {
        env: Env,
        subject: Node,
        start: u32,
        li: u64,
        top: bool,
        /// A `parseType="Resource"` blank node: the triple whose object it
        /// is, and where its content starts.
        patch: Option<(usize, u32)>,
        /// The reference of a nested element's `rdf:about`, while the
        /// element gives no triple of its own: at its end, it only names
        /// the object of its property (or a collection item), as an IRI
        /// written there in Turtle, and is [`Role::Object`].
        names: Option<usize>,
    },
    Prop {
        env: Env,
        prop: Prop,
        object: Object,
        datatype: Option<Iri>,
        /// `its:dir` on this element (text takes no inherited direction).
        dir: Option<Direction>,
    },
    Collection {
        env: Env,
        prop: Prop,
        items: Vec<(Node, Span)>,
    },
    Literal {
        env: Env,
        prop: Prop,
        out: String,
        /// The names of the open elements inside.
        names: Vec<String>,
        /// The namespaces written on the open elements inside.
        written: Written,
    },
    TripleProp {
        env: Env,
        prop: Prop,
        triples: Vec<(Node, Iri, Node)>,
    },
}

impl State {
    fn env(&self) -> &Env {
        match self {
            State::Rdf { env }
            | State::Node { env, .. }
            | State::Prop { env, .. }
            | State::Collection { env, .. }
            | State::Literal { env, .. }
            | State::TripleProp { env, .. } => env,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParseType {
    Default,
    Literal,
    Resource,
    Collection,
    Triple,
    Other,
}

/// A start tag's attributes, sorted by meaning.
#[derive(Default)]
struct Attrs<'t, 'a> {
    lang: Option<&'t Attr<'a>>,
    base: Option<&'t Attr<'a>>,
    dir: Option<&'t Attr<'a>>,
    version: Option<&'t Attr<'a>>,
    id: Option<&'t Attr<'a>>,
    node_id: Option<&'t Attr<'a>>,
    about: Option<&'t Attr<'a>>,
    resource: Option<&'t Attr<'a>>,
    datatype: Option<&'t Attr<'a>>,
    parse_type: Option<&'t Attr<'a>>,
    annotation: Option<&'t Attr<'a>>,
    annotation_node: Option<&'t Attr<'a>>,
    type_: Option<&'t Attr<'a>>,
    props: Vec<(Iri, &'t Attr<'a>)>,
    /// A property attribute whose name gives no IRI (reported): it gives
    /// no triple, but the element still has property attributes.
    refused_prop: bool,
}

struct Grammar<'a> {
    text: &'a str,
    out: Parsed,
    errors: Vec<SyntaxError>,
    stack: Vec<State>,
    doc: Env,
    blanks: u32,
    labels: FxHashMap<String, BlankId>,
    ids: FxHashSet<Iri>,
    /// IRIs checked to be absolute.
    checked: FxHashSet<Iri>,
    /// IRIs that are not: a triple naming one is dropped, as Turtle and
    /// oxrdfxml drop it, and the error says why.
    invalid: FxHashSet<Iri>,
    langs: FxHashSet<Iri>,
    /// The IRIs this parse made, and their bytes: at most `iri_budget`.
    made: FxHashSet<Iri>,
    iri_bytes: usize,
    iri_budget: usize,
    /// The bytes written to XML literals by this parse: at most
    /// `literal_budget` ([`LITERAL_BYTES_PER_BYTE`]).
    literal_bytes: usize,
    literal_budget: usize,
    /// The current top-level statement.
    statement: u32,
    /// How many `parseType="Triple"` elements are open.
    in_triple: u32,
    /// Interned vocabulary.
    v: Vocab,
    /// The uses [`uses`] records; `None` for [`parse`].
    rec: Option<Recorder>,
    /// For each state on the stack, the reference of its element's name,
    /// which its end tag's name repeats.
    names: Vec<Option<usize>>,
}

struct Vocab {
    type_: Iri,
    first: Iri,
    rest: Iri,
    nil: Iri,
    statement: Iri,
    subject: Iri,
    predicate: Iri,
    object: Iri,
    reifies: Iri,
    xml_literal: Iri,
    string: Iri,
    lang_string: Iri,
    dir_lang_string: Iri,
    /// Stands for an IRI past the budget: never absolute, so its triples
    /// are dropped.
    none: Iri,
}

impl Vocab {
    fn new() -> Vocab {
        let rdf = |l: &str| Iri::intern(&format!("{RDF}{l}"));
        Vocab {
            type_: rdf("type"),
            first: rdf("first"),
            rest: rdf("rest"),
            nil: rdf("nil"),
            statement: rdf("Statement"),
            subject: rdf("subject"),
            predicate: rdf("predicate"),
            object: rdf("object"),
            reifies: rdf("reifies"),
            xml_literal: rdf("XMLLiteral"),
            string: Iri::intern(&format!("{XSD}string")),
            lang_string: rdf("langString"),
            dir_lang_string: rdf("dirLangString"),
            none: Iri::intern(""),
        }
    }
}

/// `rdf:` names that cannot name a node element; a property element cannot
/// be `rdf:Description` either.
pub(super) const RESERVED_ELEMENTS: [&str; 11] = [
    "about",
    "aboutEach",
    "aboutEachPrefix",
    "bagID",
    "datatype",
    "ID",
    "li",
    "nodeID",
    "parseType",
    "RDF",
    "resource",
];

impl<'a> Grammar<'a> {
    fn new(text: &'a str, document_base: Option<&str>, rec: Option<Recorder>) -> Grammar<'a> {
        let base = document_base
            .and_then(|b| oxiri::Iri::parse(b.to_string()).ok())
            .map(Rc::new);
        let v = Vocab::new();
        let mut invalid = FxHashSet::default();
        invalid.insert(v.none);
        Grammar {
            text,
            out: Parsed {
                syntax: Syntax::RdfXml,
                document_base: document_base.map(str::to_string),
                ..Parsed::default()
            },
            errors: Vec::new(),
            stack: Vec::new(),
            doc: Env {
                base,
                lang: None,
                dir: None,
                version: Version::V11,
            },
            blanks: 0,
            labels: FxHashMap::default(),
            ids: FxHashSet::default(),
            checked: FxHashSet::default(),
            invalid,
            langs: FxHashSet::default(),
            made: FxHashSet::default(),
            iri_bytes: 0,
            iri_budget: text.len().saturating_mul(IRI_BYTES_PER_BYTE) + MIN_IRI_BYTES,
            literal_bytes: 0,
            literal_budget: text
                .len()
                .saturating_mul(LITERAL_BYTES_PER_BYTE)
                .saturating_add(MIN_LITERAL_BYTES),
            statement: 0,
            in_triple: 0,
            v,
            rec,
            names: Vec::new(),
        }
    }

    /// Enter a start tag's namespace declarations, when recording.
    fn open_scope(&mut self, tag: &Tag<'_>) {
        let Some(r) = &mut self.rec else { return };
        let mut decls = Vec::new();
        for attr in &tag.attrs {
            let raw = attr.name.raw;
            let prefix = match raw.strip_prefix("xmlns") {
                Some("") => "",
                Some(p) if p.starts_with(':') => &p[1..],
                _ => continue,
            };
            if let Want::At(offset) = r.want
                && !attr.name.entity
                && attr.name.span.contains(offset)
            {
                r.uses.push(Use {
                    iri: self.v.none,
                    span: attr.name.span,
                    kind: UseKind::Namespace,
                    scope: Vec::new(),
                });
            }
            decls.push((prefix.to_string(), Rc::from(&*attr.value.text)));
        }
        r.scopes.push(decls);
    }

    /// Record a use of `iri` at `span`, when it is wanted; its index.
    fn record(&mut self, iri: Iri, span: Span, kind: impl FnOnce() -> UseKind) -> Option<usize> {
        let r = self.rec.as_mut()?;
        if !r.wants(iri, span) {
            return None;
        }
        let scope = r.scope();
        r.uses.push(Use {
            iri,
            span,
            kind: kind(),
            scope,
        });
        Some(r.uses.len() - 1)
    }

    /// Record the use a name makes of `iri`: an element's or an attribute's.
    fn record_name(&mut self, name: &Name<'_>, iri: Iri, element: bool) {
        let Ns::Bound(ns) = &name.ns else { return };
        let (ns, prefix_len, entity) = (ns.clone(), name.prefix_len, name.entity);
        let i = self.record(iri, name.span, || {
            if element {
                UseKind::Element {
                    ns,
                    prefix_len,
                    entity,
                    end: None,
                }
            } else {
                UseKind::Attr {
                    ns,
                    prefix_len,
                    entity,
                }
            }
        });
        if element && let Some(r) = &mut self.rec {
            r.element = i;
        }
    }

    fn finish(mut self, scanner_errors: Vec<SyntaxError>, comments: Vec<Span>) -> Parsed {
        let mut errors = scanner_errors;
        errors.append(&mut self.errors);
        errors.sort_by_key(|e| e.span.start);
        errors.truncate(xml::MAX_ERRORS);
        self.out.errors = errors;
        self.out.comments = comments;
        self.out.refs.sort_by_key(|r| (r.span.start, r.span.end));
        // The names and values read from one entity reference share its
        // span: the first stands for them.
        self.out
            .refs
            .dedup_by(|r, prev| r.span.start < prev.span.end);
        self.out.prefix_uses.sort_by_key(|u| u.span.start);
        self.out
    }

    fn error(&mut self, span: Span, message: impl Into<std::borrow::Cow<'static, str>>) {
        if self.errors.len() < xml::MAX_ERRORS {
            self.errors.push(SyntaxError::new(
                span,
                message,
                Severity::Error,
                "rdfxml-syntax",
            ));
        }
    }

    fn env(&self) -> &Env {
        self.stack.last().map_or(&self.doc, State::env)
    }

    fn blank(&mut self) -> Node {
        self.blanks += 1;
        Node::Blank(BlankId(self.blanks - 1))
    }

    fn labelled(&mut self, label: &str) -> BlankId {
        if let Some(&b) = self.labels.get(label) {
            return b;
        }
        let Node::Blank(b) = self.blank() else {
            unreachable!()
        };
        self.labels.insert(label.to_string(), b);
        b
    }

    fn role(&self, role: Role) -> Role {
        if self.in_triple > 0 {
            Role::Quoted
        } else {
            role
        }
    }

    fn push_ref(&mut self, span: Span, iri: Iri, role: Role, form: Form) {
        let role = self.role(role);
        self.out.refs.push(Ref {
            span,
            iri,
            role,
            form,
        });
    }

    /// Emit a triple; its index in the parse, unless a triple term takes it.
    fn emit(&mut self, s: Node, p: Iri, o: Node, p_span: Span, o_span: Span) -> Option<usize> {
        if self.names_invalid(&s) || self.invalid.contains(&p) || self.names_invalid(&o) {
            return None;
        }
        if self.in_triple > 0 {
            for state in self.stack.iter_mut().rev() {
                if let State::TripleProp { triples, .. } = state {
                    triples.push((s, p, o));
                    return None;
                }
            }
        }
        self.out.triples.push(Triple {
            s,
            p,
            o,
            p_span,
            o_span,
            statement: self.statement,
            graph: None,
            unresolved: false,
        });
        Some(self.out.triples.len() - 1)
    }

    /// Check that `iri` is an absolute IRI (once per IRI).
    fn check_absolute(&mut self, iri: Iri, span: Span) {
        if self.checked.contains(&iri) {
            return;
        }
        match oxiri::Iri::parse(iri.as_str()) {
            Ok(_) => {
                self.checked.insert(iri);
            }
            Err(e) => {
                self.invalid.insert(iri);
                self.error(span, format!("not an absolute IRI: {e}"));
            }
        }
    }

    /// Whether `n` is, or a triple term in it names, an IRI that is not
    /// absolute.
    fn names_invalid(&self, n: &Node) -> bool {
        match n {
            Node::Iri(i) => self.invalid.contains(i),
            Node::Triple(t) => {
                self.names_invalid(&t.0) || self.invalid.contains(&t.1) || self.names_invalid(&t.2)
            }
            Node::Literal { datatype, .. } => self.invalid.contains(datatype),
            Node::Blank(_) => false,
        }
    }

    /// The interned IRI `s`, unless it is new to this parse and past the
    /// budget ([`IRI_BYTES_PER_BYTE`]): then `None`, reported once at
    /// `span`.
    fn intern(&mut self, s: &str, span: Span) -> Option<Iri> {
        if self.iri_bytes + s.len() <= self.iri_budget {
            let iri = Iri::intern(s);
            if self.made.insert(iri) {
                self.iri_bytes += s.len();
            }
            return Some(iri);
        }
        if let Some(iri) = Iri::get(s).filter(|i| self.made.contains(i)) {
            return Some(iri);
        }
        if self.iri_bytes <= self.iri_budget {
            // Reported once.
            self.iri_bytes = self.iri_budget + 1;
            self.error(
                span,
                format!(
                    "the IRIs are over {IRI_BYTES_PER_BYTE} bytes per byte of text: the triples of the rest are dropped"
                ),
            );
        }
        None
    }

    /// `s` interned as an IRI that is not one: its triples are dropped.
    fn intern_invalid(&mut self, s: &str, span: Span) -> Iri {
        let iri = self.intern(s, span).unwrap_or(self.v.none);
        self.invalid.insert(iri);
        iri
    }

    /// An attribute value as an IRI, resolved against `base`, with its
    /// reference recorded. An IRI that does not resolve, or is not
    /// absolute, is reported and gives no triple.
    fn iri_value(&mut self, base: &Base, attr: &Attr<'_>, role: Role) -> Iri {
        let v = &attr.value;
        let raw: &str = &v.text;
        let iri = match base {
            Some(b) if resolves(base, raw) => match resolve(b, raw) {
                Ok(r) => self.intern(r.as_str(), v.span),
                Err(e) => {
                    self.error(v.span, format!("not an IRI: {e}"));
                    Some(self.intern_invalid(raw, v.span))
                }
            },
            _ => self
                .intern(raw, v.span)
                .inspect(|&iri| self.check_absolute(iri, v.span)),
        };
        let Some(iri) = iri.filter(|&i| i != self.v.none) else {
            return self.v.none;
        };
        let form = if v.refs == Refs::None {
            Form::Iri
        } else {
            Form::Anchor
        };
        self.push_ref(v.span, iri, role, form);
        self.record(iri, v.span, || UseKind::Value {
            base: base.clone(),
            text: v.text.to_string(),
            refs: v.refs,
            lead: v.lead,
            entity: attr.name.entity,
        });
        iri
    }

    /// The IRI a name stands for (namespace and local name), reported when
    /// it has none. Records no reference.
    fn name_iri(&mut self, name: &Name<'_>) -> Option<Iri> {
        match &name.ns {
            Ns::Bound(ns) => {
                let iri = self.intern(&format!("{ns}{}", name.local()), name.span)?;
                self.check_absolute(iri, name.span);
                Some(iri)
            }
            Ns::Unbound => {
                self.error(
                    name.span,
                    format!("{} is in no namespace: RDF/XML needs one", name.raw),
                );
                None
            }
            Ns::Unknown => {
                let prefix = name.prefix().unwrap_or("");
                if !name.entity {
                    self.out.prefix_uses.push(PrefixUse {
                        name: Iri::intern(prefix),
                        span: name.span,
                        resolved: false,
                    });
                }
                self.error(name.span, format!("the prefix {prefix}: is not declared"));
                None
            }
        }
    }

    /// Record the reference a name makes to `iri`.
    fn name_ref(&mut self, name: &Name<'_>, iri: Iri, role: Role) {
        match name.prefix_len {
            // A name read from an entity is not at its span.
            Some(prefix_len) if !name.entity => {
                self.out.prefix_uses.push(PrefixUse {
                    name: Iri::intern(name.prefix().unwrap_or("")),
                    span: name.span,
                    resolved: true,
                });
                self.push_ref(name.span, iri, role, Form::PName { prefix_len });
            }
            _ => self.push_ref(name.span, iri, role, Form::Anchor),
        }
    }

    /// Sort a tag's attributes, reporting those RDF/XML does not allow.
    fn attrs<'t>(&mut self, tag: &'t Tag<'a>) -> Attrs<'t, 'a> {
        let mut a = Attrs::default();
        for attr in &tag.attrs {
            let n = &attr.name;
            if n.ns() == Some(xml::XML_NS) {
                match n.local() {
                    "lang" => a.lang = Some(attr),
                    "base" => a.base = Some(attr),
                    _ => {}
                }
                continue;
            }
            if n.raw.starts_with("xml") {
                // `xmlns`, `xmlns:p` and other names starting with `xml`.
                continue;
            }
            if n.is(ITS, "dir") {
                a.dir = Some(attr);
                continue;
            }
            if n.is(ITS, "version") {
                continue;
            }
            if n.ns() == Some(RDF) {
                let slot = match n.local() {
                    "ID" => Some(&mut a.id),
                    "nodeID" => Some(&mut a.node_id),
                    "about" => Some(&mut a.about),
                    "resource" => Some(&mut a.resource),
                    "datatype" => Some(&mut a.datatype),
                    "parseType" => Some(&mut a.parse_type),
                    "version" => Some(&mut a.version),
                    "annotation" => Some(&mut a.annotation),
                    "annotationNodeID" => Some(&mut a.annotation_node),
                    "type" => Some(&mut a.type_),
                    // `oldTerms` (6.2.4) are withdrawn: no attribute.
                    "aboutEach" | "aboutEachPrefix" | "bagID" | "li" | "RDF" => {
                        self.error(
                            n.span,
                            format!("rdf:{} is not an RDF/XML attribute", n.local()),
                        );
                        continue;
                    }
                    _ => None,
                };
                if let Some(slot) = slot {
                    *slot = Some(attr);
                    continue;
                }
            }
            let Some(iri) = self.name_iri(n) else {
                a.refused_prop = true;
                continue;
            };
            a.props.push((iri, attr));
        }
        a
    }

    /// The environment of an element with these attributes.
    fn env_of(&mut self, a: &Attrs<'_, '_>) -> Env {
        let mut env = self.env().clone();
        if let Some(attr) = a.base {
            let raw: &str = &attr.value.text;
            let resolved = match &env.base {
                Some(b) => resolve(b, raw).map_err(|e| e.to_string()),
                None => oxiri::Iri::parse(raw.to_string()).map_err(|e| e.to_string()),
            };
            match resolved {
                Ok(b) => env.base = Some(Rc::new(b)),
                Err(e) => self.error(attr.value.span, format!("xml:base is not an IRI: {e}")),
            }
        }
        if let Some(attr) = a.lang {
            let tag = attr.value.text.to_ascii_lowercase();
            env.lang = if tag.is_empty() {
                None
            } else {
                let iri = Iri::intern(&tag);
                if !self.langs.contains(&iri) {
                    match oxilangtag::LanguageTag::parse(tag.as_str()) {
                        Ok(_) => {
                            self.langs.insert(iri);
                        }
                        Err(e) => {
                            self.error(attr.value.span, format!("not a language tag: {e}"));
                        }
                    }
                }
                Some(iri)
            };
        }
        if let Some(attr) = a.version {
            match &*attr.value.text {
                "1.1" => env.version = Version::V11,
                "1.2" => env.version = Version::V12,
                "1.2-basic" => env.version = Version::V12Basic,
                v => self.error(
                    attr.value.span,
                    format!("rdf:version {v:?} is not 1.1, 1.2 or 1.2-basic"),
                ),
            }
        }
        env
    }

    /// `its:dir` on this element, when the version in scope reads it.
    fn own_dir(&mut self, a: &Attrs<'_, '_>, env: &Env) -> Option<Direction> {
        let attr = a.dir?;
        if env.version == Version::V11 {
            return None;
        }
        let d = Direction::parse(&attr.value.text);
        if d.is_none() {
            self.error(attr.value.span, "its:dir must be ltr or rtl");
        }
        d
    }

    fn literal(&self, value: &str, lang: Option<Iri>, dir: Option<Direction>) -> Node {
        let (datatype, dir) = match (lang, dir) {
            (Some(_), Some(d)) => (self.v.dir_lang_string, Some(d)),
            (Some(_), None) => (self.v.lang_string, None),
            (None, _) => (self.v.string, None),
        };
        Node::Literal {
            value: value.into(),
            datatype,
            lang,
            dir,
        }
    }

    fn node_id(&mut self, attr: &Attr<'_>) -> BlankId {
        if !xml::is_nc_name(&attr.value.text) {
            self.error(
                attr.value.span,
                format!("{} must be an XML NCName", attr.name.raw),
            );
        }
        self.labelled(&attr.value.text)
    }

    /// `rdf:ID`: the base with the value as fragment, used once.
    fn id(&mut self, attr: &Attr<'_>, base: &Base) -> Iri {
        let v = &attr.value;
        if !xml::is_nc_name(&v.text) {
            self.error(v.span, "rdf:ID must be an XML NCName");
        }
        let fragment = format!("#{}", v.text);
        let iri = match base {
            Some(b) => match resolve(b, &fragment) {
                Ok(r) => self.intern(r.as_str(), v.span).unwrap_or(self.v.none),
                Err(e) => {
                    self.error(v.span, format!("not an IRI: {e}"));
                    self.intern_invalid(&fragment, v.span)
                }
            },
            None => {
                self.error(v.span, "rdf:ID needs a base IRI");
                self.intern_invalid(&fragment, v.span)
            }
        };
        if iri == self.v.none {
            return iri;
        }
        if !self.ids.insert(iri) {
            self.error(v.span, format!("rdf:ID {} is used twice", v.text));
        }
        self.push_ref(v.span, iri, Role::Subject, Form::Anchor);
        self.record(iri, v.span, || UseKind::Id {
            base: base.clone(),
            refs: v.refs,
            entity: attr.name.entity,
        });
        iri
    }

    /// Property attributes and `rdf:type` on `subject`.
    fn property_attrs(&mut self, subject: &Node, a: &Attrs<'_, '_>, env: &Env) {
        for &(p, attr) in &a.props {
            self.name_ref(&attr.name, p, Role::Predicate);
            self.record_name(&attr.name, p, false);
            let o = self.literal(&attr.value.text, env.lang, env.dir);
            self.emit(subject.clone(), p, o, attr.name.span, attr.value.span);
        }
        if let Some(attr) = a.type_ {
            self.name_ref(&attr.name, self.v.type_, Role::Predicate);
            self.record(self.v.type_, attr.name.span, || UseKind::TypeAttr);
            let o = self.iri_value(&env.base, attr, Role::Object);
            self.emit(
                subject.clone(),
                self.v.type_,
                Node::Iri(o),
                attr.name.span,
                attr.value.span,
            );
        }
    }

    fn start(&mut self, tag: &Tag<'a>) {
        if let Some(State::Literal { .. }) = self.stack.last() {
            self.literal_start(tag);
            return;
        }
        let a = self.attrs(tag);
        let env = self.env_of(&a);
        if let Some(r) = &mut self.rec {
            r.element = None;
        }
        let refs_before = self.out.refs.len();
        match self.stack.last() {
            None if tag.name.is(RDF, "RDF") => {
                let dir = self.own_dir(&a, &env);
                let env = Env {
                    dir: dir.or(env.dir),
                    ..env
                };
                self.stack.push(State::Rdf { env });
            }
            None | Some(State::Rdf { .. }) => self.node_elt(tag, &a, env, true),
            Some(State::Node { .. }) => self.prop_elt(tag, &a, env),
            Some(State::Prop { .. } | State::Collection { .. } | State::TripleProp { .. }) => {
                if let Some(State::Prop { object, .. }) = self.stack.last() {
                    let taken = match object {
                        Object::Node(..) => true,
                        Object::Text(t) => !xml::is_whitespace(t),
                        Object::None => false,
                    };
                    if taken {
                        self.error(
                            tag.name.span,
                            "a property element holds one object: text, a resource or one node element",
                        );
                    }
                }
                self.node_elt(tag, &a, env, false)
            }
            Some(State::Literal { .. }) => unreachable!("read by literal_start"),
        }
        // One state was pushed: its element use, if recorded, and its
        // name's reference, when the document writes the name.
        if let Some(r) = &mut self.rec {
            r.ends.push(r.element.take());
        }
        let name = (!tag.name.entity)
            .then(|| {
                (refs_before..self.out.refs.len()).find(|&i| self.out.refs[i].span == tag.name.span)
            })
            .flatten();
        self.names.push(name);
    }

    fn node_elt(&mut self, tag: &Tag<'a>, a: &Attrs<'_, 'a>, env: Env, top: bool) {
        let dir = self.own_dir(a, &env);
        let env = Env {
            dir: dir.or(env.dir),
            ..env
        };
        let name = &tag.name;
        let type_ = if name.is(RDF, "Description") {
            None
        } else if name.ns() == Some(RDF) && RESERVED_ELEMENTS.contains(&name.local()) {
            self.error(
                name.span,
                format!("rdf:{} cannot name a node element", name.local()),
            );
            None
        } else {
            self.name_iri(name)
        };
        for (x, y, what) in [
            (a.id, a.node_id, "rdf:ID and rdf:nodeID"),
            (a.about, a.node_id, "rdf:about and rdf:nodeID"),
            (a.about, a.id, "rdf:about and rdf:ID"),
        ] {
            if let (Some(_), Some(y)) = (x, y) {
                self.error(y.name.span, format!("{what} cannot be on one node element"));
            }
        }
        let subject_span;
        let mut names = None;
        let subject = if let Some(attr) = a.about {
            subject_span = attr.value.span;
            let refs = self.out.refs.len();
            let iri = self.iri_value(&env.base, attr, Role::Subject);
            let own = !a.props.is_empty() || a.type_.is_some() || type_.is_some();
            if !top && !own && self.out.refs.len() > refs {
                names = Some(refs);
            }
            Node::Iri(iri)
        } else if let Some(attr) = a.id {
            subject_span = attr.value.span;
            Node::Iri(self.id(attr, &env.base))
        } else if let Some(attr) = a.node_id {
            subject_span = attr.value.span;
            Node::Blank(self.node_id(attr))
        } else {
            subject_span = name.span;
            self.blank()
        };
        // The node elements of one entity reference share its span, and one
        // statement.
        let shared = self
            .out
            .statements
            .last()
            .is_some_and(|st| st.span.start == tag.span.start);
        if top && !shared {
            self.out.statements.push(Statement {
                span: tag.span,
                subject: match &subject {
                    Node::Iri(i) if *i != self.v.none => Some((*i, subject_span)),
                    _ => None,
                },
                graph: None,
            });
            self.statement = self.out.statements.len() as u32 - 1;
        }
        self.property_attrs(&subject, a, &env);
        if let Some(t) = type_ {
            self.name_ref(name, t, Role::Object);
            self.record_name(name, t, true);
            self.emit(
                subject.clone(),
                self.v.type_,
                Node::Iri(t),
                name.span,
                name.span,
            );
        }
        self.stack.push(State::Node {
            env,
            subject,
            start: tag.span.start,
            li: 0,
            top,
            patch: None,
            names,
        });
    }

    fn prop_elt(&mut self, tag: &Tag<'a>, a: &Attrs<'_, 'a>, env: Env) {
        let dir = self.own_dir(a, &env);
        let env = Env {
            dir: dir.or(env.dir),
            ..env
        };
        let name = &tag.name;
        let Some(State::Node {
            subject, li, names, ..
        }) = self.stack.last_mut()
        else {
            unreachable!("a property element is in a node element")
        };
        // It gives a triple of its own: `rdf:about` names its subject.
        *names = None;
        let subject = subject.clone();
        let p = if name.is(RDF, "li") {
            *li += 1;
            let li = format!("{RDF}_{li}");
            self.intern(&li, name.span).map(|iri| {
                self.push_ref(name.span, iri, Role::Predicate, Form::Anchor);
                self.record(iri, name.span, || UseKind::Li);
                (iri, name.span)
            })
        } else if name.ns() == Some(RDF)
            && (RESERVED_ELEMENTS.contains(&name.local()) || name.local() == "Description")
        {
            self.error(
                name.span,
                format!("rdf:{} cannot name a property element", name.local()),
            );
            None
        } else {
            self.name_iri(name).map(|iri| {
                self.name_ref(name, iri, Role::Predicate);
                self.record_name(name, iri, true);
                (iri, name.span)
            })
        };
        let id = a.id.map(|attr| (self.id(attr, &env.base), attr.value.span));
        let parse_type = match a.parse_type.map(|t| &*t.value.text) {
            None => ParseType::Default,
            Some("Literal") => ParseType::Literal,
            Some("Resource") => ParseType::Resource,
            Some("Collection") => ParseType::Collection,
            Some("Triple") => ParseType::Triple,
            Some(_) => ParseType::Other,
        };
        // Its IRI is written, so it is a reference even where refused.
        let mut annotation = a.annotation.map(|attr| {
            (
                self.iri_value(&env.base, attr, Role::Subject),
                attr.value.span,
            )
        });
        // At most one of the two (`(annotationAttr | annotationNodeIDAttr)?`),
        // and neither on a collection (6.2.20).
        let mut annotation_node = a.annotation_node;
        if let (Some(_), Some(attr)) = (a.annotation, annotation_node) {
            self.error(
                attr.name.span,
                "rdf:annotation and rdf:annotationNodeID cannot be on one property element",
            );
            annotation_node = None;
        }
        if parse_type == ParseType::Collection {
            for (attr, what) in [
                (a.annotation, "rdf:annotation"),
                (annotation_node.take(), "rdf:annotationNodeID"),
            ] {
                if let Some(attr) = attr {
                    self.error(
                        attr.name.span,
                        format!(
                            "{what} cannot be on a property element with rdf:parseType=\"Collection\""
                        ),
                    );
                }
            }
            annotation = None;
        }
        let annotation_node = annotation_node.map(|attr| (self.node_id(attr), attr.value.span));
        if let Some(attr) = a.about {
            self.error(attr.name.span, "rdf:about cannot be on a property element");
        }
        let prop = Prop {
            p,
            subject,
            id,
            annotation,
            annotation_node,
            tag: tag.span,
        };
        if parse_type != ParseType::Default {
            for (attr, what) in [
                (a.resource, "rdf:resource"),
                (a.node_id, "rdf:nodeID"),
                (a.datatype, "rdf:datatype"),
                (a.type_, "rdf:type"),
            ] {
                if let Some(attr) = attr {
                    self.error(
                        attr.name.span,
                        format!("{what} cannot be on a property element with rdf:parseType"),
                    );
                }
            }
            if let Some((_, attr)) = a.props.first() {
                self.error(
                    attr.name.span,
                    "property attributes cannot be on a property element with rdf:parseType",
                );
            }
        }
        let state = match parse_type {
            ParseType::Default => {
                let datatype = a
                    .datatype
                    .map(|attr| self.iri_value(&env.base, attr, Role::Datatype));
                // emptyPropertyElt (RDF/XML 7.2.21): rdf:type is a
                // property attribute too, and a refused one is still
                // written: the object is the blank node, not "".
                let attributes = !a.props.is_empty() || a.type_.is_some() || a.refused_prop;
                if a.resource.is_some() || a.node_id.is_some() || attributes {
                    if let (Some(_), Some(n)) = (a.resource, a.node_id) {
                        self.error(
                            n.name.span,
                            "rdf:resource and rdf:nodeID cannot be on one property element",
                        );
                    }
                    let (object, span) = if let Some(attr) = a.resource {
                        let iri = self.iri_value(&env.base, attr, Role::Object);
                        (Node::Iri(iri), attr.value.span)
                    } else if let Some(attr) = a.node_id {
                        (Node::Blank(self.node_id(attr)), attr.value.span)
                    } else {
                        (self.blank(), tag.span)
                    };
                    self.property_attrs(&object, a, &env);
                    State::Prop {
                        env,
                        prop,
                        object: Object::Node(object, span),
                        datatype,
                        dir,
                    }
                } else {
                    State::Prop {
                        env,
                        prop,
                        object: Object::None,
                        datatype,
                        dir,
                    }
                }
            }
            // Any other value reads as "Literal" (RDF/XML 7.2.20).
            ParseType::Literal | ParseType::Other => State::Literal {
                env,
                prop,
                out: String::new(),
                names: Vec::new(),
                written: Written::default(),
            },
            ParseType::Resource => {
                let object = self.blank();
                let patch = self.prop_triple(&prop, object.clone(), tag.span);
                self.stack.push(State::Node {
                    env,
                    subject: object,
                    start: tag.span.start,
                    li: 0,
                    top: false,
                    patch: patch.map(|i| (i, tag.span.end)),
                    names: None,
                });
                return;
            }
            ParseType::Collection => State::Collection {
                env,
                prop,
                items: Vec::new(),
            },
            ParseType::Triple => {
                self.in_triple += 1;
                State::TripleProp {
                    env,
                    prop,
                    triples: Vec::new(),
                }
            }
        };
        self.stack.push(state);
    }

    /// The triple of a property element, with its reification and
    /// annotations; its index, as [`Grammar::emit`].
    fn prop_triple(&mut self, prop: &Prop, o: Node, o_span: Span) -> Option<usize> {
        let (p, p_span) = prop.p?;
        let s = prop.subject.clone();
        if let Some((id, span)) = prop.id {
            let id = Node::Iri(id);
            let v = &self.v;
            let (t, st, su, pr, ob) = (v.type_, v.statement, v.subject, v.predicate, v.object);
            self.emit(id.clone(), t, Node::Iri(st), span, span);
            self.emit(id.clone(), su, s.clone(), span, span);
            self.emit(id.clone(), pr, Node::Iri(p), span, span);
            self.emit(id, ob, o.clone(), span, span);
        }
        let triple = || Node::Triple(Box::new((s.clone(), p, o.clone())));
        if let Some((a, span)) = prop.annotation {
            self.emit(Node::Iri(a), self.v.reifies, triple(), span, span);
        }
        if let Some((b, span)) = prop.annotation_node {
            self.emit(Node::Blank(b), self.v.reifies, triple(), span, span);
        }
        self.emit(s.clone(), p, o, p_span, o_span)
    }

    fn end(&mut self, end: End) {
        let spent = self.literal_spent();
        if let Some(State::Literal {
            names,
            out,
            written,
            ..
        }) = self.stack.last_mut()
            && let Some(name) = names.pop()
        {
            written.close(names.len() + 1);
            if !spent {
                let before = out.len();
                out.push_str("</");
                out.push_str(&name);
                out.push('>');
                let n = out.len() - before;
                self.charge_literal(n, end.span);
            }
            return;
        }
        let Some(state) = self.stack.pop() else {
            return;
        };
        // The end tag's name names the element's IRI again.
        if let Some(Some(i)) = self.names.pop()
            && let Some(span) = end.name.filter(|_| !end.implied)
        {
            let start = &self.out.refs[i];
            let same = |s: Span| &self.text[s.start as usize..s.end as usize];
            if same(span) == same(start.span) {
                let r = Ref { span, ..*start };
                self.out.refs.push(r);
            }
        }
        if let Some(r) = &mut self.rec
            && let Some(Some(i)) = r.ends.pop()
            && let UseKind::Element { end: e, .. } = &mut r.uses[i].kind
        {
            *e = Some(end);
        }
        // The content: from the start tag's end to the end tag; an empty
        // element's is its tag.
        let content = |tag: Span| {
            if end.span == tag {
                tag
            } else {
                Span::new(tag.end, end.span.start.max(tag.end))
            }
        };
        match state {
            State::Rdf { .. } => {}
            State::Node {
                subject,
                start,
                top,
                patch,
                names,
                ..
            } => {
                let span = Span::new(start, end.span.end);
                if let Some(r) = names.and_then(|i| self.out.refs.get_mut(i))
                    && r.role == Role::Subject
                {
                    r.role = Role::Object;
                }
                if top && let Some(st) = self.out.statements.get_mut(self.statement as usize) {
                    st.span.end = end.span.end;
                }
                if let Some((i, from)) = patch {
                    self.out.triples[i].o_span = Span::new(from, end.span.start.max(from));
                    return;
                }
                match self.stack.last_mut() {
                    Some(State::Prop { object, .. }) => match object {
                        Object::Node(..) => {}
                        Object::Text(t) if !xml::is_whitespace(t) => {}
                        _ => *object = Object::Node(subject, span),
                    },
                    Some(State::Collection { items, .. }) => items.push((subject, span)),
                    _ => {}
                }
            }
            State::Prop {
                env,
                prop,
                object,
                datatype,
                dir,
            } => {
                if end.implied {
                    return;
                }
                let (o, span) = match object {
                    Object::Node(n, span) => (n, span),
                    Object::Text(t) => (
                        self.text_literal(&t, datatype, &env, dir),
                        content(prop.tag),
                    ),
                    Object::None => (
                        self.text_literal("", datatype, &env, dir),
                        content(prop.tag),
                    ),
                };
                self.prop_triple(&prop, o, span);
            }
            State::Collection { prop, items, .. } => {
                if end.implied || prop.p.is_none() {
                    return;
                }
                let cells: Vec<Node> = items.iter().map(|_| self.blank()).collect();
                for (i, (item, span)) in items.iter().enumerate() {
                    let rest = cells.get(i + 1).cloned().unwrap_or(Node::Iri(self.v.nil));
                    self.emit(cells[i].clone(), self.v.first, item.clone(), *span, *span);
                    self.emit(cells[i].clone(), self.v.rest, rest, *span, *span);
                }
                let head = cells.first().cloned().unwrap_or(Node::Iri(self.v.nil));
                self.prop_triple(&prop, head, content(prop.tag));
            }
            State::Literal { prop, out, .. } => {
                if end.implied || self.literal_spent() {
                    return;
                }
                let o = Node::Literal {
                    value: out.into(),
                    datatype: self.v.xml_literal,
                    lang: None,
                    dir: None,
                };
                self.prop_triple(&prop, o, content(prop.tag));
            }
            State::TripleProp { prop, triples, .. } => {
                self.in_triple -= 1;
                if end.implied {
                    return;
                }
                if triples.len() != 1 {
                    self.error(
                        content(prop.tag),
                        "rdf:parseType=\"Triple\" holds exactly one triple",
                    );
                    return;
                }
                let (s, p, o) = triples.into_iter().next().expect("one triple");
                self.prop_triple(&prop, Node::Triple(Box::new((s, p, o))), content(prop.tag));
            }
        }
    }

    fn text_literal(
        &self,
        text: &str,
        datatype: Option<Iri>,
        env: &Env,
        dir: Option<Direction>,
    ) -> Node {
        match datatype {
            Some(d) => Node::Literal {
                value: text.into(),
                datatype: d,
                lang: None,
                dir: None,
            },
            None => self.literal(text, env.lang, dir),
        }
    }

    fn text(&mut self, span: Span, text: &str) {
        let spent = self.literal_spent();
        match self.stack.last_mut() {
            Some(State::Prop { object, .. }) => match object {
                Object::Node(..) => {
                    if !xml::is_whitespace(text) {
                        self.error(span, "a property element holds one object: text, a resource or one node element");
                    }
                }
                Object::Text(t) => t.push_str(text),
                Object::None => *object = Object::Text(text.to_string()),
            },
            Some(State::Literal { out, .. }) => {
                if !spent {
                    let before = out.len();
                    escape_text(out, text);
                    let n = out.len() - before;
                    self.charge_literal(n, span);
                }
            }
            _ => {
                if !xml::is_whitespace(text) {
                    // The white space the source text starts with: an
                    // entity's expansion is not in it.
                    let raw = &self.text[span.start as usize..span.end as usize];
                    let lead = raw.len() - raw.trim_start().len();
                    let at = Span::new(span.start + lead as u32, span.end);
                    self.error(at, "text where RDF/XML has only elements");
                }
            }
        }
    }

    fn pi(&mut self, span: Span, text: &str) {
        let spent = self.literal_spent();
        if let Some(State::Literal { out, .. }) = self.stack.last_mut()
            && !spent
        {
            let before = out.len();
            // Line ends normalized (XML 1.0, 2.11).
            out.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
            let n = out.len() - before;
            self.charge_literal(n, span);
        }
    }

    /// Whether the XML literal budget ([`LITERAL_BYTES_PER_BYTE`]) is
    /// spent: then nothing more is written to a literal, and a literal
    /// that ends gives no triple.
    fn literal_spent(&self) -> bool {
        self.literal_bytes > self.literal_budget
    }

    /// Charge `n` bytes just written to the open XML literal, at `span`.
    /// Past the budget, reported once there; the literal's text is let go.
    fn charge_literal(&mut self, n: usize, span: Span) {
        let was_spent = self.literal_spent();
        self.literal_bytes = self.literal_bytes.saturating_add(n);
        if was_spent || !self.literal_spent() {
            return;
        }
        if let Some(State::Literal { out, .. }) = self.stack.last_mut() {
            *out = String::new();
        }
        self.error(
            span,
            format!(
                "the XML literals are over {LITERAL_BYTES_PER_BYTE} bytes per byte of text: this one and the ones after it give no triple"
            ),
        );
    }

    /// A start tag inside `rdf:parseType="Literal"`, in exclusive canonical
    /// XML: the namespaces it uses that are not written above it (sorted by
    /// prefix), then its attributes sorted by namespace and local name.
    fn literal_start(&mut self, tag: &Tag<'a>) {
        let spent = self.literal_spent();
        let mut unknown = Vec::new();
        let Some(State::Literal {
            out,
            names,
            written,
            ..
        }) = self.stack.last_mut()
        else {
            unreachable!()
        };
        if spent {
            // Nothing is written; the name only keeps the depth.
            names.push(String::new());
            return;
        }
        let before = out.len();
        let depth = names.len() + 1;
        let mut decls: FxHashMap<String, String> = FxHashMap::default();
        let mut used = |name: &Name<'_>, element: bool, decls: &mut FxHashMap<String, String>| {
            let prefix = match name.prefix() {
                Some("xml") => return,
                Some(p) => p,
                None if element => "",
                None => return,
            };
            let uri: &str = match &name.ns {
                Ns::Bound(u) => u,
                Ns::Unbound => "",
                Ns::Unknown => {
                    unknown.push(name.span);
                    return;
                }
            };
            let needed = match written.get(prefix) {
                Some(a) => *a != uri,
                None => !uri.is_empty(),
            };
            if needed {
                decls
                    .entry(prefix.to_string())
                    .or_insert_with(|| uri.to_string());
            }
        };
        used(&tag.name, true, &mut decls);
        let attrs: Vec<&Attr<'_>> = tag
            .attrs
            .iter()
            .filter(|a| a.name.raw != "xmlns" && !a.name.raw.starts_with("xmlns:"))
            .collect();
        for a in &attrs {
            used(&a.name, false, &mut decls);
        }
        let mut decls: Vec<(String, String)> = decls.into_iter().collect();
        decls.sort();
        out.push('<');
        out.push_str(tag.name.raw);
        for (p, uri) in &decls {
            out.push_str(if p.is_empty() { " xmlns" } else { " xmlns:" });
            out.push_str(p);
            out.push_str("=\"");
            escape_attr(out, uri);
            out.push('"');
            written.open(depth, p, uri);
        }
        let mut attrs = attrs;
        attrs.sort_by(|x, y| {
            (x.name.ns().unwrap_or(""), x.name.local())
                .cmp(&(y.name.ns().unwrap_or(""), y.name.local()))
        });
        for a in attrs {
            out.push(' ');
            out.push_str(a.name.raw);
            out.push_str("=\"");
            escape_attr(out, &a.value.text);
            out.push('"');
        }
        out.push('>');
        let n = out.len() - before;
        names.push(tag.name.raw.to_string());
        for span in unknown {
            self.error(span, "the prefix is not declared");
        }
        self.charge_literal(n, tag.name.span);
    }
}

/// The namespaces an XML literal has written on its open elements: per
/// prefix, innermost last, and the order they were written in, so closing
/// an element undoes its own in constant time each.
#[derive(Default)]
struct Written {
    by_prefix: FxHashMap<String, Vec<String>>,
    log: Vec<(usize, String)>,
}

impl Written {
    fn get(&self, prefix: &str) -> Option<&String> {
        self.by_prefix.get(prefix)?.last()
    }

    fn open(&mut self, depth: usize, prefix: &str, uri: &str) {
        self.by_prefix
            .entry(prefix.to_string())
            .or_default()
            .push(uri.to_string());
        self.log.push((depth, prefix.to_string()));
    }

    /// Close the element at `depth`.
    fn close(&mut self, depth: usize) {
        while self.log.last().is_some_and(|l| l.0 >= depth) {
            let (_, prefix) = self.log.pop().expect("not empty");
            if let Some(uris) = self.by_prefix.get_mut(&prefix) {
                uris.pop();
            }
        }
    }
}

/// `raw` resolved against `base`.
fn resolve(
    base: &oxiri::Iri<String>,
    raw: &str,
) -> Result<oxiri::Iri<String>, oxiri::IriParseError> {
    oxiri::IriRef::parse(raw).and_then(|r| base.resolve(&r))
}

/// Text in canonical XML.
fn escape_text(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

/// An attribute value in canonical XML.
fn escape_attr(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\t' => out.push_str("&#x9;"),
            '\n' => out.push_str("&#xA;"),
            '\r' => out.push_str("&#xD;"),
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests;
