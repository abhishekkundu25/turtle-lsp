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
//! mapper resolves it ([`crate::ofn::rdf::IriValue::Based`]). An
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
//! [`MAX_ERRORS`], the last then saying so.
//!
//! Not read yet, an error that breaks the item: literals, anonymous
//! individuals, `IRI` and `AbbreviatedIRI` elements (so annotations), and
//! the constructs with a cardinality, facets, keys or data properties
//! before a range.

use super::CODE;
use crate::ofn::rdf::{IRI_BYTES_PER_BYTE, MIN_IRI_BYTES};
use crate::ofn::signature::{Cat, Entity, Rep, is_axiom, keyword_in, signature};
use crate::ofn::tree::{Document, Keyword, Node, NodeKind};
use crate::ofn::{MAX_ERRORS, shown};
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

/// What the tree does not hold of a node, which the source answers from.
#[derive(Clone, Copy, Debug)]
pub(super) enum Term {
    /// A construct, and its element's name as written.
    Construct { name: Span },
    /// An IRI, and how the text at its span spells it.
    Iri { value: Value, form: Form },
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
    /// The bases in force somewhere, each an absolute IRI.
    pub(super) bases: Vec<Str>,
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

/// The element named `local` in the OWL namespace. The SWRL keywords, which
/// OWL API writes in Functional syntax, are not in the Recommendation.
fn element(local: &str) -> Option<Element> {
    use Keyword as K;
    let leaf = match local {
        "IRI" => Leaf::Iri,
        "AbbreviatedIRI" => Leaf::AbbreviatedIri,
        "AnonymousIndividual" => Leaf::AnonymousIndividual,
        "Literal" => Leaf::Literal,
        "FacetRestriction" => Leaf::FacetRestriction,
        _ => {
            return Keyword::of(local)
                .filter(|k| {
                    !matches!(
                        k,
                        K::DLSafeRule
                            | K::Body
                            | K::Head
                            | K::ClassAtom
                            | K::DataRangeAtom
                            | K::ObjectPropertyAtom
                            | K::DataPropertyAtom
                            | K::BuiltInAtom
                            | K::SameIndividualAtom
                            | K::DifferentIndividualsAtom
                            | K::Variable
                    )
                })
                .map(Element::Keyword);
        }
    };
    Some(Element::Leaf(leaf))
}

/// Whether a slot of `cat` takes the element `leaf`.
fn takes(cat: Cat, leaf: Leaf) -> bool {
    match leaf {
        Leaf::Literal => matches!(cat, Cat::Literal | Cat::AnnotationValue),
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

/// Whether construct `k` has a slot of a shape the reader does not read
/// yet.
fn not_read_yet(k: Keyword) -> bool {
    signature(k).iter().any(|&(cat, _)| {
        matches!(
            cat,
            Cat::Integer
                | Cat::Facets
                | Cat::ObjectKeys
                | Cat::DataKeys
                | Cat::DataPropertiesThenRange
        )
    })
}

/// What a slot of `cat` is called in an error.
fn expected(cat: Cat) -> &'static str {
    match cat {
        Cat::Iri(Some(e)) => e.name(),
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
    /// The bases of the open elements that have an `xml:base`: the depth
    /// of each, and its base.
    scopes: Vec<(u32, Base)>,
    /// The base outside every `xml:base`: the document's URL.
    outer: Base,
    prefixes: FxHashMap<Box<str>, Prefix>,
    errors: Vec<SyntaxError>,
    /// Every error reported, those past [`MAX_ERRORS`] too.
    reported: usize,
    /// Where the first error past [`MAX_ERRORS`] is.
    dropped: Option<Span>,
    /// The item being read used a base in error.
    tainted: bool,
    /// The bytes of the bases and prefix IRIs resolved, and the most
    /// allowed; past it, every item is broken.
    resolved: usize,
    budget: usize,
    over: bool,
}

impl<'a> Reader<'a> {
    fn new(text: &'a str, prolog: &'a Prolog, base: Option<&str>) -> Reader<'a> {
        let mut terms = Terms::default();
        let outer = match base.filter(|b| oxiri::Iri::parse(*b).is_ok()) {
            Some(b) => {
                terms.arena.push_str(b);
                terms.bases.push(Str::Arena(0, b.len() as u32));
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
            },
            scopes: Vec::new(),
            outer,
            prefixes: FxHashMap::default(),
            errors: Vec::new(),
            reported: 0,
            dropped: None,
            tainted: false,
            resolved: 0,
            budget: text.len().saturating_mul(IRI_BYTES_PER_BYTE) + MIN_IRI_BYTES,
            over: false,
        }
    }

    fn finish(self) -> (Document, Terms) {
        let Reader {
            scanner,
            mut doc,
            terms,
            errors,
            dropped,
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

    /// Report `message` at `span`, and stop.
    fn stop<T>(&mut self, span: Span, message: impl Into<Cow<'static, str>>) -> R<T> {
        self.error(span, message);
        Err(Stop)
    }

    // ---- events ----

    /// The next event: an element's `xml:base` is read at its start, and
    /// out of scope at its end.
    fn next(&mut self) -> Event<'a> {
        let e = self.scanner.next();
        match &e {
            Event::Start(tag) => {
                self.depth += 1;
                self.last = tag.span.end;
                self.base_of(tag);
            }
            Event::End(end) => {
                self.depth = self.depth.saturating_sub(1);
                if !end.implied {
                    self.last = end.span.end;
                }
                self.end = *end;
                while self.scopes.last().is_some_and(|s| s.0 > self.depth) {
                    self.scopes.pop();
                }
            }
            Event::Eof => {
                self.end = End {
                    span: Span::new(self.last, self.last),
                    implied: true,
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
                    let message =
                        format!("Text in <{}>, which holds elements only", parent.name.raw);
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
                tag.name.raw, child.name.raw
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
                        tag.name.raw, child.name.raw
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

    fn at(&self, span: Span) -> &'a str {
        &self.text[span.start as usize..span.end as usize]
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
                            "The prefix '{prefix}:' of the attribute {} is not declared",
                            n.raw
                        ),
                    );
                    bad = true;
                }
                Ns::Unbound => match names.iter().position(|&x| x == n.raw) {
                    Some(k) => {
                        out[k] = out[k].or(Some(a));
                    }
                    None => {
                        let message = format!("<{}> has no attribute {}", tag.name.raw, n.raw);
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
                None => format!("<{}> is no element of OWL/XML", n.raw),
            },
            Ns::Bound(ns) => format!(
                "<{}> is in the namespace {}, not in OWL's ({OWL})",
                n.raw,
                shown(ns)
            ),
            Ns::Unbound => format!("<{}> is in no namespace: OWL/XML's are in {OWL}", n.raw),
            Ns::Unknown => format!(
                "The prefix '{}:' of <{}> is not declared",
                n.prefix().unwrap_or(""),
                n.raw
            ),
        };
        self.stop(n.span, message)
    }

    // ---- bases ----

    fn base(&self) -> Base {
        self.scopes.last().map_or(self.outer, |s| s.1)
    }

    /// The `xml:base` of `tag`, if it has one, in scope from here to its
    /// end: resolved against the base outside it.
    fn base_of(&mut self, tag: &Tag<'a>) {
        let Some(a) = tag
            .attrs
            .iter()
            .find(|a| a.name.ns() == Some(xml::XML_NS) && a.name.local() == "base")
        else {
            return;
        };
        let v = Written::of(&a.value);
        let base = match self.base() {
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
                        self.terms.bases.push(s);
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
        };
        self.scopes.push((self.depth, base));
    }

    /// `reference` resolved against base `b`, kept as a new base; `None`
    /// past the budget.
    fn resolve(&mut self, b: u32, reference: &oxiri::IriRef<&str>, at: Span) -> Option<u32> {
        let resolved = self.resolved_str(b, reference, at)?;
        self.terms.bases.push(resolved);
        Some(self.terms.bases.len() as u32 - 1)
    }

    /// `reference` resolved against base `b`, in the arena; `None` past the
    /// budget.
    fn resolved_str(&mut self, b: u32, reference: &oxiri::IriRef<&str>, at: Span) -> Option<Str> {
        let base = self.terms.str(self.text, self.terms.bases[b as usize]);
        let cost = base.len() + reference.as_str().len();
        if self.over || self.resolved + cost > self.budget {
            self.over_limit(at);
            return None;
        }
        self.resolved += cost;
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
        let Some(p) = self.prefixes.get(name) else {
            let iri = STANDARD.iter().find(|s| s.0 == name).map_or("…", |s| s.1);
            let message = format!(
                "Undeclared prefix '{name}:': an OWL/XML document declares each prefix it uses, as <Prefix name=\"{name}\" IRI=\"{iri}\"/>"
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
        let i = self.doc.nodes.len();
        self.doc.nodes.push(Node {
            kind: NodeKind::Iri { prefix_len },
            span: w.span,
            next: i as u32 + 1,
        });
        self.terms.terms.push(Term::Iri { value, form });
        i
    }

    /// The IRI node of an entity element, from its `IRI` or its
    /// `abbreviatedIRI`, one of them.
    fn entity_iri(&mut self, tag: &Tag<'a>) -> R<usize> {
        let [iri, abbreviated] = self.attrs(tag, ["IRI", "abbreviatedIRI"])?;
        let (w, value) = match (iri, abbreviated) {
            (Some(a), None) => {
                let w = Written::of(&a.value);
                let value = self.any_uri(&w, self.base())?;
                (w, value)
            }
            (None, Some(a)) => {
                let w = Written::of(&a.value);
                let value = self.abbreviated(&w)?;
                (w, value)
            }
            (Some(_), Some(a)) => {
                let message = format!(
                    "<{}> has an IRI and an abbreviatedIRI: one of them, not both",
                    tag.name.raw
                );
                return self.stop(a.name.span, message);
            }
            (None, None) => {
                let message = format!(
                    "<{}> needs the attribute IRI or abbreviatedIRI",
                    tag.name.raw
                );
                return self.stop(tag.name.span, message);
            }
        };
        Ok(self.iri_node(&w, value))
    }

    // ---- constructs ----

    fn open(&mut self, k: Keyword, tag: &Tag<'a>) -> usize {
        self.doc.nodes.push(Node {
            kind: NodeKind::Construct(k),
            span: Span::new(tag.span.start, tag.span.start),
            next: 0,
        });
        self.terms.terms.push(Term::Construct {
            name: tag.name.span,
        });
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
        if not_read_yet(k) {
            let message = format!("<{}> is not read yet", tag.name.raw);
            return self.stop(tag.name.span, message);
        }
        if Entity::of(k).is_some() {
            // Inside `Declaration`: the entity and its IRI.
            self.entity_iri(tag)?;
            return self.empty(tag);
        }
        self.attrs(tag, [])?;
        let slots = signature(k);
        let (mut s, mut taken) = (0, 0);
        while let Some(child) = self.child(tag)? {
            let el = self.element_of(&child)?;
            loop {
                let Some(&(cat, rep)) = slots.get(s) else {
                    let message = format!(
                        "Expected the end of <{}>, found <{}>",
                        tag.name.raw, child.name.raw
                    );
                    return self.stop(child.name.span, message);
                };
                let open = matches!(rep, Rep::AtLeast(_)) || taken == 0;
                if open && self.operand(cat, el, &child)? {
                    taken += 1;
                    break;
                }
                let filled = match rep {
                    Rep::One => taken == 1,
                    Rep::Optional => true,
                    Rep::AtLeast(n) => taken >= n as usize,
                };
                if !filled {
                    return self.unfilled(tag, cat, rep, taken, Some(&child));
                }
                (s, taken) = (s + 1, 0);
            }
        }
        if self.end.implied {
            // The scanner reported why.
            return Err(Stop);
        }
        for &(cat, rep) in &slots[s.min(slots.len())..] {
            let filled = match rep {
                Rep::One => taken == 1,
                Rep::Optional => true,
                Rep::AtLeast(n) => taken >= n as usize,
            };
            if !filled {
                return self.unfilled(tag, cat, rep, taken, None);
            }
            taken = 0;
        }
        Ok(())
    }

    /// The error of a slot left unfilled before `child`, or before the end.
    fn unfilled(
        &mut self,
        tag: &Tag<'a>,
        cat: Cat,
        rep: Rep,
        taken: usize,
        child: Option<&Tag<'a>>,
    ) -> R {
        if let Rep::AtLeast(n) = rep {
            let message = format!(
                "<{}> needs at least {}, found {taken}",
                tag.name.raw,
                cat.counted(n)
            );
            return self.stop(tag.name.span, message);
        }
        match child {
            Some(c) => {
                let message = format!("Expected {}, found <{}>", expected(cat), c.name.raw);
                self.stop(c.name.span, message)
            }
            None => {
                let message = format!("Expected {}, found </{}>", expected(cat), tag.name.raw);
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
            Element::Leaf(leaf) if takes(cat, leaf) => {
                let message = format!("<{}> is not read yet", child.name.raw);
                return self.stop(child.name.span, message);
            }
            Element::Keyword(_) | Element::Leaf(_) => return Ok(false),
        }
        Ok(true)
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
                    root.name.raw
                ),
                _ if root.name.local() == "Ontology" => format!(
                    "The root element <{}> is not in OWL's namespace: OWL/XML's root is <Ontology> in {OWL}",
                    root.name.raw
                ),
                _ => format!(
                    "The root element is <{}>: the text is XML but not OWL/XML, whose root is <Ontology> in {OWL}",
                    root.name.raw
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
                    let message = format!("Text in <{}>, which holds elements only", root.name.raw);
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
            let w = Written::of(&a.value);
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
                    tag.name.raw
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

    /// `Import`: its IRI, the element's trimmed content.
    fn import(&mut self, tag: &Tag<'a>) -> R {
        let i = self.open(Keyword::Import, tag);
        let r = (|| {
            self.attrs(tag, [])?;
            // The element's own, out of scope once its end is read.
            let base = self.base();
            let Some(w) = self.content(tag)? else {
                let message = format!("Expected an IRI, found </{}>", tag.name.raw);
                return self.stop(self.end.span, message);
            };
            let value = self.any_uri(&w, base)?;
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
                format!("<{}> needs the attribute {missing}", tag.name.raw),
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
        if self.prefixes.contains_key(written) {
            self.error(
                name.value.span,
                format!("The prefix '{written}:' is declared twice: the first declaration holds"),
            );
            return None;
        }
        let w = Written::of(&iri.value);
        let value = self.any_uri(&w, self.base()).ok()?;
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
            .insert(d.name.as_str().into(), Prefix { index, context });
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
