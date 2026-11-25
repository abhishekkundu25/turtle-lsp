//! The reader: an OWL/XML text, scanned by [`crate::rdfxml::xml`], checked
//! against the Recommendation's schema and prose, into the tree the
//! Functional grammar builds ([`Document`]) and the [`Terms`] the mapping
//! asks the source for.
//!
//! One walk reads every construct: the slots of its signature
//! ([`signature`]) are filled in order from its child elements, an element
//! named by a keyword of the slot's category as a construct, an entity
//! element of the slot's kind ([`Cat::entity`]) as a bare IRI. Only the
//! entity elements inside `Declaration` are constructs, as in Functional
//! syntax. The document level is read apart: the root `Ontology`, its
//! `ontologyIRI` and `versionIRI`, then `Prefix`, `Import`, ontology
//! annotations and axioms, in the schema's order.
//!
//! An IRI is an attribute value or an element's trimmed content. A value of
//! schema type `xsd:anyURI` resolves against the base in scope (the nearest
//! `xml:base`, resolved against the one outside it, else the document's
//! URL), and must be an absolute IRI: the reader checks the reference, the
//! mapper resolves it ([`crate::ofn::rdf::IriValue::Based`]). An empty
//! value is the empty reference, the base itself. Where OWL API, which
//! joins a relative reference to the nearest `xml:base` as strings, reads
//! another IRI, a warning names both ([`Joined`]). An
//! `abbreviatedIRI` is `prefix:local`, its prefix declared by a `Prefix`
//! element of this document; no name is built in. The reader resolves only
//! the bases and the prefix IRIs, and charges them to a budget of
//! [`IRI_BYTES_PER_BYTE`] per byte of text, as the mapper charges its IRIs.
//!
//! An item (an import, an ontology annotation, an axiom) is broken, and
//! gives nothing, when an error is reported inside it, when the scanner's
//! fault count moves while it is read (from its start tag to its end tag),
//! when its end is implied, or when it uses a base in error. The errors
//! are one list in text order, `xml-syntax` and `owx-syntax`, at most
//! [`MAX_ERRORS`], then one more that says so; the warnings, at most
//! [`MAX_WARNINGS`] apart from them, then one more that says so.
//!
//! What OWL API writes beyond the Recommendation (the departures the
//! module `owx` lists) is read in the same walk: the SWRL elements are
//! keywords like the others; a `Variable` is read as an entity element is,
//! a construct with the IRI of its `IRI` or `abbreviatedIRI`; a
//! `BuiltInAtom`'s IRI comes from the same attributes, as a `cardinality`
//! from its own; a `nodeID` may be `_:` and an `NCName`.

use super::CODE;
use crate::ofn::rdf::{IRI_BYTES_PER_BYTE, MIN_IRI_BYTES};
use crate::ofn::signature::{Cat, Entity, Rep, Slot, is_axiom, keyword_in, signature};
use crate::ofn::tree::{Document, Keyword, Node, NodeKind};
use crate::ofn::{MAX_ERRORS, MAX_WARNINGS, more_warnings, shown};
use crate::rdfxml::xml::{self, Attr, End, Event, Ns, Prolog, Refs, Scanner, Tag};
use crate::syntax::{Form, Iri, PrefixDecl, Severity, Span, SyntaxError, has_scheme};
use rustc_hash::FxHashMap;
use std::borrow::Cow;

/// The namespace of every OWL/XML element.
const OWL: &str = "http://www.w3.org/2002/07/owl#";
const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";

/// XML's white space (XML 1.0, 2.3).
const WHITE: &[char] = &[' ', '\t', '\n', '\r'];

/// The prefix names of Table 2 of the structural specification: an OWL/XML
/// document declares them too, and an error says how.
const STANDARD: [(&str, &str); 4] = [
    ("rdf", RDF),
    ("rdfs", "http://www.w3.org/2000/01/rdf-schema#"),
    ("xsd", "http://www.w3.org/2001/XMLSchema#"),
    ("owl", OWL),
];

/// A string the reader keeps: the span of the text that spells it, or a
/// range of [`Terms::arena`].
#[derive(Clone, Copy, Debug)]
pub(super) enum Str {
    Text(Span),
    Arena(u32, u32),
}

impl Str {
    /// Its length in bytes.
    pub(super) fn len(self) -> usize {
        match self {
            Str::Text(span) => (span.end - span.start) as usize,
            Str::Arena(from, to) => (to - from) as usize,
        }
    }
}

/// What the tree does not hold of a node, which the source answers from.
#[derive(Clone, Copy, Debug)]
pub(super) enum Term {
    /// A construct, and its element's name as written.
    Construct { name: Span },
    /// An IRI, and how the text at its span spells it.
    Iri { value: Value, form: Form },
    /// An anonymous individual's `nodeID`.
    Label(Str),
    /// A literal: its content whole, the `xml:lang` in scope, and the span
    /// of its content (of its element when it is empty).
    Literal {
        lexical: Str,
        lang: Option<Str>,
        string: Span,
    },
    /// A `cardinality`, its white space collapsed.
    Integer(Str),
    /// The object or the data properties of a `HasKey`.
    Group,
}

/// An IRI node's value.
#[derive(Clone, Copy, Debug)]
pub(super) enum Value {
    /// An absolute IRI.
    Full(Str),
    /// A reference to resolve against base `base` of [`Terms::bases`].
    Based { base: u32, reference: Str },
    /// `prefix:local`, its `:` at byte `colon`, its prefix bound to prefix
    /// IRI `prefix` of [`Terms::prefixes`].
    Abbreviated {
        written: Str,
        colon: u32,
        prefix: u32,
    },
}

/// What the source answers from, besides the text: one [`Term`] per node.
#[derive(Debug, Default)]
pub(super) struct Terms {
    pub(super) terms: Vec<Term>,
    /// The strings no span of the text spells: values through a reference,
    /// resolved bases and prefix IRIs.
    pub(super) arena: String,
    /// The bases in force somewhere, each an absolute IRI, read through
    /// [`Terms::base`].
    pub(super) bases: Vec<BaseStr>,
    /// The IRI of each prefix declared, in text order.
    pub(super) prefixes: Vec<Str>,
    /// The root element's name.
    pub(super) ontology_name: Span,
}

impl Terms {
    pub(super) fn str<'a>(&'a self, text: &'a str, s: Str) -> &'a str {
        match s {
            Str::Text(span) => &text[span.start as usize..span.end as usize],
            Str::Arena(from, to) => &self.arena[from as usize..to as usize],
        }
    }

    /// The text of base `b` of [`Terms::bases`].
    pub(super) fn base<'a>(&'a self, text: &'a str, b: u32) -> &'a str {
        self.str(text, base_read(self.bases[b as usize]))
    }
}

/// Read `text`, whose URL is `base`, into a tree and its terms.
pub(super) fn read(text: &str, base: Option<&str>) -> (Document, Terms) {
    let prolog = Prolog::read(text);
    let mut r = Reader::new(text, &prolog, base);
    r.document();
    r.finish()
}

/// The item being read is abandoned; the error that says why is reported
/// (or the item used a base in error).
struct Stop;

type R<T = ()> = Result<T, Stop>;

/// The base in scope.
#[derive(Clone, Copy)]
enum Base {
    None,
    /// An `xml:base` in error: a relative IRI under it is not known.
    Broken,
    At(u32),
}

/// The language in scope (XML 1.0, section 2.12): the nearest `xml:lang`.
#[derive(Clone, Copy)]
enum Lang {
    /// None written, or `xml:lang=""`.
    None,
    /// An `xml:lang` that is no language tag: a literal under it is not
    /// known.
    Broken,
    Tag(Str),
}

/// What an element with an `xml:base` or an `xml:lang` puts in scope, to
/// its end: its depth, and the base and the language in scope inside it,
/// and the nearest `xml:base` as OWL API reads it ([`Joined`]).
#[derive(Clone, Copy)]
struct Scope {
    depth: u32,
    base: Base,
    lang: Lang,
    joined: Option<u32>,
}

/// An `xml:base` as OWL API (4.5.29, ROBOT 1.9.11) reads it: a relative
/// `IRI`, `<IRI>`, `<Import>`, `datatypeIRI` or Prefix `IRI` (and SWRL's
/// `IRI`s) is the nearest `xml:base` as written, the reference joined to
/// it as strings; `ontologyIRI`, `versionIRI` and `facet` it does not
/// resolve, and an `xml:base` on an entity element itself it refuses.
/// Where the joined IRI is not the resolution of XML Base, a warning.
#[derive(Clone, Copy)]
struct Joined {
    /// The `xml:base` as written, read as a base is.
    written: BaseStr,
    /// The base it resolved to, in [`Terms::bases`].
    base: u32,
    /// The two are one string.
    same: bool,
    shape: BaseShape,
}

/// What decides, for a relative reference, whether joining it to a base
/// as strings gives its resolution (RFC 3986, section 5.2): the base's
/// components.
#[derive(Clone, Copy, Debug)]
pub(super) struct BaseShape {
    /// `scheme:` alone.
    scheme_only: bool,
    /// No path, query or fragment.
    bare: bool,
    query: bool,
    fragment: bool,
    /// The path ends in `/`, or is empty with no authority, and has no `.`
    /// or `..` segment: a relative path resolves after it.
    dir: bool,
}

impl BaseShape {
    /// The shape of the absolute IRI `base`.
    pub(super) fn of(base: &str) -> BaseShape {
        let b = oxiri::Iri::parse_unchecked(base);
        let path = b.path();
        let (query, fragment) = (b.query().is_some(), b.fragment().is_some());
        let authority = b.authority().is_some();
        let bare = path.is_empty() && !query && !fragment;
        BaseShape {
            scheme_only: bare && !authority,
            bare,
            query,
            fragment,
            dir: (path.ends_with('/') || (path.is_empty() && !authority)) && !dotted(path),
        }
    }

    /// Whether the relative reference `r` (valid) joined to the base as
    /// strings is its resolution against it.
    pub(super) fn joins_alike(self, r: &str) -> bool {
        let r = oxiri::IriRef::parse_unchecked(r);
        let path = r.path();
        if r.authority().is_some() {
            return self.scheme_only && !dotted(path);
        }
        if path.is_empty() {
            return !self.fragment && (r.query().is_none() || !self.query);
        }
        if path.starts_with('/') {
            return self.bare && !dotted(path);
        }
        !self.query && !self.fragment && self.dir && !dotted(path)
    }
}

/// Whether `path` has a `.` or `..` segment, which resolving removes.
fn dotted(path: &str) -> bool {
    path.split('/').any(|s| s == "." || s == "..")
}

/// `s` as a warning names an IRI: its first 30 and last 40 characters
/// when it is longer, where a base and a reference meet.
fn cut(s: &str) -> Cow<'_, str> {
    const HEAD: usize = 30;
    const TAIL: usize = 40;
    let n = s.chars().count();
    if n <= HEAD + TAIL + 1 {
        return Cow::Borrowed(s);
    }
    let at = |k: usize| s.char_indices().nth(k).map_or(s.len(), |(i, _)| i);
    Cow::Owned(format!("{}…{}", &s[..at(HEAD)], &s[at(n - TAIL)..]))
}

/// The key of [`Reader::prefixes`]: a prefix name. In the lib tests, a
/// `str` that counts its hashes and comparisons, for the tests that read in
/// linear work (`owx/timing.rs`).
#[cfg(not(test))]
type PrefixKey = str;
#[cfg(test)]
use super::timing::work::PrefixKey;

/// The prefix name `s` as a key of [`Reader::prefixes`].
#[cfg(not(test))]
fn prefix_key(s: &str) -> &PrefixKey {
    s
}
#[cfg(test)]
use super::timing::work::prefix_key;

/// A base of [`Terms::bases`], or an `xml:base` as written ([`Joined`]).
/// In the lib tests, a [`Str`] that counts the bytes read through it
/// ([`base_read`]), for the tests that read in linear work
/// (`owx/timing.rs`).
#[cfg(not(test))]
type BaseStr = Str;
#[cfg(test)]
use super::timing::work::BaseStr;

/// `s` as a base of [`Terms::bases`].
#[cfg(not(test))]
fn base_str(s: Str) -> BaseStr {
    s
}
/// Where base `b` is, to read it.
#[cfg(not(test))]
fn base_read(b: BaseStr) -> Str {
    b
}
#[cfg(test)]
use super::timing::work::{base_read, base_str};

/// A declared prefix: its IRI's place in [`Terms::prefixes`], and what its
/// expansions are checked after ([`context`]).
struct Prefix {
    index: u32,
    context: &'static str,
}

/// An element of the Recommendation's schema that is no keyword.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Leaf {
    Iri,
    AbbreviatedIri,
    AnonymousIndividual,
    Literal,
    FacetRestriction,
}

#[derive(Clone, Copy)]
enum Element {
    Keyword(Keyword),
    Leaf(Leaf),
}

/// The element named `local` in the OWL namespace: a keyword, the SWRL
/// ones among them, or a leaf.
fn element(local: &str) -> Option<Element> {
    let leaf = match local {
        "IRI" => Leaf::Iri,
        "AbbreviatedIRI" => Leaf::AbbreviatedIri,
        "AnonymousIndividual" => Leaf::AnonymousIndividual,
        "Literal" => Leaf::Literal,
        "FacetRestriction" => Leaf::FacetRestriction,
        _ => return Keyword::of(local).map(Element::Keyword),
    };
    Some(Element::Leaf(leaf))
}

/// Whether a slot of `cat` takes the element `leaf`.
fn takes(cat: Cat, leaf: Leaf) -> bool {
    match leaf {
        Leaf::Literal => matches!(cat, Cat::Literal | Cat::AnnotationValue | Cat::DArg),
        Leaf::AnonymousIndividual => matches!(
            cat,
            Cat::Individual | Cat::AnnotationSubject | Cat::AnnotationValue
        ),
        Leaf::Iri | Leaf::AbbreviatedIri => matches!(
            cat,
            Cat::Iri(None) | Cat::AnnotationSubject | Cat::AnnotationValue
        ),
        Leaf::FacetRestriction => cat == Cat::Facets,
    }
}

/// The slots of `k` as its child elements fill them, each with the slot of
/// the signature it is part of: the signature's own, but that the data
/// properties and the data range of `DataPropertiesThenRange` are two (at
/// least one `DataProperty`, then a data range), the facets are at least
/// one `FacetRestriction`, and a key list is any number of properties,
/// under a group node. A `cardinality` is no child: the first slot, read
/// from the attribute; nor is the IRI of a `BuiltInAtom`, read from its
/// `IRI` or `abbreviatedIRI`.
fn element_slots(k: Keyword) -> impl Iterator<Item = (Slot, Slot)> {
    signature(k).iter().flat_map(|&slot| {
        let (first, second) = match slot.0 {
            Cat::DataPropertiesThenRange => (
                (Cat::DataProperty, Rep::AtLeast(1)),
                Some((Cat::DataRange, Rep::One)),
            ),
            Cat::Facets => ((Cat::Facets, Rep::AtLeast(1)), None),
            Cat::ObjectKeys => ((Cat::ObjectProperty, Rep::AtLeast(0)), None),
            Cat::DataKeys => ((Cat::DataProperty, Rep::AtLeast(0)), None),
            _ => (slot, None),
        };
        std::iter::once((slot, first)).chain(second.map(|s| (slot, s)))
    })
}

/// Whether a slot that takes `rep` holds enough with `taken`.
fn filled(rep: Rep, taken: usize) -> bool {
    match rep {
        Rep::One => taken == 1,
        Rep::Optional => true,
        Rep::AtLeast(n) => taken >= n as usize,
    }
}

/// Whether `s` is in the lexical space of `xsd:nonNegativeInteger` (XML
/// Schema 1.1 Part 2, 3.4.20): digits after an optional sign, `-` only
/// before a zero.
fn is_non_negative_integer(s: &str) -> bool {
    let (sign, digits) = match s.as_bytes().first() {
        Some(b'+') => (b'+', &s[1..]),
        Some(b'-') => (b'-', &s[1..]),
        _ => (b'+', s),
    };
    !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && (sign == b'+' || digits.bytes().all(|b| b == b'0'))
}

/// What a slot of `cat` is called in an error.
fn expected(cat: Cat) -> &'static str {
    match cat {
        Cat::Iri(Some(e)) => e.name(),
        Cat::Body => "<Body>",
        Cat::Head => "<Head>",
        _ => cat.name(),
    }
}

/// The fault count and the errors reported before an element's start tag
/// was read: what moved from there was inside it.
#[derive(Clone, Copy)]
struct Before {
    faults: usize,
    reported: usize,
}

/// Where the children of `Ontology` are, in the schema's order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Prefixes,
    Imports,
    Annotations,
    Axioms,
}

/// An IRI as written: its span, its value, and whether the text at the
/// span spells the value.
struct Written<'a> {
    span: Span,
    text: Cow<'a, str>,
    spelled: bool,
}

impl<'a> Written<'a> {
    fn of(v: &xml::Value<'a>) -> Written<'a> {
        Written {
            span: v.span,
            text: v.text.clone(),
            spelled: v.refs == Refs::None,
        }
    }
}

struct Reader<'a> {
    text: &'a str,
    scanner: Scanner<'a>,
    doc: Document,
    terms: Terms,
    /// The elements open.
    depth: u32,
    /// The end of the last tag read.
    last: u32,
    /// The last end read.
    end: End,
    /// The scopes of the open elements that have an `xml:base` or an
    /// `xml:lang`, outermost first.
    scopes: Vec<Scope>,
    /// The base outside every `xml:base`: the document's URL.
    outer: Base,
    prefixes: FxHashMap<Box<PrefixKey>, Prefix>,
    errors: Vec<SyntaxError>,
    /// Every error reported, those past [`MAX_ERRORS`] too.
    reported: usize,
    /// Where the first error past [`MAX_ERRORS`] is.
    dropped: Option<Span>,
    /// The warnings, apart from the errors, at most [`MAX_WARNINGS`], and
    /// where the first past it is.
    warnings: Vec<SyntaxError>,
    dropped_warning: Option<Span>,
    /// The item being read used a base or a language in error.
    tainted: bool,
    /// The bytes of the bases and prefix IRIs resolved, and the most
    /// allowed; past it, every item is broken.
    resolved: usize,
    budget: usize,
    over: bool,
    /// The `xml:base`s as OWL API reads them.
    joined: Vec<Joined>,
    /// The bytes of the IRIs the warnings name; past [`Reader::budget`],
    /// a warning names none.
    warned: usize,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str, prolog: &'a Prolog, base: Option<&str>) -> Reader<'a> {
        let mut terms = Terms::default();
        let outer = match base.filter(|b| oxiri::Iri::parse(*b).is_ok()) {
            Some(b) => {
                terms.arena.push_str(b);
                terms.bases.push(base_str(Str::Arena(0, b.len() as u32)));
                Base::At(0)
            }
            None => Base::None,
        };
        Reader {
            text,
            scanner: Scanner::new(text, prolog),
            doc: Document::default(),
            terms,
            depth: 0,
            last: 0,
            end: End {
                span: Span::new(0, 0),
                implied: true,
                name: None,
            },
            scopes: Vec::new(),
            outer,
            prefixes: FxHashMap::default(),
            errors: Vec::new(),
            reported: 0,
            dropped: None,
            warnings: Vec::new(),
            dropped_warning: None,
            tainted: false,
            resolved: 0,
            budget: text.len().saturating_mul(IRI_BYTES_PER_BYTE) + MIN_IRI_BYTES,
            over: false,
            joined: Vec::new(),
            warned: 0,
        }
    }

    fn finish(self) -> (Document, Terms) {
        let Reader {
            scanner,
            mut doc,
            terms,
            errors,
            dropped,
            mut warnings,
            dropped_warning,
            ..
        } = self;
        let (xml_errors, _) = scanner.finish();
        let mut all = xml_errors;
        all.extend(errors);
        all.sort_by_key(|e| e.span.start);
        let cut = all.get(MAX_ERRORS).map(|e| e.span).or(dropped);
        all.truncate(MAX_ERRORS);
        if let Some(span) = cut {
            all.push(SyntaxError::new(
                span,
                format!("More than {MAX_ERRORS} errors; the rest are not reported"),
                Severity::Error,
                CODE,
            ));
        }
        if let Some(span) = dropped_warning {
            warnings.push(more_warnings(span, CODE));
        }
        // In text order; at one start, the error first.
        all.extend(warnings);
        all.sort_by_key(|e| e.span.start);
        doc.errors = all;
        (doc, terms)
    }

    fn error(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        self.reported += 1;
        if self.errors.len() < MAX_ERRORS {
            self.errors
                .push(SyntaxError::new(span, message, Severity::Error, CODE));
        } else if self.dropped.is_none() {
            self.dropped = Some(span);
        }
    }

    /// A warning: it breaks no item, and has a cap of its own.
    fn warning(&mut self, span: Span, message: impl Into<Cow<'static, str>>) {
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings
                .push(SyntaxError::new(span, message, Severity::Warning, CODE));
        } else if self.dropped_warning.is_none() {
            self.dropped_warning = Some(span);
        }
    }

    /// Report `message` at `span`, and stop.
    fn stop<T>(&mut self, span: Span, message: impl Into<Cow<'static, str>>) -> R<T> {
        self.error(span, message);
        Err(Stop)
    }

    // ---- events ----

    /// The next event: an element's `xml:base` and `xml:lang` are read at
    /// its start, and out of scope at its end.
    fn next(&mut self) -> Event<'a> {
        let e = self.scanner.next();
        match &e {
            Event::Start(tag) => {
                self.depth += 1;
                self.last = tag.span.end;
                self.scope_of(tag);
            }
            Event::End(end) => {
                self.depth = self.depth.saturating_sub(1);
                if !end.implied {
                    self.last = end.span.end;
                }
                self.end = *end;
                while self.scopes.last().is_some_and(|s| s.depth > self.depth) {
                    self.scopes.pop();
                }
            }
            Event::Eof => {
                self.end = End {
                    span: Span::new(self.last, self.last),
                    implied: true,
                    name: None,
                };
            }
            Event::Text { .. } | Event::Pi(..) => {}
        }
        e
    }

    /// Read on to the end of the element open at `depth + 1`, unless it has
    /// ended: that end.
    fn skip_to(&mut self, depth: u32) -> End {
        while self.depth > depth {
            if self.next() == Event::Eof {
                break;
            }
        }
        self.end
    }

    /// The next child element of the element `parent`, or `None` at its
    /// end. White space, comments and processing instructions between
    /// elements are passed over; other text is an error.
    fn child(&mut self, parent: &Tag<'a>) -> R<Option<Tag<'a>>> {
        loop {
            match self.next() {
                Event::Start(tag) => return Ok(Some(tag)),
                Event::End(_) | Event::Eof => return Ok(None),
                Event::Text { span, text } if !xml::is_whitespace(&text) => {
                    let message = format!(
                        "Text in <{}>, which holds elements only",
                        shown(parent.name.raw)
                    );
                    return self.stop(self.trimmed(span), message);
                }
                Event::Text { .. } | Event::Pi(..) => {}
            }
        }
    }

    /// The end of `tag`, which holds nothing: white space, comments and
    /// processing instructions at most.
    fn empty(&mut self, tag: &Tag<'a>) -> R {
        if let Some(child) = self.child(tag)? {
            let message = format!(
                "<{}> holds no element, found <{}>",
                shown(tag.name.raw),
                shown(child.name.raw)
            );
            return self.stop(child.name.span, message);
        }
        Ok(())
    }

    /// The content of `tag`, which holds text and no element: its span and
    /// text, trimmed, and whether the text at the span spells it. `None`
    /// when it is empty or white space.
    fn content(&mut self, tag: &Tag<'a>) -> R<Option<Written<'a>>> {
        let mut parts: Vec<(Span, Cow<'a, str>)> = Vec::new();
        loop {
            match self.next() {
                Event::Start(child) => {
                    let message = format!(
                        "<{}> holds text and no element, found <{}>",
                        shown(tag.name.raw),
                        shown(child.name.raw)
                    );
                    return self.stop(child.name.span, message);
                }
                Event::End(_) | Event::Eof => break,
                Event::Text { span, text } => parts.push((span, text)),
                Event::Pi(..) => {}
            }
        }
        let (Some(first), Some(last)) = (parts.first(), parts.last()) else {
            return Ok(None);
        };
        let span = self.trimmed(Span::new(first.0.start, last.0.end));
        let text = match &parts[..] {
            [(_, Cow::Borrowed(t))] => Cow::Borrowed(t.trim_matches(WHITE)),
            [(_, t)] => Cow::Owned(t.trim_matches(WHITE).to_string()),
            _ => {
                let all: String = parts.iter().map(|(_, t)| t.as_ref()).collect();
                Cow::Owned(all.trim_matches(WHITE).to_string())
            }
        };
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(Written {
            span,
            spelled: self.at(span) == text,
            text,
        }))
    }

    /// The content of `tag`, an `xsd:anyURI`: empty or white space, the
    /// empty relative reference (the base in scope), written as the whole
    /// element, which does not spell it.
    fn uri_content(&mut self, tag: &Tag<'a>) -> R<Written<'a>> {
        if let Some(w) = self.content(tag)? {
            return Ok(w);
        }
        if self.end.implied {
            // The scanner reported why.
            return Err(Stop);
        }
        Ok(Written {
            span: Span::new(tag.span.start, self.end.span.end),
            text: Cow::Borrowed(""),
            spelled: false,
        })
    }

    fn at(&self, span: Span) -> &'a str {
        &self.text[span.start as usize..span.end as usize]
    }

    /// The value `v` of an attribute whose schema type collapses white
    /// space (`xsd:anyURI`, `xsd:NCName`, `xsd:nonNegativeInteger`): its
    /// text and its span without the XML white space at their ends.
    fn collapsed(&self, v: &xml::Value<'a>) -> Written<'a> {
        let text = match &v.text {
            Cow::Borrowed(t) => Cow::Borrowed(t.trim_matches(WHITE)),
            Cow::Owned(t) if t.trim_matches(WHITE).len() == t.len() => v.text.clone(),
            Cow::Owned(t) => Cow::Owned(t.trim_matches(WHITE).to_string()),
        };
        let span = self.trimmed(v.span);
        Written {
            spelled: self.at(span) == text,
            span,
            text,
        }
    }

    /// The value `v` of an attribute whose schema type keeps white space
    /// (`abbreviatedIRI`, an `xsd:string` restricted to a pattern), as
    /// written.
    fn as_written(&self, v: &xml::Value<'a>) -> Written<'a> {
        Written {
            spelled: self.at(v.span) == v.text,
            span: v.span,
            text: v.text.clone(),
        }
    }

    /// `span` without the XML white space at its ends.
    fn trimmed(&self, span: Span) -> Span {
        let raw = self.at(span);
        let start = span.start + (raw.len() - raw.trim_start_matches(WHITE).len()) as u32;
        let end = span.end - (raw.len() - raw.trim_end_matches(WHITE).len()) as u32;
        Span::new(start, end.max(start))
    }

    /// The attributes of `tag` named `names` in no namespace, each the
    /// first so named. Any other in no namespace is an error, and so is a
    /// prefix not declared; `xml:base`, `xml:lang`, `xml:space`, namespace
    /// declarations and attributes in another namespace are passed over.
    fn attrs<'t, const N: usize>(
        &mut self,
        tag: &'t Tag<'a>,
        names: [&str; N],
    ) -> R<[Option<&'t Attr<'a>>; N]> {
        let mut out = [None; N];
        let mut bad = false;
        for a in &tag.attrs {
            let n = &a.name;
            if n.raw == "xmlns" || n.prefix() == Some("xmlns") {
                continue;
            }
            match &n.ns {
                Ns::Bound(_) => {}
                Ns::Unknown => {
                    let prefix = n.prefix().unwrap_or("");
                    self.error(
                        n.span,
                        format!(
                            "The prefix '{}:' of the attribute {} is not declared",
                            shown(prefix),
                            shown(n.raw)
                        ),
                    );
                    bad = true;
                }
                Ns::Unbound => match names.iter().position(|&x| x == n.raw) {
                    Some(k) => {
                        out[k] = out[k].or(Some(a));
                    }
                    None => {
                        let message = format!(
                            "<{}> has no attribute {}",
                            shown(tag.name.raw),
                            shown(n.raw)
                        );
                        self.error(n.span, message);
                        bad = true;
                    }
                },
            }
        }
        if bad { Err(Stop) } else { Ok(out) }
    }

    /// `tag`'s element in the OWL namespace; else an error at its name.
    fn element_of(&mut self, tag: &Tag<'a>) -> R<Element> {
        let n = &tag.name;
        let message = match &n.ns {
            Ns::Bound(ns) if &**ns == OWL => match element(n.local()) {
                Some(e) => return Ok(e),
                None => format!("<{}> is no element of OWL/XML", shown(n.raw)),
            },
            Ns::Bound(ns) => format!(
                "<{}> is in the namespace {}, not in OWL's ({OWL})",
                shown(n.raw),
                shown(ns)
            ),
            Ns::Unbound => format!(
                "<{}> is in no namespace: OWL/XML's are in {OWL}",
                shown(n.raw)
            ),
            Ns::Unknown => format!(
                "The prefix '{}:' of <{}> is not declared",
                n.prefix().unwrap_or(""),
                shown(n.raw)
            ),
        };
        self.stop(n.span, message)
    }

    // ---- bases ----

    fn base(&self) -> Base {
        self.scopes.last().map_or(self.outer, |s| s.base)
    }

    fn lang(&self) -> Lang {
        self.scopes.last().map_or(Lang::None, |s| s.lang)
    }

    /// The scope of `tag`, if it has an `xml:base` or an `xml:lang`, from
    /// here to its end.
    fn scope_of(&mut self, tag: &Tag<'a>) {
        let xml = |local: &str| {
            tag.attrs
                .iter()
                .find(|a| a.name.ns() == Some(xml::XML_NS) && a.name.local() == local)
        };
        let (base, lang) = (xml("base"), xml("lang"));
        if base.is_none() && lang.is_none() {
            return;
        }
        let (base, joined) = match base {
            Some(a) => {
                let base = self.base_of(a);
                (base, self.joined_of(a, base))
            }
            None => (self.base(), self.joined()),
        };
        let lang = match lang {
            Some(a) => self.lang_of(a),
            None => self.lang(),
        };
        let depth = self.depth;
        self.scopes.push(Scope {
            depth,
            base,
            lang,
            joined,
        });
    }

    /// The nearest `xml:base` as OWL API reads it, if one is in scope.
    fn joined(&self) -> Option<u32> {
        self.scopes.last().and_then(|s| s.joined)
    }

    /// The `xml:base` attribute `a`, which put `base` in scope, as OWL API
    /// reads it.
    fn joined_of(&mut self, a: &Attr<'a>, base: Base) -> Option<u32> {
        let Base::At(b) = base else {
            return None;
        };
        let w = Written::of(&a.value);
        let written = base_str(self.keep(&w).ok()?);
        let resolved = self.terms.base(self.text, b);
        self.joined.push(Joined {
            written,
            base: b,
            same: resolved == w.text,
            shape: BaseShape::of(resolved),
        });
        Some(self.joined.len() as u32 - 1)
    }

    /// A warning at `w`, whose value is `value`, where OWL API reads it
    /// otherwise, joined to the `xml:base` `joined` as strings.
    fn joins(&mut self, w: &Written<'a>, value: Value, joined: Option<u32>) {
        let (Some(j), Value::Based { .. }) = (joined, value) else {
            return;
        };
        let reference: &str = &w.text;
        let j = self.joined[j as usize];
        if has_scheme(reference) || (j.same && j.shape.joins_alike(reference)) {
            return;
        }
        let cost = self.terms.bases[j.base as usize].len() + j.written.len() + 2 * reference.len();
        if self.warned + cost > self.budget {
            self.warned = self.budget;
            let message = "Read as XML Base resolves it; OWL API joins it to the xml:base as strings, to another IRI";
            return self.warning(w.span, message);
        }
        self.warned += cost;
        let base = self.terms.base(self.text, j.base);
        let written = self.terms.str(self.text, base_read(j.written));
        let mut ours = String::new();
        oxiri::Iri::parse_unchecked(base)
            .resolve_into_unchecked(&oxiri::IriRef::parse_unchecked(reference), &mut ours);
        let theirs = format!("{written}{reference}");
        let message = format!(
            "Read as <{}> (XML Base); OWL API reads <{}>",
            cut(&ours),
            cut(&theirs)
        );
        self.warning(w.span, message);
    }

    /// The language `xml:lang` attribute `a` puts in scope: a language tag
    /// (BCP 47) or empty.
    fn lang_of(&mut self, a: &Attr<'a>) -> Lang {
        let v = Written::of(&a.value);
        if v.text.is_empty() {
            return Lang::None;
        }
        match oxilangtag::LanguageTag::parse(&*v.text) {
            Ok(_) => match self.keep(&v) {
                Ok(s) => Lang::Tag(s),
                Err(Stop) => Lang::Broken,
            },
            Err(e) => {
                let message = format!("Invalid language tag '{}': {e}", shown(&v.text));
                self.error(v.span, message);
                Lang::Broken
            }
        }
    }

    /// The base the `xml:base` attribute `a` puts in scope: resolved
    /// against the base outside it.
    fn base_of(&mut self, a: &Attr<'a>) -> Base {
        let v = Written::of(&a.value);
        match self.base() {
            Base::At(b) => match oxiri::IriRef::parse(&*v.text) {
                Ok(r) => match self.resolve(b, &r, v.span) {
                    Some(at) => Base::At(at),
                    None => Base::Broken,
                },
                Err(e) => {
                    self.error(v.span, format!("xml:base is not an IRI: {e}"));
                    Base::Broken
                }
            },
            outer => match oxiri::Iri::parse(&*v.text) {
                Ok(_) => match self.keep(&v) {
                    Ok(s) => {
                        self.terms.bases.push(base_str(s));
                        Base::At(self.terms.bases.len() as u32 - 1)
                    }
                    Err(Stop) => Base::Broken,
                },
                // Under a base in error, the error is that base's.
                Err(_) if matches!(outer, Base::Broken) && !has_scheme(&v.text) => Base::Broken,
                Err(e) => {
                    self.error(v.span, format!("xml:base is not an absolute IRI: {e}"));
                    Base::Broken
                }
            },
        }
    }

    /// `reference` resolved against base `b`, kept as a new base; `None`
    /// past the budget.
    fn resolve(&mut self, b: u32, reference: &oxiri::IriRef<&str>, at: Span) -> Option<u32> {
        let resolved = self.resolved_str(b, reference, at)?;
        self.terms.bases.push(base_str(resolved));
        Some(self.terms.bases.len() as u32 - 1)
    }

    /// `reference` resolved against base `b`, in the arena; `None` past the
    /// budget.
    fn resolved_str(&mut self, b: u32, reference: &oxiri::IriRef<&str>, at: Span) -> Option<Str> {
        let cost = self.terms.bases[b as usize].len() + reference.as_str().len();
        if self.over || self.resolved + cost > self.budget {
            self.over_limit(at);
            return None;
        }
        self.resolved += cost;
        let base = self.terms.base(self.text, b);
        let mut out = String::new();
        // Both are valid and the base is absolute: the result is an IRI.
        oxiri::Iri::parse_unchecked(base).resolve_into_unchecked(reference, &mut out);
        self.stash(&out, at).ok()
    }

    /// Past a limit, every item from here on is broken: reported once, at
    /// `at`, in the mapper's words.
    fn over_limit(&mut self, at: Span) {
        if !self.over {
            self.over = true;
            self.error(
                at,
                format!(
                    "The IRIs are over {IRI_BYTES_PER_BYTE} bytes per byte of text: the axioms from here on are not read"
                ),
            );
        }
    }

    /// `s` copied to the arena, written at `at`. The arena holds at most
    /// the scanner's expansions and this budget, about 33 bytes per byte of
    /// text; past `u32::MAX` bytes, a limit.
    fn stash(&mut self, s: &str, at: Span) -> R<Str> {
        let from = self.terms.arena.len();
        let Ok(to) = u32::try_from(from + s.len()) else {
            self.over_limit(at);
            return Err(Stop);
        };
        self.terms.arena.push_str(s);
        Ok(Str::Arena(from as u32, to))
    }

    /// `w` as kept: its span when the text spells it, else a copy.
    fn keep(&mut self, w: &Written<'a>) -> R<Str> {
        if w.spelled {
            return Ok(Str::Text(w.span));
        }
        self.stash(&w.text, w.span)
    }

    // ---- IRIs ----

    /// The value of the `xsd:anyURI` `w`, resolved against `base` (the base
    /// of the element it is written on) where it needs to be.
    fn any_uri(&mut self, w: &Written<'a>, base: Base) -> R<Value> {
        let value: &str = &w.text;
        let dots = value.contains("/.") || value.contains(":.");
        match base {
            Base::At(base) if !has_scheme(value) || dots => match oxiri::IriRef::parse(value) {
                Ok(_) => Ok(Value::Based {
                    base,
                    reference: self.keep(w)?,
                }),
                Err(e) => self.stop(w.span, format!("Invalid IRI: {e}")),
            },
            Base::Broken if !has_scheme(value) => {
                self.tainted = true;
                Err(Stop)
            }
            _ => match oxiri::Iri::parse(value) {
                Ok(_) => Ok(Value::Full(self.keep(w)?)),
                Err(_) if !has_scheme(value) => self.stop(
                    w.span,
                    format!(
                        "<{}> is a relative IRI, and no base is in scope to resolve it against",
                        shown(value)
                    ),
                ),
                Err(e) => self.stop(w.span, format!("Invalid IRI: {e}")),
            },
        }
    }

    /// The value of the abbreviated IRI `w`.
    fn abbreviated(&mut self, w: &Written<'a>) -> R<Value> {
        let value: &str = &w.text;
        let Some(colon) = value.find(':') else {
            let message = format!("'{}' is no abbreviated IRI: it has no ':'", shown(value));
            return self.stop(w.span, message);
        };
        let (name, local) = (&value[..colon], &value[colon + 1..]);
        if !(name.is_empty() || is_pn_prefix(name)) || !(local.is_empty() || is_pn_local(local)) {
            let message = format!(
                "'{}' is no abbreviated IRI: a prefix name, ':' and a local name",
                shown(value)
            );
            return self.stop(w.span, message);
        }
        let Some(p) = self.prefixes.get(prefix_key(name)) else {
            let iri = STANDARD.iter().find(|s| s.0 == name).map_or("…", |s| s.1);
            let message = format!(
                "Undeclared prefix '{name}:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"{name}\" IRI=\"{iri}\"/>",
                name = shown(name)
            );
            return self.stop(w.span, message);
        };
        let (index, context) = (p.index, p.context);
        if let Err(e) = oxiri::Iri::parse(format!("{context}{local}")) {
            let prefix = self
                .terms
                .str(self.text, self.terms.prefixes[index as usize]);
            let full: String = prefix.chars().take(61).chain(local.chars()).collect();
            let message = format!("'{}' expands to <{}>: {e}", shown(value), shown(&full));
            return self.stop(w.span, message);
        }
        Ok(Value::Abbreviated {
            written: self.keep(w)?,
            colon: colon as u32,
            prefix: index,
        })
    }

    /// An IRI node of `value`, written as `w`.
    fn iri_node(&mut self, w: &Written<'_>, value: Value) -> usize {
        let form = match value {
            _ if !w.spelled => Form::Anchor,
            Value::Abbreviated { colon, .. } => Form::PName { prefix_len: colon },
            Value::Full(_) | Value::Based { .. } => Form::Iri,
        };
        let prefix_len = match form {
            Form::PName { prefix_len } => Some(prefix_len),
            _ => None,
        };
        self.leaf(
            NodeKind::Iri { prefix_len },
            w.span,
            Term::Iri { value, form },
        )
    }

    /// A node with no child, of `kind` at `span`.
    fn leaf(&mut self, kind: NodeKind, span: Span, term: Term) -> usize {
        let i = self.doc.nodes.len();
        self.doc.nodes.push(Node {
            kind,
            span,
            next: i as u32 + 1,
        });
        self.terms.terms.push(term);
        i
    }

    /// The IRI node of an entity element, from its `IRI` or its
    /// `abbreviatedIRI`, one of them.
    fn entity_iri(&mut self, tag: &Tag<'a>) -> R<usize> {
        let [iri, abbreviated] = self.attrs(tag, ["IRI", "abbreviatedIRI"])?;
        let (w, value) = match (iri, abbreviated) {
            (Some(a), None) => {
                let w = self.collapsed(&a.value);
                let value = self.any_uri(&w, self.base())?;
                // OWL API refuses an `xml:base` on an entity element
                // itself: it reads no IRI there to name.
                let own_base = tag
                    .attrs
                    .iter()
                    .any(|a| a.name.ns() == Some(xml::XML_NS) && a.name.local() == "base");
                if !own_base {
                    self.joins(&w, value, self.joined());
                }
                (w, value)
            }
            (None, Some(a)) => {
                let w = self.as_written(&a.value);
                let value = self.abbreviated(&w)?;
                (w, value)
            }
            (Some(_), Some(a)) => {
                let message = format!(
                    "<{}> has an IRI and an abbreviatedIRI: one of them, not both",
                    shown(tag.name.raw)
                );
                return self.stop(a.name.span, message);
            }
            (None, None) => {
                let message = format!(
                    "<{}> needs the attribute IRI or abbreviatedIRI",
                    shown(tag.name.raw)
                );
                return self.stop(tag.name.span, message);
            }
        };
        Ok(self.iri_node(&w, value))
    }

    // ---- constructs ----

    /// The node of construct `k`, element `tag`, to close.
    fn open(&mut self, k: Keyword, tag: &Tag<'a>) -> usize {
        let name = tag.name.span;
        self.open_node(NodeKind::Construct(k), tag, Term::Construct { name })
    }

    /// The node of `kind` at element `tag`, to close.
    fn open_node(&mut self, kind: NodeKind, tag: &Tag<'a>, term: Term) -> usize {
        self.doc.nodes.push(Node {
            kind,
            span: Span::new(tag.span.start, tag.span.start),
            next: 0,
        });
        self.terms.terms.push(term);
        self.doc.nodes.len() - 1
    }

    /// Close node `i`: at its element's end tag when read whole, else at
    /// the last tag read.
    fn close(&mut self, i: usize, whole: bool) {
        let end = if whole && !self.end.implied {
            self.end.span.end
        } else {
            self.last
        };
        let next = self.doc.nodes.len() as u32;
        let node = &mut self.doc.nodes[i];
        node.span.end = end.max(node.span.start);
        node.next = next;
    }

    /// The construct `k` of element `tag`, to its end.
    fn construct(&mut self, k: Keyword, tag: &Tag<'a>) -> R {
        let i = self.open(k, tag);
        let r = self.construct_body(k, tag);
        self.close(i, r.is_ok());
        r
    }

    fn construct_body(&mut self, k: Keyword, tag: &Tag<'a>) -> R {
        if Entity::of(k).is_some() || k == Keyword::Variable {
            // An entity inside `Declaration`, or a SWRL variable: its IRI.
            self.entity_iri(tag)?;
            return self.empty(tag);
        }
        let faults = self.scanner.faults();
        let mut group = None;
        let r = self.operands(k, tag, faults, &mut group);
        if let Some(g) = group {
            self.leave(g, self.last);
        }
        r
    }

    /// The operands of construct `k`, element `tag`, in the slots of its
    /// signature ([`element_slots`]); `group` the key list open.
    fn operands(
        &mut self,
        k: Keyword,
        tag: &Tag<'a>,
        faults: usize,
        group: &mut Option<usize>,
    ) -> R {
        let mut parts = element_slots(k);
        let mut part = parts.next();
        if let Some(((Cat::Integer, _), _)) = part {
            self.cardinality(tag)?;
            part = parts.next();
        } else if k == Keyword::BuiltInAtom {
            self.entity_iri(tag)?;
            part = parts.next();
        } else {
            self.attrs(tag, [])?;
        }
        *group = self.enter(part);
        // The slot left last, if it would have taken more.
        let mut open: Option<Cat> = None;
        let (mut taken, mut keys) = (0, 0);
        while let Some(child) = self.child(tag)? {
            let el = self.element_of(&child)?;
            loop {
                let Some((slot, (cat, rep))) = part else {
                    let message = match open {
                        Some(cat) => format!(
                            "Expected {} or </{}>, found <{}>",
                            expected(cat),
                            shown(tag.name.raw),
                            shown(child.name.raw)
                        ),
                        None => format!(
                            "Expected the end of <{}>, found <{}>",
                            shown(tag.name.raw),
                            shown(child.name.raw)
                        ),
                    };
                    return self.stop(child.name.span, message);
                };
                let more = matches!(rep, Rep::AtLeast(_)) || taken == 0;
                if more && self.operand(cat, el, &child)? {
                    taken += 1;
                    keys += usize::from(group.is_some());
                    break;
                }
                if !filled(rep, taken) {
                    return self.unfilled(tag, slot, (cat, rep), taken, Some(&child), faults);
                }
                if let Some(g) = group.take() {
                    self.leave(g, child.span.start);
                }
                let more = matches!(rep, Rep::AtLeast(_)) || (rep == Rep::Optional && taken == 0);
                open = more.then_some(cat);
                (part, taken) = (parts.next(), 0);
                *group = self.enter(part);
            }
        }
        if self.end.implied {
            // The scanner reported why.
            return Err(Stop);
        }
        while let Some((slot, (cat, rep))) = part {
            if !filled(rep, taken) {
                return self.unfilled(tag, slot, (cat, rep), taken, None, faults);
            }
            if let Some(g) = group.take() {
                self.leave(g, self.end.span.start);
            }
            (part, taken) = (parts.next(), 0);
            *group = self.enter(part);
        }
        if k == Keyword::HasKey && keys == 0 {
            let message = format!(
                "<{}> needs at least one object or data property expression",
                shown(tag.name.raw)
            );
            return self.stop(tag.name.span, message);
        }
        Ok(())
    }

    /// The group node of `part`, open, if it is a key list.
    fn enter(&mut self, part: Option<(Slot, Slot)>) -> Option<usize> {
        let ((Cat::ObjectKeys | Cat::DataKeys, _), _) = part? else {
            return None;
        };
        let i = self.doc.nodes.len();
        self.doc.nodes.push(Node {
            kind: NodeKind::Group,
            span: Span::new(0, 0),
            next: 0,
        });
        self.terms.terms.push(Term::Group);
        Some(i)
    }

    /// Close group `g`: from its first property to its last, or empty at
    /// `at`, where the element after it starts.
    fn leave(&mut self, g: usize, at: u32) {
        let next = self.doc.nodes.len();
        let span = if next == g + 1 {
            Span::new(at, at)
        } else {
            let mut last = g + 1;
            while (self.doc.nodes[last].next as usize) < next {
                last = self.doc.nodes[last].next as usize;
            }
            Span::new(
                self.doc.nodes[g + 1].span.start,
                self.doc.nodes[last].span.end,
            )
        };
        self.doc.nodes[g].span = span;
        self.doc.nodes[g].next = next as u32;
    }

    /// The `cardinality` of `tag`: an integer node at its value.
    fn cardinality(&mut self, tag: &Tag<'a>) -> R {
        let [cardinality] = self.attrs(tag, ["cardinality"])?;
        let Some(a) = cardinality else {
            let message = format!("<{}> needs the attribute cardinality", shown(tag.name.raw));
            return self.stop(tag.name.span, message);
        };
        let w = self.collapsed(&a.value);
        if !is_non_negative_integer(&w.text) {
            let message = format!(
                "Expected a non-negative integer, found '{}'",
                shown(&w.text)
            );
            return self.stop(w.span, message);
        }
        let value = self.keep(&w)?;
        self.leaf(NodeKind::Integer, w.span, Term::Integer(value));
        Ok(())
    }

    /// The error of a slot left unfilled before `child`, or before the end:
    /// `part` of the signature's `slot`. Not reported when the scanner met
    /// a fault since `faults` (an element it skipped): that is the error.
    fn unfilled(
        &mut self,
        tag: &Tag<'a>,
        slot: Slot,
        part: Slot,
        taken: usize,
        child: Option<&Tag<'a>>,
        faults: usize,
    ) -> R {
        if self.scanner.faults() != faults {
            return Err(Stop);
        }
        let name = &tag.name;
        if let Rep::AtLeast(n) = slot.1 {
            let message = format!(
                "<{}> needs at least {}, found {taken}",
                shown(name.raw),
                slot.0.counted(n)
            );
            return self.stop(name.span, message);
        }
        if slot.0 == Cat::DataPropertiesThenRange && part.0 == Cat::DataProperty {
            let message = format!(
                "<{}> needs a data property and a data range",
                shown(name.raw)
            );
            return self.stop(name.span, message);
        }
        match child {
            Some(c) => {
                let message = format!(
                    "Expected {}, found <{}>",
                    expected(part.0),
                    shown(c.name.raw)
                );
                self.stop(c.name.span, message)
            }
            None => {
                let message = format!(
                    "Expected {}, found </{}>",
                    expected(part.0),
                    shown(name.raw)
                );
                self.stop(self.end.span, message)
            }
        }
    }

    /// Read the element `child`, `el`, as one of `cat`, if it is one:
    /// whether it was.
    fn operand(&mut self, cat: Cat, el: Element, child: &Tag<'a>) -> R<bool> {
        match el {
            Element::Keyword(kw) if keyword_in(kw, cat) => self.construct(kw, child)?,
            Element::Keyword(kw) if Entity::of(kw).is_some() && Entity::of(kw) == cat.entity() => {
                self.entity_iri(child)?;
                self.empty(child)?;
            }
            Element::Leaf(leaf) if takes(cat, leaf) => match leaf {
                Leaf::Literal => self.literal(child)?,
                Leaf::AnonymousIndividual => self.anonymous(child)?,
                Leaf::Iri => self.iri_element(false, child)?,
                Leaf::AbbreviatedIri => self.iri_element(true, child)?,
                Leaf::FacetRestriction => self.facet(child)?,
            },
            Element::Keyword(_) | Element::Leaf(_) => return Ok(false),
        }
        Ok(true)
    }

    // ---- literals, anonymous individuals, IRI elements ----

    /// `Literal`: a literal node over the element, its `datatypeIRI` its
    /// one child, its content whole, the language the one in scope.
    fn literal(&mut self, tag: &Tag<'a>) -> R {
        let empty = Span::new(tag.span.end, tag.span.end);
        let term = Term::Literal {
            lexical: Str::Text(empty),
            lang: None,
            string: empty,
        };
        let kind = NodeKind::Literal {
            string_end: tag.span.end,
        };
        let i = self.open_node(kind, tag, term);
        let r = self.literal_body(i, tag);
        self.close(i, r.is_ok());
        r
    }

    fn literal_body(&mut self, i: usize, tag: &Tag<'a>) -> R {
        // The scope of the element's own start tag, out of scope at its end.
        let lang = match self.lang() {
            Lang::None => None,
            Lang::Tag(s) => Some(s),
            Lang::Broken => {
                self.tainted = true;
                None
            }
        };
        let [datatype] = self.attrs(tag, ["datatypeIRI"])?;
        if let Some(a) = datatype {
            let w = self.collapsed(&a.value);
            let value = self.any_uri(&w, self.base())?;
            self.joins(&w, value, self.joined());
            self.iri_node(&w, value);
        }
        let mut parts: Vec<(Span, Cow<'a, str>)> = Vec::new();
        loop {
            match self.next() {
                Event::Start(child) => {
                    let message = format!(
                        "<{}> holds text and no element, found <{}>",
                        shown(tag.name.raw),
                        shown(child.name.raw)
                    );
                    return self.stop(child.name.span, message);
                }
                Event::End(_) | Event::Eof => break,
                Event::Text { span, text } => parts.push((span, text)),
                Event::Pi(..) => {}
            }
        }
        if self.end.implied {
            // The scanner reported why.
            return Err(Stop);
        }
        let content = Span::new(tag.span.end, self.end.span.start.max(tag.span.end));
        let lexical = match &parts[..] {
            [] => Str::Text(Span::new(content.start, content.start)),
            [(span, t)] if self.at(*span) == t.as_ref() => Str::Text(*span),
            _ => {
                let all: String = parts.iter().map(|(_, t)| t.as_ref()).collect();
                self.stash(&all, content)?
            }
        };
        let string = if content.start < content.end {
            content
        } else {
            Span::new(tag.span.start, self.end.span.end)
        };
        self.doc.nodes[i].kind = NodeKind::Literal {
            string_end: string.end,
        };
        self.terms.terms[i] = Term::Literal {
            lexical,
            lang,
            string,
        };
        Ok(())
    }

    /// `AnonymousIndividual`: a node-id node at its `nodeID` value, an
    /// `NCName`, or `_:` and an `NCName` as OWL API writes it, the whole
    /// value its label (`x` and `_:x` are two nodes).
    fn anonymous(&mut self, tag: &Tag<'a>) -> R {
        let [node_id] = self.attrs(tag, ["nodeID"])?;
        let Some(a) = node_id else {
            let message = format!("<{}> needs the attribute nodeID", shown(tag.name.raw));
            return self.stop(tag.name.span, message);
        };
        let w = self.collapsed(&a.value);
        if !xml::is_nc_name(w.text.strip_prefix("_:").unwrap_or(&w.text)) {
            let message = format!(
                "'{}' is no nodeID: a nodeID is an NCName (a name without ':'), or '_:' and an NCName",
                shown(&w.text)
            );
            return self.stop(w.span, message);
        }
        let label = self.keep(&w)?;
        self.leaf(NodeKind::NodeId, w.span, Term::Label(label));
        self.empty(tag)
    }

    /// `FacetRestriction`: an IRI node at its `facet` value, then its one
    /// `Literal`, the flat pair of Functional syntax.
    fn facet(&mut self, tag: &Tag<'a>) -> R {
        let [facet] = self.attrs(tag, ["facet"])?;
        let Some(a) = facet else {
            let message = format!("<{}> needs the attribute facet", shown(tag.name.raw));
            return self.stop(tag.name.span, message);
        };
        let w = self.collapsed(&a.value);
        let value = self.any_uri(&w, self.base())?;
        self.iri_node(&w, value);
        let Some(child) = self.child(tag)? else {
            if self.end.implied {
                return Err(Stop);
            }
            let message = format!("Expected a literal, found </{}>", shown(tag.name.raw));
            return self.stop(self.end.span, message);
        };
        match self.element_of(&child)? {
            Element::Leaf(Leaf::Literal) => self.literal(&child)?,
            Element::Keyword(_) | Element::Leaf(_) => {
                let message = format!("Expected a literal, found <{}>", shown(child.name.raw));
                return self.stop(child.name.span, message);
            }
        }
        if let Some(extra) = self.child(tag)? {
            let message = format!(
                "Expected the end of <{}>, found <{}>",
                shown(tag.name.raw),
                shown(extra.name.raw)
            );
            return self.stop(extra.name.span, message);
        }
        if self.end.implied {
            return Err(Stop);
        }
        Ok(())
    }

    /// `IRI`, or `AbbreviatedIRI` if `abbreviated`: an IRI node at its
    /// trimmed content. An empty `IRI` is the base in scope; an empty
    /// `AbbreviatedIRI` is an error.
    fn iri_element(&mut self, abbreviated: bool, tag: &Tag<'a>) -> R {
        self.attrs(tag, [])?;
        // The element's own, out of scope once its end is read.
        let (base, joined) = (self.base(), self.joined());
        let (w, value) = if abbreviated {
            let Some(w) = self.content(tag)? else {
                let message = format!(
                    "Expected an abbreviated IRI, found </{}>",
                    shown(tag.name.raw)
                );
                return self.stop(self.end.span, message);
            };
            let value = self.abbreviated(&w)?;
            (w, value)
        } else {
            let w = self.uri_content(tag)?;
            let value = self.any_uri(&w, base)?;
            self.joins(&w, value, joined);
            (w, value)
        };
        self.iri_node(&w, value);
        Ok(())
    }

    // ---- the document ----

    /// What [`Before`] holds now.
    fn before(&self) -> Before {
        Before {
            faults: self.scanner.faults(),
            reported: self.reported,
        }
    }

    /// Whether nothing went wrong since `before`.
    fn whole_since(&self, before: Before) -> bool {
        self.reported == before.reported
            && self.scanner.faults() == before.faults
            && !self.end.implied
            && !self.tainted
            && !self.over
    }

    fn document(&mut self) {
        let before = self.before();
        let root = loop {
            match self.next() {
                Event::Start(tag) => break tag,
                Event::Eof => return,
                Event::End(_) | Event::Text { .. } | Event::Pi(..) => {}
            }
        };
        if !root.name.is(OWL, "Ontology") {
            let message = match root.name.ns() {
                Some(RDF) if root.name.local() == "RDF" => format!(
                    "The root element is <{}>: the text is RDF/XML, not OWL/XML, whose root is <Ontology> in {OWL}",
                    shown(root.name.raw)
                ),
                _ if root.name.local() == "Ontology" => format!(
                    "The root element <{}> is not in OWL's namespace: OWL/XML's root is <Ontology> in {OWL}",
                    shown(root.name.raw)
                ),
                _ => format!(
                    "The root element is <{}>: the text is XML but not OWL/XML, whose root is <Ontology> in {OWL}",
                    shown(root.name.raw)
                ),
            };
            self.error(root.name.span, message);
        } else {
            self.ontology(&root, before);
        }
        // The rest, for the scanner's errors.
        while self.next() != Event::Eof {}
    }

    /// `Ontology`, its header and its children.
    fn ontology(&mut self, root: &Tag<'a>, before: Before) {
        self.terms.ontology_name = root.name.span;
        self.tainted = false;
        self.header(root);
        // The root's end is not read yet.
        let reported = self.reported == before.reported;
        let faults = self.scanner.faults() == before.faults;
        if !(reported && faults && !self.tainted && !self.over) {
            self.doc.header_broken = true;
        }
        let mut phase = Phase::Prefixes;
        loop {
            let before = self.before();
            match self.next() {
                Event::Start(tag) => self.top(&tag, before, &mut phase),
                Event::End(_) | Event::Eof => break,
                Event::Text { span, text } if !xml::is_whitespace(&text) => {
                    let message = format!(
                        "Text in <{}>, which holds elements only",
                        shown(root.name.raw)
                    );
                    self.error(self.trimmed(span), message);
                }
                Event::Text { .. } | Event::Pi(..) => {}
            }
        }
        let end = if self.end.implied {
            self.last
        } else {
            self.end.span.end
        };
        self.doc.ontology = Some(Span::new(root.span.start, end));
    }

    /// The ontology IRI and the version IRI, in text order.
    fn header(&mut self, root: &Tag<'a>) {
        let Ok([ontology, version]) = self.attrs(root, ["ontologyIRI", "versionIRI"]) else {
            return;
        };
        if let (Some(v), None) = (version, ontology) {
            self.error(
                v.name.span,
                "A versionIRI needs an ontologyIRI (structural specification, section 3.1)",
            );
            return;
        }
        let mut written: Vec<&Attr<'a>> = [ontology, version].into_iter().flatten().collect();
        written.sort_by_key(|a| a.span.start);
        for a in written {
            let w = self.collapsed(&a.value);
            let Ok(value) = self.any_uri(&w, self.base()) else {
                continue;
            };
            let i = self.iri_node(&w, value);
            if ontology.is_some_and(|o| std::ptr::eq(o, a)) {
                self.doc.ontology_iri = Some(i);
            } else {
                self.doc.version_iri = Some(i);
            }
        }
    }

    /// A child of `Ontology`, its start tag `tag` read after `before`.
    fn top(&mut self, tag: &Tag<'a>, before: Before, phase: &mut Phase) {
        let depth = self.depth - 1;
        self.tainted = false;
        let el = match self.element_of(tag) {
            Ok(el) => el,
            Err(Stop) => {
                self.skip_to(depth);
                return;
            }
        };
        let (k, here) = match el {
            Element::Keyword(Keyword::Prefix) => (Keyword::Prefix, Phase::Prefixes),
            Element::Keyword(Keyword::Import) => (Keyword::Import, Phase::Imports),
            Element::Keyword(Keyword::Annotation) => (Keyword::Annotation, Phase::Annotations),
            Element::Keyword(k) if is_axiom(k) => (k, Phase::Axioms),
            Element::Keyword(_) | Element::Leaf(_) => {
                let message = format!(
                    "Expected <Prefix>, <Import>, <Annotation> or an axiom, found <{}>",
                    shown(tag.name.raw)
                );
                self.error(tag.name.span, message);
                self.skip_to(depth);
                return;
            }
        };
        let misplaced = *phase > here;
        if misplaced {
            let message = match here {
                Phase::Prefixes => "A <Prefix> comes before the imports, annotations and axioms",
                Phase::Imports => "An <Import> comes before the ontology's annotations and axioms",
                Phase::Annotations | Phase::Axioms => {
                    "An ontology annotation comes before the axioms"
                }
            };
            self.error(tag.name.span, message);
        }
        *phase = (*phase).max(here);
        if k == Keyword::Prefix {
            let declared = self.prefix(tag);
            self.skip_to(depth);
            if let Some(decl) = declared
                && self.whole_since(before)
            {
                self.declare(decl);
            }
            return;
        }
        let i = self.doc.nodes.len();
        let r = if k == Keyword::Import {
            self.import(tag)
        } else {
            self.construct(k, tag)
        };
        self.skip_to(depth);
        let whole = r.is_ok() && self.whole_since(before);
        if !whole && i < self.doc.nodes.len() {
            self.doc.broken.push(i as u32);
        }
    }

    /// `Import`: its IRI, the element's trimmed content (empty, the base in
    /// scope).
    fn import(&mut self, tag: &Tag<'a>) -> R {
        let i = self.open(Keyword::Import, tag);
        let r = (|| {
            self.attrs(tag, [])?;
            // The element's own, out of scope once its end is read.
            let (base, joined) = (self.base(), self.joined());
            let w = self.uri_content(tag)?;
            let value = self.any_uri(&w, base)?;
            self.joins(&w, value, joined);
            self.iri_node(&w, value);
            Ok(())
        })();
        self.close(i, r.is_ok());
        r
    }

    /// `Prefix`: the declaration it makes, unless it has an error.
    fn prefix(&mut self, tag: &Tag<'a>) -> Option<Declared> {
        let [name, iri] = self.attrs(tag, ["name", "IRI"]).ok()?;
        let (Some(name), Some(iri)) = (name, iri) else {
            let missing = if name.is_none() { "name" } else { "IRI" };
            self.error(
                tag.name.span,
                format!("<{}> needs the attribute {missing}", shown(tag.name.raw)),
            );
            return None;
        };
        let written: &str = &name.value.text;
        if !(written.is_empty() || is_pn_prefix(written)) {
            self.error(
                name.value.span,
                format!("'{}' is no prefix name (PN_PREFIX)", shown(written)),
            );
            return None;
        }
        if self.prefixes.contains_key(prefix_key(written)) {
            self.error(
                name.value.span,
                format!(
                    "The prefix '{}:' is declared twice: the first declaration holds",
                    shown(written)
                ),
            );
            return None;
        }
        let w = self.collapsed(&iri.value);
        let value = self.any_uri(&w, self.base()).ok()?;
        self.joins(&w, value, self.joined());
        let resolved = match value {
            Value::Full(s) => s,
            Value::Based { base, reference } => {
                let reference = self.terms.str(self.text, reference).to_string();
                let r = oxiri::IriRef::parse(reference.as_str()).ok()?;
                self.resolved_str(base, &r, w.span)?
            }
            Value::Abbreviated { .. } => unreachable!("an xsd:anyURI is no abbreviated IRI"),
        };
        self.empty(tag).ok()?;
        let end = if self.end.implied {
            self.last
        } else {
            self.end.span.end
        };
        Some(Declared {
            name: written.to_string(),
            iri: resolved,
            span: Span::new(tag.span.start, end),
            name_span: name.value.span,
            iri_span: w.span,
        })
    }

    /// Bind the prefix `d` declares.
    fn declare(&mut self, d: Declared) {
        let iri = self.terms.str(self.text, d.iri);
        let context = context(iri);
        let iri = Iri::intern(iri);
        self.terms.prefixes.push(d.iri);
        let index = self.terms.prefixes.len() as u32 - 1;
        self.prefixes
            .insert(prefix_key(&d.name).into(), Prefix { index, context });
        self.doc.prefixes.push(PrefixDecl {
            name: d.name,
            iri,
            span: d.span,
            name_span: d.name_span,
            iri_span: d.iri_span,
        });
    }
}

/// A `Prefix` element read whole: its name, its IRI, and the spans of the
/// element and of its two values.
struct Declared {
    name: String,
    iri: Str,
    span: Span,
    name_span: Span,
    iri_span: Span,
}

fn is_pn_chars_base(c: char) -> bool {
    matches!(c,
        'A'..='Z' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

fn is_pn_chars_u(c: char) -> bool {
    is_pn_chars_base(c) || c == '_'
}

fn is_pn_chars(c: char) -> bool {
    is_pn_chars_u(c)
        || matches!(c, '-' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// SPARQL's `PN_PREFIX`, the schema's pattern of a prefix name.
fn is_pn_prefix(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_pn_chars_base)
        && !s.ends_with('.')
        && chars.all(|c| is_pn_chars(c) || c == '.')
}

/// SPARQL 1.1's `PN_LOCAL` without backslash escapes (`:` and `%XX`
/// inside), as the Functional frontend reads a local name: a superset of
/// the SPARQL 1.0 one the schema cites.
fn is_pn_local(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    let mut first = true;
    let mut last_dot = false;
    while i < s.len() {
        let c = s[i..].chars().next().unwrap_or('\0');
        let ok = match c {
            '%' => {
                let hex = b
                    .get(i + 1..i + 3)
                    .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit));
                if !hex {
                    return false;
                }
                i += 3;
                first = false;
                last_dot = false;
                continue;
            }
            '.' => !first,
            ':' => true,
            c if first => is_pn_chars_u(c) || c.is_ascii_digit(),
            c => is_pn_chars(c),
        };
        if !ok {
            return false;
        }
        last_dot = c == '.';
        first = false;
        i += c.len_utf8();
    }
    !last_dot
}

/// A short IRI that ends in the same part of an IRI as `ns`, a valid IRI,
/// as the Functional grammar's: `ns` followed by a local name is an IRI if
/// and only if the context followed by it is one, so the time to check an
/// expansion is that of its local name, however long the prefix IRI.
fn context(ns: &str) -> &'static str {
    let Ok(iri) = oxiri::Iri::parse(ns) else {
        return "a:";
    };
    if iri.fragment().is_some() {
        return "a:#";
    }
    if iri.query().is_some() {
        return "a:?";
    }
    match iri.authority() {
        Some(authority) if iri.path().is_empty() => {
            let host = authority
                .split_once('@')
                .map_or(authority, |(_, host)| host);
            match host.rsplit_once(']') {
                Some((_, "")) => "a://[::]",
                Some(_) => "a://h:",
                None if host.contains(':') => "a://h:",
                None => "a://h",
            }
        }
        _ => "a:",
    }
}

#[cfg(test)]
mod joins {
    use super::BaseShape;

    /// Whether joining a reference to a base as strings gives its
    /// resolution, decided from the base's shape and the reference alone,
    /// is what resolving and joining give, over bases and references of
    /// every shape.
    #[test]
    fn the_shape_of_a_base_decides_as_resolving_does() {
        let bases = [
            "http://example.org/lib",
            "http://example.org/lib/",
            "http://example.org",
            "http://example.org/",
            "http://example.org/lib#f",
            "http://example.org/lib/#f",
            "http://example.org/a/lib?x=1",
            "http://example.org/lib/?x",
            "http://example.org/a/./b/",
            "http://example.org/a/../",
            "http://example.org/a/..",
            "http://example.org/.",
            "urn:",
            "urn:x",
            "urn:a/",
            "http:",
            "file:///w/",
            "file:///w/o.owx",
            "tag:example.org,2026:",
        ];
        let references = [
            "",
            "#a",
            "?q",
            "?q#f",
            "/B",
            "/a/../b",
            "/./b",
            "//other.org/x",
            "//other.org",
            "//other.org/a/../b",
            "Glued",
            "a/b",
            "./x",
            "../x",
            "x/.",
            "x/..",
            ".",
            "..",
            "a?q",
            "a#f",
            "%41",
            ";p",
            "a//b",
            "a/.b",
            "..x",
            "/",
            "//",
        ];
        for b in bases {
            let shape = BaseShape::of(b);
            for r in references {
                let mut resolved = String::new();
                oxiri::Iri::parse(b)
                    .unwrap()
                    .resolve_into(&oxiri::IriRef::parse(r).unwrap(), &mut resolved)
                    .unwrap();
                let joined = format!("{b}{r}");
                assert_eq!(
                    shape.joins_alike(r),
                    joined == resolved,
                    "<{b}> and <{r}>: resolved <{resolved}>, joined <{joined}>"
                );
            }
        }
    }
}
