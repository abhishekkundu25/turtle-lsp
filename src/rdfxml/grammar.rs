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
//! `rdf:annotation`, `rdf:annotationNodeID` and `its:dir`, which, as in
//! `oxrdfxml`, only `rdf:version="1.2"` (or `1.2-basic`) turns on:
//! - `its:dir` on a property element gives its text that direction; on any
//!   element it gives the direction of the property attributes below it;
//! - `rdf:parseType="Triple"` gives a triple term only under version 1.2,
//!   and nothing otherwise.
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
//! - line ends are normalized (XML 1.0, 2.11) and so are attribute values
//!   (3.3.3: white space becomes a space); `oxrdfxml` keeps `\r\n` in text
//!   and line ends and tabs in attribute values. FOAF has two such values
//!   (`CORPUS_KNOWN`).
//!
//! Recovery: an RDF/XML error (code `rdfxml-syntax`) is reported at the
//! attribute or name that causes it, and the element is read as far as it
//! can be: an element whose name is no IRI gives no triple of its own but
//! its content is read; of two attributes that cannot go together, the
//! first in the order `rdf:about`, `rdf:ID`, `rdf:nodeID` (`rdf:resource`,
//! `rdf:nodeID` on a property element) is kept; an IRI that does not
//! resolve, or is not absolute, is kept as written in its reference, and a
//! triple that names it (as subject, predicate, object or datatype) is
//! dropped, as Turtle and `oxrdfxml` drop it. The IRIs one parse makes are
//! bounded ([`IRI_BYTES_PER_BYTE`]); past that, new ones give no triple. An element the XML scanner closes for it (an
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
//!   document uses;
//! - the comments. No [`PrefixDecl`](crate::syntax::PrefixDecl): XML namespaces are scoped, and a
//!   redeclared prefix is no mistake there;
//! - blank nodes numbered in the order they first appear, so two parses of
//!   one text are equal.

use super::xml::{self, Attr, End, Event, Name, Ns, Refs, Scanner, Tag, Value};
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

/// Parse an RDF/XML document. `base` is the document's URL, against which
/// relative IRIs resolve until an `xml:base` replaces it.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    let mut g = Grammar::new(text, base);
    let prolog = xml::Prolog::read(text);
    let mut scanner = Scanner::new(text, &prolog);
    loop {
        match scanner.next() {
            Event::Eof => break,
            Event::Start(tag) => g.start(&tag),
            Event::End(end) => g.end(end),
            Event::Text { span, text } => g.text(span, &text),
            Event::Pi(_, text) => g.pi(text),
        }
    }
    let (errors, comments) = scanner.finish();
    g.finish(errors, comments)
}

type Base = Option<Rc<oxiri::Iri<String>>>;

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
    /// The current top-level statement.
    statement: u32,
    /// How many `parseType="Triple"` elements are open.
    in_triple: u32,
    /// Interned vocabulary.
    v: Vocab,
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
const RESERVED_ELEMENTS: [&str; 11] = [
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
    fn new(text: &'a str, document_base: Option<&str>) -> Grammar<'a> {
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
            statement: 0,
            in_triple: 0,
            v,
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
    fn iri_value(&mut self, base: &Base, v: &Value<'_>, role: Role) -> Iri {
        let raw: &str = &v.text;
        let iri = match base {
            Some(b) if !has_scheme(raw) || raw.contains("/.") || raw.contains(":.") => {
                match resolve(b, raw) {
                    Ok(r) => self.intern(r.as_str(), v.span),
                    Err(e) => {
                        self.error(v.span, format!("not an IRI: {e}"));
                        Some(self.intern_invalid(raw, v.span))
                    }
                }
            }
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
                    "bagID" => {
                        if !xml::is_nc_name(&attr.value.text) {
                            self.error(attr.value.span, "rdf:bagID must be an XML NCName");
                        }
                        continue;
                    }
                    "aboutEach" | "aboutEachPrefix" | "li" | "RDF" => {
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
        iri
    }

    /// Property attributes and `rdf:type` on `subject`.
    fn property_attrs(&mut self, subject: &Node, a: &Attrs<'_, '_>, env: &Env) {
        for &(p, attr) in &a.props {
            self.name_ref(&attr.name, p, Role::Predicate);
            let o = self.literal(&attr.value.text, env.lang, env.dir);
            self.emit(subject.clone(), p, o, attr.name.span, attr.value.span);
        }
        if let Some(attr) = a.type_ {
            self.name_ref(&attr.name, self.v.type_, Role::Predicate);
            let o = self.iri_value(&env.base, &attr.value, Role::Object);
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
            let iri = self.iri_value(&env.base, &attr.value, Role::Subject);
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
                (iri, name.span)
            })
        };
        let id = a.id.map(|attr| (self.id(attr, &env.base), attr.value.span));
        let annotation = a.annotation.map(|attr| {
            (
                self.iri_value(&env.base, &attr.value, Role::Subject),
                attr.value.span,
            )
        });
        let annotation_node = a
            .annotation_node
            .map(|attr| (self.node_id(attr), attr.value.span));
        if let Some(attr) = a.about {
            self.error(attr.name.span, "rdf:about cannot be on a property element");
        }
        let parse_type = match a.parse_type.map(|t| &*t.value.text) {
            None => ParseType::Default,
            Some("Literal") => ParseType::Literal,
            Some("Resource") => ParseType::Resource,
            Some("Collection") => ParseType::Collection,
            Some("Triple") => ParseType::Triple,
            Some(_) => ParseType::Other,
        };
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
                    .map(|attr| self.iri_value(&env.base, &attr.value, Role::Datatype));
                // emptyPropertyElt (RDF/XML 7.2.21): rdf:type is a
                // property attribute too.
                let attributes = !a.props.is_empty() || a.type_.is_some();
                if a.resource.is_some() || a.node_id.is_some() || attributes {
                    if let (Some(_), Some(n)) = (a.resource, a.node_id) {
                        self.error(
                            n.name.span,
                            "rdf:resource and rdf:nodeID cannot be on one property element",
                        );
                    }
                    let (object, span) = if let Some(attr) = a.resource {
                        let iri = self.iri_value(&env.base, &attr.value, Role::Object);
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
        if let Some(State::Literal {
            names,
            out,
            written,
            ..
        }) = self.stack.last_mut()
            && let Some(name) = names.pop()
        {
            out.push_str("</");
            out.push_str(&name);
            out.push('>');
            written.close(names.len() + 1);
            return;
        }
        let Some(state) = self.stack.pop() else {
            return;
        };
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
                if end.implied {
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
            State::TripleProp { env, prop, triples } => {
                self.in_triple -= 1;
                if end.implied || env.version != Version::V12 {
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
            Some(State::Literal { out, .. }) => escape_text(out, text),
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

    fn pi(&mut self, text: &str) {
        if let Some(State::Literal { out, .. }) = self.stack.last_mut() {
            // Line ends normalized (XML 1.0, 2.11).
            out.push_str(&text.replace("\r\n", "\n").replace('\r', "\n"));
        }
    }

    /// A start tag inside `rdf:parseType="Literal"`, in exclusive canonical
    /// XML: the namespaces it uses that are not written above it (sorted by
    /// prefix), then its attributes sorted by namespace and local name.
    fn literal_start(&mut self, tag: &Tag<'a>) {
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
        let depth = names.len() + 1;
        let mut decls: FxHashMap<String, String> = FxHashMap::default();
        let mut used = |name: &Name<'_>, element: bool, decls: &mut FxHashMap<String, String>| {
            let prefix = match name.prefix() {
                Some("xml") => return,
                Some(p) => p,
                None if element => "",
                None => return,
            };
            let uri = match &name.ns {
                Ns::Bound(u) => u.to_string(),
                Ns::Unbound => String::new(),
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
                decls.entry(prefix.to_string()).or_insert(uri);
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
        names.push(tag.name.raw.to_string());
        for span in unknown {
            self.error(span, "the prefix is not declared");
        }
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
mod tests {
    use super::*;
    use oxrdf::graph::CanonicalizationAlgorithm;
    use oxrdf::{BaseDirection, BlankNode, Graph, Literal, NamedNode, NamedOrBlankNode, Term};
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    include!("../../tests/support/corpus.rs");

    const BASE: &str = "http://example.org/base";

    fn term(n: &Node) -> Term {
        match n {
            Node::Iri(i) => NamedNode::new_unchecked(i.as_str()).into(),
            Node::Blank(b) => BlankNode::new_unchecked(format!("b{}", b.0)).into(),
            Node::Literal {
                value,
                datatype,
                lang,
                dir,
            } => match (lang, dir) {
                (Some(l), Some(d)) => Literal::new_directional_language_tagged_literal_unchecked(
                    &**value,
                    l.as_str(),
                    match d {
                        Direction::Ltr => BaseDirection::Ltr,
                        Direction::Rtl => BaseDirection::Rtl,
                    },
                )
                .into(),
                (Some(l), None) => {
                    Literal::new_language_tagged_literal_unchecked(&**value, l.as_str()).into()
                }
                _ => Literal::new_typed_literal(
                    &**value,
                    NamedNode::new_unchecked(datatype.as_str()),
                )
                .into(),
            },
            Node::Triple(t) => oxrdf::Triple::new(
                subject(&t.0),
                NamedNode::new_unchecked(t.1.as_str()),
                term(&t.2),
            )
            .into(),
        }
    }

    fn subject(n: &Node) -> NamedOrBlankNode {
        match term(n) {
            Term::NamedNode(n) => n.into(),
            Term::BlankNode(b) => b.into(),
            t => panic!("not a subject: {t}"),
        }
    }

    fn canonical(triples: impl IntoIterator<Item = oxrdf::Triple>) -> Graph {
        let mut g = Graph::new();
        for t in triples {
            g.insert(&t);
        }
        g.canonicalize(CanonicalizationAlgorithm::Unstable);
        g
    }

    fn graph(p: &Parsed) -> Graph {
        canonical(p.triples.iter().map(|t| {
            oxrdf::Triple::new(
                subject(&t.s),
                NamedNode::new_unchecked(t.p.as_str()),
                term(&t.o),
            )
        }))
    }

    /// `oxrdfxml`'s graph, or its first error.
    fn theirs(text: &str, base: Option<&str>) -> Result<Graph, String> {
        let mut parser = oxrdfxml::RdfXmlParser::new();
        if let Some(b) = base {
            parser = parser.with_base_iri(b).map_err(|e| e.to_string())?;
        }
        let mut triples = Vec::new();
        for t in parser.for_slice(text) {
            triples.push(t.map_err(|e| e.to_string())?);
        }
        Ok(canonical(triples))
    }

    /// `None` when the graphs are equal, else the triples only one has.
    fn difference(ours: &Graph, theirs: &Graph) -> Option<String> {
        if ours == theirs {
            return None;
        }
        let mut out = format!("ours {} triples, oxrdfxml {}\n", ours.len(), theirs.len());
        for (name, x, y) in [("ours", ours, theirs), ("oxrdfxml", theirs, ours)] {
            for t in x.iter().filter(|t| !y.contains(*t)).take(10) {
                writeln!(out, "  only {name}: {t}").unwrap();
            }
        }
        Some(out)
    }

    /// [`difference`], unless the graphs differ only where `oxrdfxml` keeps
    /// the white space of an attribute value that XML 1.0 (3.3.3) makes
    /// spaces: each triple only `oxrdfxml` has is one only ours has, with
    /// that white space in its literal made spaces.
    fn difference_past_normalization(ours: &Graph, theirs: &Graph) -> Option<String> {
        let only_ours: Vec<_> = ours.iter().filter(|t| !theirs.contains(*t)).collect();
        let only_theirs: Vec<_> = theirs.iter().filter(|t| !ours.contains(*t)).collect();
        let normalized = |t: &oxrdf::TripleRef<'_>| -> Option<oxrdf::Triple> {
            let oxrdf::TermRef::Literal(l) = t.object else {
                return None;
            };
            let value = l
                .value()
                .replace("\r\n", " ")
                .replace(['\t', '\n', '\r'], " ");
            let o = match l.language() {
                Some(lang) => Literal::new_language_tagged_literal_unchecked(value, lang),
                None => Literal::new_typed_literal(value, l.datatype()),
            };
            Some(oxrdf::Triple::new(
                t.subject.into_owned(),
                t.predicate.into_owned(),
                o,
            ))
        };
        let explained = only_ours.len() == only_theirs.len()
            && only_theirs.iter().all(|t| {
                normalized(t).is_some_and(|n| only_ours.iter().any(|o| o.into_owned() == n))
            });
        if explained {
            None
        } else {
            difference(ours, theirs)
        }
    }

    /// Our parse of a text `oxrdfxml` reads without error: no error, and
    /// the same graph.
    #[track_caller]
    fn agrees(text: &str, base: Option<&str>) -> Parsed {
        let p = parse(text, base);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let want = theirs(text, base).expect("oxrdfxml reads it");
        if let Some(d) = difference(&graph(&p), &want) {
            panic!("{d}");
        }
        p
    }

    fn at(text: &str, s: Span) -> &str {
        &text[s.start as usize..s.end as usize]
    }

    /// Every span is inside the text and on character boundaries; refs and
    /// statements are sorted and disjoint; `statement` never decreases.
    #[track_caller]
    fn spans_hold(text: &str, p: &Parsed) {
        let inside = |s: Span| {
            s.start <= s.end
                && (s.end as usize) <= text.len()
                && text.is_char_boundary(s.start as usize)
                && text.is_char_boundary(s.end as usize)
        };
        for t in &p.triples {
            assert!(inside(t.p_span) && inside(t.o_span), "{t:?}");
            assert!((t.statement as usize) < p.statements.len(), "{t:?}");
        }
        assert!(
            p.triples
                .windows(2)
                .all(|w| w[0].statement <= w[1].statement)
        );
        for e in &p.errors {
            assert!(inside(e.span), "{e:?}");
        }
        for r in &p.refs {
            assert!(inside(r.span), "{r:?}");
        }
        assert!(p.refs.windows(2).all(|w| w[0].span.end <= w[1].span.start));
        for s in &p.statements {
            assert!(inside(s.span), "{s:?}");
        }
        assert!(
            p.statements
                .windows(2)
                .all(|w| w[0].span.end <= w[1].span.start)
        );
    }

    #[test]
    fn a_document_gives_its_triples_refs_and_statements() {
        let text = r##"<?xml version="1.0"?>
<!DOCTYPE rdf:RDF [
  <!ENTITY ex "http://example.org/">
]>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:ex="http://example.org/"
         xmlns="http://example.org/d#"
         xml:base="http://example.org/base">
  <!-- a class -->
  <rdfs:Class rdf:about="http://example.org/Dog" rdfs:label="Dog">
    <rdfs:subClassOf rdf:resource="&ex;Animal"/>
    <rdfs:comment xml:lang="en">A dog</rdfs:comment>
    <ex:size rdf:datatype="http://www.w3.org/2001/XMLSchema#integer">3</ex:size>
    <rdfs:seeAlso>
      <ex:Page rdf:about="#page"><ex:title>t</ex:title></ex:Page>
    </rdfs:seeAlso>
    <ex:list rdf:parseType="Collection"><rdf:Description rdf:about="a"/><ex:B/></ex:list>
    <ex:knows rdf:parseType="Resource"><ex:name>n</ex:name></ex:knows>
    <ex:ref rdf:ID="r1" rdf:nodeID="b1"/>
    <local>x</local>
  </rdfs:Class>
  <rdf:Seq rdf:ID="s"><rdf:li>a</rdf:li><rdf:li rdf:resource="#x"/></rdf:Seq>
</rdf:RDF>
"##;
        let p = agrees(text, None);
        spans_hold(text, &p);
        assert_eq!(p.syntax, Syntax::RdfXml);
        assert_eq!(p.document_base, None);
        assert_eq!(parse(text, Some(BASE)).document_base.as_deref(), Some(BASE));
        assert_eq!(p.statements.len(), 2);
        assert_eq!(
            p.statements[0]
                .subject
                .map(|(i, s)| (i.as_str(), at(text, s))),
            Some(("http://example.org/Dog", "http://example.org/Dog"))
        );
        assert!(at(text, p.statements[0].span).starts_with("<rdfs:Class"));
        assert!(at(text, p.statements[0].span).ends_with("</rdfs:Class>"));
        assert!(at(text, p.statements[1].span).starts_with("<rdf:Seq"));
        // Each triple's predicate and object, as written.
        let written: Vec<(&str, &str)> = p
            .triples
            .iter()
            .map(|t| (at(text, t.p_span), at(text, t.o_span)))
            .collect();
        for pair in [
            ("rdfs:label", "Dog"),
            ("rdfs:Class", "rdfs:Class"),
            ("rdfs:subClassOf", "&ex;Animal"),
            ("rdfs:comment", "A dog"),
            ("ex:size", "3"),
            ("ex:title", "t"),
            ("ex:name", "n"),
            ("local", "x"),
            ("rdf:li", "a"),
            ("rdf:li", "#x"),
        ] {
            assert!(written.contains(&pair), "{pair:?} not in {written:?}");
        }
        assert!(written.iter().any(|(p, o)| *p == "rdfs:seeAlso"
            && o.starts_with("<ex:Page")
            && o.ends_with("</ex:Page>")));
        assert!(
            written
                .iter()
                .any(|(p, o)| *p == "ex:knows" && *o == "<ex:name>n</ex:name>")
        );
        // Every reference slices its IRI or name; an anchor is near it.
        let base = oxiri::Iri::parse(BASE.to_string()).unwrap();
        for r in &p.refs {
            let s = at(text, r.span);
            match r.form {
                Form::Iri => assert_eq!(resolve(&base, s).unwrap().as_str(), r.iri.as_str()),
                Form::PName { prefix_len } => {
                    assert_eq!(s.as_bytes()[prefix_len as usize], b':', "{s}");
                    let use_ = p.prefix_uses.iter().find(|u| u.span == r.span).unwrap();
                    assert_eq!(use_.name.as_str(), &s[..prefix_len as usize]);
                    assert!(
                        r.iri.as_str().ends_with(&s[prefix_len as usize + 1..]),
                        "{s}"
                    );
                }
                Form::Anchor => assert!(
                    s.contains('&') || matches!(s, "local" | "rdf:li" | "r1" | "s"),
                    "{s}"
                ),
                Form::A => panic!("no `a` in RDF/XML"),
            }
        }
        let roles = |iri: &str| -> Vec<Role> {
            p.refs
                .iter()
                .filter(|r| r.iri.as_str() == iri)
                .map(|r| r.role)
                .collect()
        };
        assert_eq!(roles("http://example.org/Dog"), [Role::Subject]);
        assert_eq!(roles("http://example.org/Animal"), [Role::Object]);
        assert_eq!(
            roles("http://www.w3.org/2001/XMLSchema#integer"),
            [Role::Datatype]
        );
        assert_eq!(
            roles("http://www.w3.org/2000/01/rdf-schema#subClassOf"),
            [Role::Predicate]
        );
        assert_eq!(roles(&format!("{RDF}_2")), [Role::Predicate]);
        assert!(p.prefixes.is_empty());
        assert_eq!(p.comments.len(), 1);
        // The same text with single-quoted entities reads the same.
        let single = text.replace(r#""http://example.org/">"#, "'http://example.org/'>");
        assert_ne!(single, text);
        let q = parse(&single, None);
        assert!(q.errors.is_empty(), "{:?}", q.errors);
        assert_eq!(graph(&q), graph(&p));
        // Blank nodes are numbered in the order they appear: two parses agree.
        assert_eq!(parse(text, None), p);
    }

    /// A nested node element that only names an IRI (a collection item, or
    /// a property's object) is a use of it, not a description.
    #[test]
    fn a_nested_element_that_only_names_an_iri_is_an_object() {
        let text = concat!(
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" ",
            "xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:ex=\"http://example.org/\">\n",
            "  <owl:Class rdf:about=\"http://example.org/A\">\n",
            "    <owl:unionOf rdf:parseType=\"Collection\">\n",
            "      <rdf:Description rdf:about=\"http://example.org/B\"/>\n",
            "      <rdf:Description rdf:about=\"http://example.org/C\" ex:n=\"1\"/>\n",
            "      <ex:K rdf:about=\"http://example.org/D\"/>\n",
            "    </owl:unionOf>\n",
            "    <ex:p><rdf:Description rdf:about=\"http://example.org/E\"></rdf:Description></ex:p>\n",
            "    <ex:p><rdf:Description rdf:about=\"http://example.org/F\"><ex:q>x</ex:q></rdf:Description></ex:p>\n",
            "  </owl:Class>\n",
            "  <rdf:Description rdf:about=\"http://example.org/G\"/>\n",
            "</rdf:RDF>\n",
        );
        let p = agrees(text, None);
        let role = |iri: &str| {
            let roles: Vec<Role> = p
                .refs
                .iter()
                .filter(|r| r.iri.as_str() == format!("http://example.org/{iri}"))
                .map(|r| r.role)
                .collect();
            roles
        };
        for (iri, want) in [
            ("A", Role::Subject),
            ("B", Role::Object),
            ("C", Role::Subject),
            ("D", Role::Subject),
            ("E", Role::Object),
            ("F", Role::Subject),
            ("G", Role::Subject),
        ] {
            assert_eq!(role(iri), [want], "{iri}");
        }
    }

    #[test]
    fn rdf_12_terms_read_as_oxrdfxml_reads_them() {
        let head = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:its="http://www.w3.org/2005/11/its" xmlns:ex="http://example.org/""#;
        for body in [
            // its:dir: on text, on property attributes, inherited by them.
            r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a" xml:lang="ar" its:dir="rtl" ex:q="x"><ex:p its:dir="ltr">t</ex:p><ex:r>u</ex:r><ex:s ex:t="v"/></rdf:Description></rdf:RDF>"#,
            // Without rdf:version 1.2, its:dir is not read.
            r#"><rdf:Description rdf:about="http://example.org/a" xml:lang="en" its:dir="rtl" ex:q="x"><ex:p its:dir="ltr">t</ex:p></rdf:Description></rdf:RDF>"#,
            r#" rdf:version="1.2-basic"><rdf:Description rdf:about="http://example.org/a" xml:lang="en" its:dir="ltr"><ex:p its:dir="rtl">t</ex:p></rdf:Description></rdf:RDF>"#,
            // Triple terms, annotated and not.
            r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a"><ex:says rdf:parseType="Triple" rdf:annotation="http://example.org/r" rdf:annotationNodeID="n"><rdf:Description rdf:about="http://example.org/b"><ex:p rdf:resource="http://example.org/c"/></rdf:Description></ex:says></rdf:Description></rdf:RDF>"#,
            r#"><rdf:Description rdf:about="http://example.org/a"><ex:says rdf:parseType="Triple"><rdf:Description rdf:about="http://example.org/b"><ex:p>x</ex:p></rdf:Description></ex:says><ex:q>y</ex:q></rdf:Description></rdf:RDF>"#,
            r#" rdf:version="1.2"><rdf:Description rdf:about="http://example.org/a"><ex:p rdf:annotationNodeID="n" rdf:ID="i" xml:lang="en">x</ex:p><ex:q rdf:annotation="http://example.org/r2" rdf:resource="http://example.org/o"/></rdf:Description></rdf:RDF>"#,
        ] {
            let text = format!("{head}{body}");
            let p = agrees(&text, Some(BASE));
            spans_hold(&text, &p);
        }
    }

    #[test]
    fn rdf_11_forms_read_as_oxrdfxml_reads_them() {
        for text in [
            // A typed node element as the root, without rdf:RDF.
            r#"<ex:Thing xmlns:ex="http://example.org/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" rdf:about="a" ex:p="1"><ex:r rdf:type="http://example.org/T" ex:s="2"/></ex:Thing>"#,
            // Nested xml:base and xml:lang, rdf:li, containers, empty and
            // nested collections, rdf:nodeID shared, reification.
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/" xml:base="http://example.org/one/" xml:lang="en">
  <rdf:Bag rdf:about="bag" xml:base="http://example.org/two/"><rdf:li rdf:resource="x"/><rdf:li xml:lang="fr">deux</rdf:li><rdf:_7>seven</rdf:_7><rdf:li>three</rdf:li></rdf:Bag>
  <rdf:Description rdf:nodeID="n1"><ex:p rdf:nodeID="n1"/><ex:e rdf:parseType="Collection"/><ex:l rdf:parseType="Collection"><ex:T/><rdf:Description rdf:nodeID="n1"/></ex:l></rdf:Description>
  <rdf:Description rdf:ID="d"><ex:p rdf:ID="st">v</ex:p><ex:q rdf:parseType="Resource" rdf:ID="st2"><ex:r>w</ex:r></ex:q></rdf:Description>
  <rdf:Description><ex:empty/><ex:space>  </ex:space><ex:entity>a &amp; b &#x263A;</ex:entity><ex:cdata><![CDATA[<x>]]></ex:cdata></rdf:Description>
</rdf:RDF>"#,
        ] {
            let p = agrees(text, Some(BASE));
            spans_hold(text, &p);
        }
    }

    #[test]
    fn errors_are_reported_and_the_rest_is_read() {
        let text = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/">
  <rdf:Description rdf:about="http://example.org/a" rdf:nodeID="x">
    <ex:p>one</ex:p>
    <nope:q>two</nope:q>
    <ex:r>three<ex:s/></ex:r>
    <rdf:Description/>
    <ex:t rdf:resource="http://example.org/b" rdf:nodeID="y"/>
    <ex:u>unclosed
  </rdf:Description>
  <rdf:Description rdf:about="http://example.org/c"><ex:v>ok</ex:v></rdf:Description>
</rdf:RDF>"#;
        let p = parse(text, None);
        spans_hold(text, &p);
        let found: Vec<(&str, &str)> = p
            .errors
            .iter()
            .map(|e| (e.code, at(text, e.span)))
            .collect();
        assert_eq!(
            found,
            [
                ("rdfxml-syntax", "rdf:nodeID"),
                ("rdfxml-syntax", "nope:q"),
                ("rdfxml-syntax", "ex:s"),
                ("rdfxml-syntax", "rdf:Description"),
                ("rdfxml-syntax", "rdf:nodeID"),
                ("xml-syntax", "ex:u"),
            ]
        );
        let objects: Vec<String> = p.triples.iter().map(|t| term(&t.o).to_string()).collect();
        for o in ["\"one\"", "\"three\"", "<http://example.org/b>", "\"ok\""] {
            assert!(objects.iter().any(|x| x == o), "{o} not in {objects:?}");
        }
        assert!(
            !objects
                .iter()
                .any(|x| x.contains("unclosed") || x == "\"two\"")
        );
    }

    #[test]
    fn xml_literals_are_exclusive_canonical_xml() {
        let text = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/" xmlns="http://www.w3.org/1999/xhtml" xmlns:h="http://example.org/h">
<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:parseType="Literal"><h:b z="1" a="&lt;&quot;"><i>x &amp; y</i><!-- c --><br/></h:b></ex:p><ex:e rdf:parseType="Literal"></ex:e></rdf:Description></rdf:RDF>"#;
        let p = parse(text, None);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        let values: Vec<&str> = p
            .triples
            .iter()
            .filter_map(|t| match &t.o {
                Node::Literal { value, .. } => Some(&**value),
                _ => None,
            })
            .collect();
        assert_eq!(
            values,
            [
                r#"<h:b xmlns:h="http://example.org/h" a="&lt;&quot;" z="1"><i xmlns="http://www.w3.org/1999/xhtml">x &amp; y</i><br xmlns="http://www.w3.org/1999/xhtml"></br></h:b>"#,
                ""
            ]
        );
    }

    /// Texts `oxrdfxml` reads into another graph than this parser, on
    /// purpose (see the module doc): (name, text). Each must still be read
    /// by `oxrdfxml` and still differ, so the list cannot go stale.
    #[test]
    fn oxrdfxml_departures() {
        let head = r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:ex="http://example.org/">"#;
        let cases = [
            (
                "XML literal namespaces",
                format!(
                    r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:parseType="Literal"><b/></ex:p></rdf:Description></rdf:RDF>"#
                ),
            ),
            (
                "whitespace, CDATA, text",
                format!(
                    "{head}<rdf:Description rdf:about=\"http://example.org/a\"><ex:p>\n<![CDATA[x]]>\n</ex:p></rdf:Description></rdf:RDF>"
                ),
            ),
            (
                "an entity declared twice",
                format!(
                    r#"<!DOCTYPE r [<!ENTITY e "http://example.org/1"><!ENTITY e "http://example.org/2">]>{head}<rdf:Description rdf:about="&e;"><ex:p>x</ex:p></rdf:Description></rdf:RDF>"#
                ),
            ),
            (
                "a parameter entity",
                format!(
                    r#"<!DOCTYPE r [<!ENTITY % e "http://example.org/1">]>{head}<rdf:Description rdf:about="&e;"><ex:p>x</ex:p></rdf:Description></rdf:RDF>"#
                ),
            ),
            (
                "`<` in an attribute value",
                format!(
                    r#"{head}<rdf:Description rdf:about="http://example.org/a" ex:q="a<b"/></rdf:RDF>"#
                ),
            ),
            (
                "rdf:type alone on an empty property element",
                format!(
                    r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:type="http://example.org/T"/></rdf:Description></rdf:RDF>"#
                ),
            ),
            (
                "a line end in an attribute value",
                format!(
                    "{head}<rdf:Description rdf:about=\"http://example.org/a\" ex:q=\"a\nb\"/></rdf:RDF>"
                ),
            ),
            (
                "a tab in an attribute value",
                format!(
                    "{head}<rdf:Description rdf:about=\"http://example.org/a\" ex:q=\"a\tb\"/></rdf:RDF>"
                ),
            ),
            (
                "a CR LF line end in text",
                format!(
                    "{head}<rdf:Description rdf:about=\"http://example.org/a\"><ex:p>a\r\nb</ex:p></rdf:Description></rdf:RDF>"
                ),
            ),
            (
                "an unknown rdf:parseType",
                format!(
                    r#"{head}<rdf:Description rdf:about="http://example.org/a"><ex:p rdf:parseType="Other"><ex:b>x</ex:b></ex:p></rdf:Description></rdf:RDF>"#
                ),
            ),
        ];
        for (name, text) in cases {
            let want = theirs(&text, None).unwrap_or_else(|e| panic!("{name}: oxrdfxml: {e}"));
            let p = parse(&text, None);
            spans_hold(&text, &p);
            assert!(
                difference(&graph(&p), &want).is_some(),
                "{name}: the graphs agree now"
            );
        }
    }

    /// The W3C RDF/XML suite (RDF 1.1), vendored in tests/w3c.
    const W3C: &str = "https://w3c.github.io/rdf-tests/rdf/";

    /// Tests where this parser departs from the suite: (test IRI, why).
    /// A listed test that passes fails the run.
    const W3C_KNOWN: &[(&str, &str)] = &[];

    /// Evaluation tests `oxrdfxml` reads into another graph than the
    /// suite's (and ours): their XML literals are not in exclusive
    /// canonical form. Any other difference from `oxrdfxml` fails the run.
    const W3C_NOT_OXRDFXML: &[&str] = &["xml-canon-test001", "xml-canon-test002"];

    fn w3c_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/w3c/rdf-tests")
    }

    fn w3c_local(url: &str) -> PathBuf {
        w3c_dir().join(url.strip_prefix(W3C).expect("a suite URL"))
    }

    /// (test IRI, evaluation or not, action URL, result URL).
    fn w3c_tests() -> Vec<(String, bool, String, Option<String>)> {
        use oxrdf::vocab::rdf;
        let url = format!("{W3C}rdf11/rdf-xml/manifest.ttl");
        let src = std::fs::read(w3c_local(&url)).unwrap();
        let mut g = Graph::new();
        for t in oxttl::TurtleParser::new()
            .with_base_iri(&url)
            .unwrap()
            .for_slice(&src)
        {
            g.insert(&t.unwrap());
        }
        let mf = |l: &str| {
            NamedNode::new_unchecked(format!(
                "http://www.w3.org/2001/sw/DataAccess/tests/test-manifest#{l}"
            ))
        };
        let manifest: NamedOrBlankNode = NamedNode::new_unchecked(&url).into();
        let mut list = g
            .object_for_subject_predicate(&manifest, &mf("entries"))
            .unwrap()
            .into_owned();
        let mut tests = Vec::new();
        while list != Term::NamedNode(rdf::NIL.into_owned()) {
            let node: NamedOrBlankNode = match &list {
                Term::BlankNode(b) => b.clone().into(),
                Term::NamedNode(n) => n.clone().into(),
                _ => panic!("bad list"),
            };
            let entry = g
                .object_for_subject_predicate(&node, rdf::FIRST)
                .unwrap()
                .into_owned();
            list = g
                .object_for_subject_predicate(&node, rdf::REST)
                .unwrap()
                .into_owned();
            let Term::NamedNode(entry) = entry else {
                panic!("an entry is an IRI")
            };
            let iri = |p: &str| match g.object_for_subject_predicate(&entry, &mf(p)) {
                Some(oxrdf::TermRef::NamedNode(n)) => Some(n.as_str().to_string()),
                _ => None,
            };
            let ty = match g.object_for_subject_predicate(&entry, rdf::TYPE) {
                Some(oxrdf::TermRef::NamedNode(n)) => n.as_str().to_string(),
                _ => panic!("{entry} has no type"),
            };
            let eval = match ty.strip_prefix("http://www.w3.org/ns/rdftest#") {
                Some("TestXMLEval") => true,
                Some("TestXMLNegativeSyntax") => false,
                _ => panic!("unknown test type {ty}"),
            };
            tests.push((
                entry.as_str().to_string(),
                eval,
                iri("action").unwrap(),
                iri("result"),
            ));
        }
        tests
    }

    /// `Ok` when the test passes, else why not.
    fn w3c_run(eval: bool, action: &str, result: Option<&str>) -> Result<(), String> {
        let text = std::fs::read_to_string(w3c_local(action)).map_err(|e| e.to_string())?;
        let p = parse(&text, Some(action));
        spans_hold(&text, &p);
        if !eval {
            return if p.errors.is_empty() {
                Err("no error reported".into())
            } else {
                Ok(())
            };
        }
        if let Some(e) = p.errors.first() {
            return Err(format!("{} at {}: {}", e.code, e.span.start, e.message()));
        }
        let nt = std::fs::read(w3c_local(result.ok_or("no result")?)).map_err(|e| e.to_string())?;
        let want = canonical(
            oxttl::NTriplesParser::new()
                .for_slice(&nt)
                .map(|t| t.expect("the expected N-Triples parse")),
        );
        let got = graph(&p);
        if let Some(d) = difference(&got, &want) {
            return Err(d);
        }
        let name = action.rsplit_once('/').map_or(action, |x| x.0);
        let id = format!(
            "{}-{}",
            name.rsplit('/').next().unwrap_or(""),
            action
                .rsplit('/')
                .next()
                .unwrap_or("")
                .trim_end_matches(".rdf")
        );
        let listed = W3C_NOT_OXRDFXML.contains(&id.as_str());
        match theirs(&text, Some(action)) {
            Ok(g) if (g != got) != listed => Err(format!(
                "oxrdfxml {} this test (W3C_NOT_OXRDFXML lists {id}: {listed})",
                if listed {
                    "now agrees on"
                } else {
                    "gives another graph for"
                }
            )),
            _ => Ok(()),
        }
    }

    #[test]
    fn rdfxml_w3c_suite() {
        let tests = w3c_tests();
        let (mut passed, mut failures, mut stale) = (0, Vec::new(), Vec::new());
        for (iri, eval, action, result) in &tests {
            let known = W3C_KNOWN.iter().find(|k| k.0 == iri);
            match (w3c_run(*eval, action, result.as_deref()), known) {
                (Ok(()), None) => passed += 1,
                (Ok(()), Some(_)) => stale.push(iri.clone()),
                (Err(_), Some(_)) => {}
                (Err(e), None) => failures.push(format!("{iri}: {e}")),
            }
        }
        eprintln!(
            "W3C rdf11/rdf-xml: {passed}/{} pass, {} known",
            tests.len(),
            W3C_KNOWN.len()
        );
        assert!(tests.len() >= 160, "only {} tests", tests.len());
        assert!(stale.is_empty(), "known failures now pass: {stale:?}");
        assert!(
            failures.is_empty(),
            "{} failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    /// Corpus files where our graph departs from `oxrdfxml`'s on purpose:
    /// (file, why). Only attribute-value normalization may explain it
    /// ([`difference_past_normalization`]).
    const CORPUS_KNOWN: &[(&str, &str)] = &[(
        "foaf.rdf",
        "two rdfs:comment attribute values hold a line end, which XML 1.0 (3.3.3) makes a space; oxrdfxml keeps it",
    )];
    const CORPUS: [&str; 4] = ["foaf.rdf", "obo-bfo.rdf", "obo-ro.rdf", "obo-pato.rdf"];

    fn corpus(name: &str) -> Option<(String, String)> {
        let path = corpus_path(name)?;
        let text = std::fs::read_to_string(path).unwrap();
        Some((format!("http://example.org/corpus/{name}"), text))
    }

    #[test]
    fn corpus_graphs_are_oxrdfxml_graphs() {
        for name in CORPUS {
            let Some((base, text)) = corpus(name) else {
                continue;
            };
            let p = parse(&text, Some(&base));
            assert!(
                p.errors.is_empty(),
                "{name}: {:?}",
                &p.errors[..p.errors.len().min(5)]
            );
            spans_hold(&text, &p);
            let want = theirs(&text, Some(&base)).unwrap_or_else(|e| panic!("{name}: {e}"));
            let ours = graph(&p);
            if let Some(d) = difference_past_normalization(&ours, &want) {
                panic!("{name}: {d}");
            }
            let known = CORPUS_KNOWN.iter().find(|k| k.0 == name);
            match (ours == want, known) {
                (true, None) => eprintln!("{name}: ok, {} triples", p.triples.len()),
                (false, Some((_, why))) => eprintln!("{name}: known difference: {why}"),
                (true, Some(_)) => panic!("{name} now agrees: remove it from CORPUS_KNOWN"),
                (false, None) => panic!(
                    "{name}: attribute values normalized: add it to CORPUS_KNOWN\n{}",
                    difference(&ours, &want).unwrap_or_default()
                ),
            }
        }
    }

    /// xorshift64: the same inputs on every run.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    /// The start and end of every tag (not comments, PIs or declarations).
    fn tags(text: &str) -> Vec<(usize, usize)> {
        let b = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while let Some(o) = b[i..].iter().position(|&c| c == b'<') {
            let s = i + o;
            let Some(e) = b[s..].iter().position(|&c| c == b'>').map(|e| s + e + 1) else {
                break;
            };
            if !matches!(b.get(s + 1), Some(b'!' | b'?')) {
                out.push((s, e));
            }
            i = s + 1;
        }
        out
    }

    /// Parse a mutated text: no panic, spans inside it, in bounded time,
    /// and `oxrdfxml`'s graph whenever it reads the text without error.
    /// `true` when `oxrdfxml` read it.
    #[track_caller]
    fn survives(what: &str, text: &str, budget: Duration) -> bool {
        let started = Instant::now();
        let p = parse(text, Some(BASE));
        let took = started.elapsed();
        assert!(took < budget, "{what}: {took:?}");
        spans_hold(text, &p);
        let Ok(want) = theirs(text, Some(BASE)) else {
            return false;
        };
        if let Some(d) = difference_past_normalization(&graph(&p), &want) {
            panic!("{what}: oxrdfxml reads it, and the graphs differ: {d}");
        }
        true
    }

    fn fuzz(name: &str, text: &str, cuts: usize, edits: usize, seed: u64) {
        let mut rng = Rng(seed);
        let budget = Duration::from_millis(200) + Duration::from_micros(text.len() as u64 / 10);
        let mut accepted = 0;
        for i in 0..cuts {
            let mut at = rng.below(text.len());
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            accepted += usize::from(survives(
                &format!("{name} cut {i} at {at}"),
                &text[..at],
                budget,
            ));
        }
        let tags = tags(text);
        for i in 0..edits {
            let (s, e) = tags[rng.below(tags.len())];
            let mutated = if i % 2 == 0 {
                format!("{}{}", &text[..s], &text[e..])
            } else {
                format!("{}{}", &text[..e], &text[s..])
            };
            let what = format!(
                "{name} {} {:?}",
                if i % 2 == 0 { "deleted" } else { "doubled" },
                &text[s..e]
            );
            accepted += usize::from(survives(&what, &mutated, budget));
        }
        eprintln!(
            "{name}: {} mutations, {accepted} read by oxrdfxml",
            cuts + edits
        );
    }

    /// Cut and tag-edited corpus files: each survived, and read as
    /// `oxrdfxml` reads it when it reads it. `CHAOS_ITERS=n` makes it n
    /// mutations of FOAF and of BFO and n / 8 of RO (1.2 MB), two thirds of
    /// them cuts; `CHAOS_SEED` changes the seed.
    #[test]
    fn mutated_corpus_files_are_survived() {
        let env = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<u64>().ok());
        let seed = env("CHAOS_SEED").unwrap_or(0x9E37_79B9_7F4A_7C15);
        let n = env("CHAOS_ITERS").map_or(300, |n| n as usize);
        for (name, count) in [
            ("foaf.rdf", n),
            ("obo-bfo.rdf", n),
            ("obo-ro.rdf", if n == 300 { n } else { n / 8 }),
            ("obo-pato.rdf", 5),
        ] {
            let Some((_, text)) = corpus(name) else {
                continue;
            };
            fuzz(name, &text, count * 2 / 3, count - count * 2 / 3, seed);
        }
    }

    /// Inputs that would take quadratic time in a careless scanner: a tag
    /// with 100,000 attributes, a long run of `%` in a DOCTYPE, 20,000
    /// namespace declarations used by 100,000 names, and an XML literal
    /// whose element uses 20,000 namespaces. Each parses well under a
    /// second (a few tens of milliseconds measured).
    #[test]
    fn timing_long_inputs_parse_in_linear_time() {
        let rdf = r#"xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#""#;
        let mut attrs = String::new();
        for i in 0..100_000 {
            write!(attrs, " ex:a{i}=\"v\"").unwrap();
        }
        let many_attrs = format!(
            r#"<rdf:RDF {rdf} xmlns:ex="http://example.org/"><rdf:Description rdf:about="http://example.org/s"{attrs}/></rdf:RDF>"#
        );
        let percent = format!("<!DOCTYPE r [{} x;]><r/>", "%".repeat(200_000));
        let mut decls = String::new();
        for i in 0..20_000 {
            write!(decls, " xmlns:p{i}=\"http://example.org/{i}/\"").unwrap();
        }
        let mut names = String::new();
        for i in 0..100_000 {
            write!(names, "<p{}:q>x</p{}:q>", i % 20_000, i % 20_000).unwrap();
        }
        let many_names = format!(
            r#"<rdf:RDF {rdf}{decls}><rdf:Description rdf:about="http://example.org/s">{names}</rdf:Description></rdf:RDF>"#
        );
        let mut used = String::new();
        for i in 0..20_000 {
            write!(used, " p{i}:a=\"v\"").unwrap();
        }
        let literal = format!(
            r#"<rdf:RDF {rdf}{decls}><rdf:Description rdf:about="http://example.org/s"><p0:q rdf:parseType="Literal"><x{used}><y{used}/></x></p0:q></rdf:Description></rdf:RDF>"#
        );
        for (name, text, triples) in [
            ("attributes", &many_attrs, 100_000),
            ("percent", &percent, 0),
            ("names", &many_names, 100_000),
            ("literal", &literal, 1),
        ] {
            let started = Instant::now();
            let p = parse(text, None);
            let took = started.elapsed();
            eprintln!("{name}: {} bytes in {took:?}", text.len());
            assert_eq!(p.triples.len(), triples, "{name}: {:?}", p.errors.first());
            assert!(took < Duration::from_secs(1), "{name}: {took:?}");
        }
    }

    const HEAD: &str =
        r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:e="urn:ex:">"#;

    /// The triples of `p` as N-Triples lines, sorted.
    fn lines_of(p: &Parsed) -> Vec<String> {
        let mut v: Vec<String> = p
            .triples
            .iter()
            .map(|t| {
                oxrdf::Triple::new(
                    subject(&t.s),
                    NamedNode::new_unchecked(t.p.as_str()),
                    term(&t.o),
                )
                .to_string()
            })
            .collect();
        v.sort();
        v
    }

    /// RDF/XML 7.2.21: an empty property element with only property
    /// attributes, `rdf:type` among them, has a blank node object that
    /// takes them.
    #[test]
    fn an_empty_property_element_with_rdf_type_has_a_blank_node_object() {
        let text = format!(
            r#"{HEAD}<rdf:Description rdf:about="urn:s"><e:p rdf:type="urn:T"/><e:q rdf:type="urn:T" e:r="v"/></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        spans_hold(&text, &p);
        assert_eq!(
            lines_of(&p),
            [
                "<urn:s> <urn:ex:p> _:b0",
                "<urn:s> <urn:ex:q> _:b1",
                "_:b0 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T>",
                "_:b1 <http://www.w3.org/1999/02/22-rdf-syntax-ns#type> <urn:T>",
                "_:b1 <urn:ex:r> \"v\"",
            ]
        );
    }

    /// XML 1.0 2.11 and 3.3.3: line ends are `\n`; in an attribute value,
    /// white space is a space, but a character reference stays the
    /// character. Spans stay over the text as written.
    #[test]
    fn line_ends_and_attribute_values_are_normalized() {
        let text = format!(
            "{HEAD}<rdf:Description rdf:about=\"urn:s\" e:a=\"a\nb\" e:b=\"a\r\nb\" e:c=\"a\tb&#10;c&#9;\"><e:t>a\r\nb\rc&#13;</e:t><e:d><![CDATA[x\r\ny]]></e:d></rdf:Description></rdf:RDF>"
        );
        let p = parse(&text, None);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        spans_hold(&text, &p);
        let value = |iri: &str| {
            let t = p.triples.iter().find(|t| t.p.as_str() == iri).unwrap();
            let Node::Literal { value, .. } = &t.o else {
                panic!()
            };
            (value.to_string(), at(&text, t.o_span))
        };
        assert_eq!(value("urn:ex:a"), ("a b".into(), "a\nb"));
        assert_eq!(value("urn:ex:b"), ("a b".into(), "a\r\nb"));
        assert_eq!(value("urn:ex:c"), ("a b\nc\t".into(), "a\tb&#10;c&#9;"));
        assert_eq!(value("urn:ex:t"), ("a\nb\nc\r".into(), "a\r\nb\rc&#13;"));
        assert_eq!(value("urn:ex:d").0, "x\ny");
    }

    /// A triple whose subject, predicate or object does not resolve to an
    /// absolute IRI is dropped, whatever the path; the error stays.
    #[test]
    fn a_triple_with_an_iri_that_does_not_resolve_is_dropped() {
        let text = format!(
            r#"{HEAD}<rdf:Description rdf:about="bad subject"><e:p>text</e:p></rdf:Description><rdf:Description rdf:about="urn:s"><e:p rdf:resource="bad object"/><e:q rdf:datatype="bad type">1</e:q><e:ok>x</e:ok></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, Some("http://example.org/"));
        spans_hold(&text, &p);
        assert_eq!(lines_of(&p), ["<urn:s> <urn:ex:ok> \"x\""]);
        let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
        assert_eq!(errors, ["bad subject", "bad object", "bad type"]);
        // rdf:ID with no base: no IRI, no triple.
        let text = format!(
            r#"{HEAD}<rdf:Description rdf:ID="a"><e:p>x</e:p></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        assert!(p.triples.is_empty(), "{:?}", p.triples);
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    }

    /// RDF/XML 7.2.20: any other `rdf:parseType` reads as "Literal".
    #[test]
    fn an_unknown_parse_type_is_an_xml_literal() {
        let text = format!(
            r#"{HEAD}<rdf:Description rdf:about="urn:s"><e:p rdf:parseType="Other"><e:b>x</e:b></e:p></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(
            lines_of(&p),
            [
                r#"<urn:s> <urn:ex:p> "<e:b xmlns:e=\"urn:ex:\">x</e:b>"^^<http://www.w3.org/1999/02/22-rdf-syntax-ns#XMLLiteral>"#
            ]
        );
    }

    /// XML 1.0 4.4.2 and 4.5: an entity reference in a replacement text
    /// expands where the entity is used, and a replacement text with
    /// markup in content is read as markup. Everything read from it has
    /// the span of the reference.
    #[test]
    fn entities_nest_and_their_markup_is_read() {
        let text = format!(
            r#"<!DOCTYPE r [<!ENTITY a "&b;"><!ENTITY b "value"><!ENTITY l "<e:label>hello</e:label>"><!ENTITY two "&l;<e:n>&a;</e:n>"><!ENTITY ltx "&lt;e:x/>"><!ENTITY sixty "&#60;e:y/>">]>{HEAD}<rdf:Description rdf:about="urn:s" e:p="&a;">&l;</rdf:Description><rdf:Description rdf:about="urn:t">&two;<e:q>&lt;</e:q></rdf:Description><rdf:Description rdf:about="urn:u"><e:text>&ltx;</e:text>&sixty;</rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        spans_hold(&text, &p);
        assert_eq!(
            lines_of(&p),
            [
                "<urn:s> <urn:ex:label> \"hello\"",
                "<urn:s> <urn:ex:p> \"value\"",
                "<urn:t> <urn:ex:label> \"hello\"",
                "<urn:t> <urn:ex:n> \"value\"",
                "<urn:t> <urn:ex:q> \"<\"",
                "<urn:u> <urn:ex:text> \"<e:x/>\"",
                "<urn:u> <urn:ex:y> \"\"",
            ]
        );
        for t in &p.triples {
            let (pred, obj) = (at(&text, t.p_span), at(&text, t.o_span));
            match t.p.as_str() {
                "urn:ex:label" => assert!(pred == "&l;" || pred == "&two;", "{pred}"),
                "urn:ex:n" => assert_eq!((pred, obj), ("&two;", "&two;")),
                "urn:ex:y" => assert_eq!(pred, "&sixty;"),
                _ => {}
            }
        }
        // References inside an entity are anchors at its reference.
        for r in &p.refs {
            let s = at(&text, r.span);
            if s.starts_with('&') {
                assert_eq!(r.form, Form::Anchor, "{s}");
            }
        }
    }

    /// The entity limits hold with references that nest: a recursive
    /// entity is refused, markup that does not close is closed at the end
    /// of its replacement text, and a markup entity in an attribute value
    /// is reported.
    #[test]
    fn nested_entities_keep_their_limits() {
        for (body, error) in [
            ("<!ENTITY a \"&b;\"><!ENTITY b \"&a;\">", "refers to itself"),
            ("<!ENTITY a \"<e:p>x\">", "is not closed in the entity &a;"),
            (
                "<!ENTITY a \"</rdf:Description>\">",
                "closes no open element",
            ),
            (
                "<!ENTITY a \"<e:p>x</e:p>\">",
                "puts `<` in an attribute value",
            ),
        ] {
            let text = format!(
                r#"<!DOCTYPE r [{body}]>{HEAD}<rdf:Description rdf:about="urn:s" e:v="&a;">&a;</rdf:Description></rdf:RDF>"#
            );
            let p = parse(&text, None);
            spans_hold(&text, &p);
            assert!(
                p.errors.iter().any(|e| e.message().contains(error)),
                "{body}: {:?}",
                p.errors
            );
            for e in &p.errors {
                assert_ne!(e.span.start, e.span.end, "{body}: {e:?}");
            }
        }
        // An element the entity leaves open ends at the reference (found by
        // the chaos test: a collection item's span ended inside the
        // replacement text).
        let text = format!(
            r#"<!DOCTYPE r [<!ENTITY a "x<e:N>">]>{HEAD}<rdf:Description rdf:about="urn:s"><e:list rdf:parseType="Collection">&a;</e:list></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        assert!(!p.triples.is_empty());
        // A billion laughs in markup: refused past 64 KiB, and the text read
        // stays within the budget.
        let mut decl = String::from("<!DOCTYPE r [<!ENTITY a0 \"<e:p>x</e:p>\">");
        for i in 1..30 {
            write!(decl, "<!ENTITY a{i} \"&a{0};&a{0};\">", i - 1).unwrap();
        }
        let text = format!(
            r#"{decl}]>{HEAD}<rdf:Description rdf:about="urn:s">{}</rdf:Description></rdf:RDF>"#,
            "&a12;&a29;".repeat(50)
        );
        let started = Instant::now();
        let p = parse(&text, None);
        assert!(started.elapsed() < Duration::from_secs(1));
        spans_hold(&text, &p);
        assert!(
            p.errors.iter().any(|e| e.message().contains("KiB")),
            "{:?}",
            p.errors
        );
        assert!(p.triples.len() <= text.len() * xml::EXPANSION_PER_BYTE / 10 + 10_000);
    }

    /// Every span made inside or after an entity reference is over the
    /// text, start before end: the white space an entity expands to is not
    /// in the text (12 spaces from `&pad;`).
    #[test]
    fn spans_after_an_entity_are_in_the_text() {
        let text = format!(
            r#"<!DOCTYPE r [<!ENTITY pad "            ">]>{HEAD}&pad;x<rdf:Description rdf:about="urn:s"><e:p>&pad;y</e:p>&pad;z</rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
        assert_eq!(errors, ["&pad;x", "&pad;z"], "{:?}", p.errors);
    }

    /// Namespaces in XML 6.3: two attributes with one namespace and local
    /// name are an error; the tag is read on.
    #[test]
    fn two_attributes_with_one_expanded_name_are_an_error() {
        let text = format!(
            r#"{HEAD}<rdf:Description rdf:about="urn:s" xmlns:z="urn:ex:" e:p="1" z:p="2"><e:q>x</e:q></rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        let errors: Vec<(&str, &str)> = p
            .errors
            .iter()
            .map(|e| (e.code, at(&text, e.span)))
            .collect();
        assert_eq!(errors, [("xml-syntax", "z:p")]);
        assert!(lines_of(&p).contains(&"<urn:s> <urn:ex:q> \"x\"".to_string()));
        // Many attributes: the same, through the map.
        let many: String = (0..40).map(|i| format!(" e:a{i}=\"v\"")).collect();
        let text = format!(
            r#"{HEAD}<rdf:Description rdf:about="urn:s" xmlns:z="urn:ex:"{many} z:a3="w"/></rdf:RDF>"#
        );
        let p = parse(&text, None);
        let errors: Vec<&str> = p.errors.iter().map(|e| at(&text, e.span)).collect();
        assert_eq!(errors, ["z:a3"]);
    }

    /// XML 1.0 2.1: a document has a root element.
    #[test]
    fn a_document_without_a_root_element_is_an_error() {
        for text in [
            "<?xml version=\"1.0\"?>",
            "<?xml version=\"1.0\"?>\n<!-- c -->\n",
            "",
            "  \n",
        ] {
            let p = parse(text, None);
            spans_hold(text, &p);
            let errors: Vec<(&str, &str)> =
                p.errors.iter().map(|e| (e.code, e.message())).collect();
            assert_eq!(
                errors,
                [("xml-syntax", "the document has no root element")],
                "{text:?}"
            );
        }
    }

    /// The IRIs one parse makes are bounded by its text: a long namespace
    /// with many short local names does not make the interner keep a copy
    /// of the namespace per name past [`IRI_BYTES_PER_BYTE`].
    #[test]
    fn the_iris_a_parse_makes_are_bounded() {
        let unique = format!(
            "{:x}{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let ns = format!("http://example.org/{unique}/{}/", "n".repeat(8 * 1024));
        let names = 20_000;
        let mut body = String::new();
        for i in 0..names {
            write!(body, "<x:p{i}>v</x:p{i}>").unwrap();
        }
        let text = format!(
            r#"<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" xmlns:x="{ns}"><rdf:Description rdf:about="urn:s">{body}</rdf:Description></rdf:RDF>"#
        );
        let p = parse(&text, None);
        spans_hold(&text, &p);
        let iris: Vec<String> = (0..names).map(|i| format!("{ns}p{i}")).collect();
        let kept: Vec<&String> = iris.iter().filter(|i| Iri::get(i).is_some()).collect();
        let kept_bytes: usize = kept.iter().map(|i| i.len()).sum();
        let unbounded: usize = iris.iter().map(String::len).sum();
        let kept = kept.len();
        let budget = text.len() * IRI_BYTES_PER_BYTE + MIN_IRI_BYTES;
        eprintln!(
            "text {} bytes; {kept} of {names} IRIs kept, {kept_bytes} bytes (budget {budget}, unbounded {unbounded})",
            text.len()
        );
        assert!(kept_bytes <= budget, "{kept_bytes} > {budget}");
        assert!(kept < names);
        assert_eq!(p.triples.len(), kept);
        assert_eq!(
            p.errors.len(),
            1,
            "{:?}",
            &p.errors[..p.errors.len().min(3)]
        );
        assert!(p.errors[0].message().contains("bytes per byte of text"));
        // A second parse makes no more.
        let again = parse(&text, None);
        assert_eq!(again, p);
    }

    /// The release-build parse of PATO (21 MB, 257k triples): 71 ms
    /// measured (2026-11-19, Apple silicon, load average 3.4). The budget
    /// leaves room for CI, which builds without fat LTO on slower runners,
    /// as the JSON-LD frontend's does.
    /// A triple that names an IRI that is not absolute is dropped, as Turtle
    /// and oxrdfxml drop it; the error stays.
    #[test]
    fn a_triple_with_an_invalid_iri_is_dropped_and_reported() {
        let text = concat!(
            "<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:ex=\"http://example.org/\">\n",
            "  <rdf:Description rdf:about=\"http://example.org/a b\">\n",
            "    <ex:p rdf:resource=\"http://example.org/c\"/>\n",
            "  </rdf:Description>\n",
            "  <rdf:Description rdf:about=\"http://example.org/d\">\n",
            "    <ex:p rdf:resource=\"http://example.org/c\"/>\n",
            "  </rdf:Description>\n",
            "</rdf:RDF>\n",
        );
        let p = parse(text, Some("http://example.org/"));
        assert_eq!(p.triples.len(), 1, "{:?}", p.triples);
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
        assert!(p.errors[0].message.contains("not an absolute IRI"));
    }

    #[test]
    fn timing_rdfxml_parse() {
        let Some((base, text)) = corpus("obo-pato.rdf") else {
            return;
        };
        let started = Instant::now();
        let p = parse(&text, Some(&base));
        let took = started.elapsed();
        eprintln!("obo-pato.rdf: {} triples in {took:?}", p.triples.len());
        assert!(p.errors.is_empty());
        assert!(took < Duration::from_millis(500), "{took:?}");
    }
}
