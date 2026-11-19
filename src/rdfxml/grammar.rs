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
//! - text split by a comment or a CDATA section is one literal.
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
//!   value and its tag end there.
//!
//! Recovery: an RDF/XML error (code `rdfxml-syntax`) is reported at the
//! attribute or name that causes it, and the element is read as far as it
//! can be: an element whose name is no IRI gives no triple of its own but
//! its content is read; of two attributes that cannot go together, the
//! first in the order `rdf:about`, `rdf:ID`, `rdf:nodeID` (`rdf:resource`,
//! `rdf:nodeID` on a property element) is kept; an IRI that does not parse
//! is kept as written. An element the XML scanner closes for it (an
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
//!   overlap;
//! - a [`Ref`] per IRI written in the text: an attribute value is
//!   [`Form::Iri`] (its value span, without the quotes), a prefixed element
//!   or attribute name [`Form::PName`] with a [`PrefixUse`]. Where the text
//!   is not the IRI, the span is [`Form::Anchor`]: a value with an entity or
//!   character reference, `rdf:ID` (a fragment), a name in the default
//!   namespace (no colon), and `rdf:li` (`rdf:_n`). `rdf:about` and `rdf:ID`
//!   are [`Role::Subject`], but for the `rdf:about` of a nested node element
//!   that gives no triple of its own (a collection item that only names a
//!   class): [`Role::Object`], as that IRI is in Turtle. Every reference
//!   inside a triple term is [`Role::Quoted`];
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

/// Parse an RDF/XML document. `base` is the document's URL, against which
/// relative IRIs resolve until an `xml:base` replaces it.
pub(crate) fn parse(text: &str, base: Option<&str>) -> Parsed {
    let mut g = Grammar::new(text, base);
    let mut scanner = Scanner::new(text);
    loop {
        match scanner.next() {
            Event::Eof => break,
            Event::Start(tag) => g.start(&tag),
            Event::End(end) => g.end(end),
            Event::Text { span, text } => g.text(span, &text),
            Event::Pi(span) => g.pi(span),
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
        /// `false` for an unknown `parseType`: read, and no triple.
        emit: bool,
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
    langs: FxHashSet<Iri>,
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
            langs: FxHashSet::default(),
            statement: 0,
            in_triple: 0,
            v: Vocab::new(),
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
            Err(e) => self.error(span, format!("not an absolute IRI: {e}")),
        }
    }

    /// An attribute value as an IRI, resolved against `base`, with its
    /// reference recorded.
    fn iri_value(&mut self, base: &Base, v: &Value<'_>, role: Role) -> Iri {
        let raw: &str = &v.text;
        let iri = match base {
            Some(_) if has_scheme(raw) && !raw.contains("/.") && !raw.contains(":.") => {
                let iri = Iri::intern(raw);
                self.check_absolute(iri, v.span);
                iri
            }
            Some(b) => match resolve(b, raw) {
                Ok(r) => Iri::intern(r.as_str()),
                Err(e) => {
                    self.error(v.span, format!("not an IRI: {e}"));
                    Iri::intern(raw)
                }
            },
            None => {
                let iri = Iri::intern(raw);
                self.check_absolute(iri, v.span);
                iri
            }
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
                let iri = Iri::intern(&format!("{ns}{}", name.local()));
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
                self.out.prefix_uses.push(PrefixUse {
                    name: Iri::intern(prefix),
                    span: name.span,
                    resolved: false,
                });
                self.error(name.span, format!("the prefix {prefix}: is not declared"));
                None
            }
        }
    }

    /// Record the reference a name makes to `iri`.
    fn name_ref(&mut self, name: &Name<'_>, iri: Iri, role: Role) {
        match name.prefix_len {
            Some(prefix_len) => {
                self.out.prefix_uses.push(PrefixUse {
                    name: Iri::intern(name.prefix().unwrap_or("")),
                    span: name.span,
                    resolved: true,
                });
                self.push_ref(name.span, iri, role, Form::PName { prefix_len });
            }
            None => self.push_ref(name.span, iri, role, Form::Anchor),
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
            let Some(iri) = self.name_iri(n) else {
                continue;
            };
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
                Ok(r) => Iri::intern(r.as_str()),
                Err(e) => {
                    self.error(v.span, format!("not an IRI: {e}"));
                    Iri::intern(&fragment)
                }
            },
            None => {
                self.error(v.span, "rdf:ID needs a base IRI");
                Iri::intern(&fragment)
            }
        };
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
            let iri = self.iri_value(&env.base, &attr.value, Role::Subject);
            let own = !a.props.is_empty() || a.type_.is_some() || type_.is_some();
            if !top && !own {
                names = Some(self.out.refs.len() - 1);
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
        if top {
            self.out.statements.push(Statement {
                span: tag.span,
                subject: match &subject {
                    Node::Iri(i) => Some((*i, subject_span)),
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
            let iri = Iri::intern(&format!("{RDF}_{li}"));
            self.push_ref(name.span, iri, Role::Predicate, Form::Anchor);
            Some((iri, name.span))
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
                if a.resource.is_some() || a.node_id.is_some() || !a.props.is_empty() {
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
                    // As oxrdfxml reads it, rdf:type alone gives no object:
                    // the object is the empty literal.
                    State::Prop {
                        env,
                        prop,
                        object: Object::None,
                        datatype,
                        dir,
                    }
                }
            }
            ParseType::Literal | ParseType::Other => State::Literal {
                env,
                prop,
                emit: parse_type == ParseType::Literal,
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
            State::Literal {
                prop, emit, out, ..
            } => {
                if end.implied || !emit {
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
                    let lead = text.len() - text.trim_start().len();
                    let at = Span::new(span.start + lead as u32, span.end);
                    self.error(at, "text where RDF/XML has only elements");
                }
            }
        }
    }

    fn pi(&mut self, span: Span) {
        if let Some(State::Literal { out, .. }) = self.stack.last_mut() {
            out.push_str(&self.text[span.start as usize..span.end as usize]);
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
            r#"<ex:Thing xmlns:ex="http://example.org/" xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#" rdf:about="a" ex:p="1"><ex:q rdf:type="http://example.org/T"/><ex:r rdf:type="http://example.org/T" ex:s="2"/></ex:Thing>"#,
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
    /// (file, why). None so far.
    const CORPUS_KNOWN: &[(&str, &str)] = &[];
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
            let diff = difference(&graph(&p), &want);
            let known = CORPUS_KNOWN.iter().any(|k| k.0 == name);
            match (diff, known) {
                (None, false) => eprintln!("{name}: ok, {} triples", p.triples.len()),
                (Some(_), true) => eprintln!("{name}: known difference"),
                (None, true) => panic!("{name} now agrees: remove it from CORPUS_KNOWN"),
                (Some(d), false) => panic!("{name}: {d}"),
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
        if let Some(d) = difference(&graph(&p), &want) {
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

    /// The release-build parse of PATO (21 MB, 257k triples): 71 ms
    /// measured (2026-11-19, Apple silicon, load average 3.4). The budget
    /// leaves room for CI, which builds without fat LTO on slower runners,
    /// as the JSON-LD frontend's does.
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
