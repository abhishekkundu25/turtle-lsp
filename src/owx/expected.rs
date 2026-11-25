//! What may come at an offset of an OWL/XML document, for completion: the
//! elements whose name may start after a `<`, or the IRIs an `IRI`,
//! `abbreviatedIRI` or `datatypeIRI` value, or the text of an `IRI` or
//! `AbbreviatedIRI` element, may be. And where a `<Prefix>` element that
//! declares a new prefix goes.
//!
//! The cursor ([`crate::rdfxml::xml::cursor`]) reads what is open at the
//! offset, so a document half typed is read as far as it goes. Inside the
//! root `Ontology`, its children before and after the offset say which part
//! of the schema's order comes there ([`Phase`]): no `<Prefix>` after an
//! import, an annotation or an axiom, and no import, annotation or axiom
//! before a `<Prefix>`. Inside a construct, its children so far fill its signature
//! through the slot machine of Functional syntax ([`Construct`]), read as
//! the reader reads them ([`super::reader`]):
//! - a keyword's element is a construct where a slot takes it as one, else
//!   an entity's element is an IRI (as anywhere outside `Declaration`);
//! - `IRI` and `AbbreviatedIRI` are IRIs, `Literal` a literal,
//!   `AnonymousIndividual` a node ID, `FacetRestriction` a facet and its
//!   literal, and an element of no slot fills one as an error does;
//! - a cardinality, and the IRI of an entity, a variable or a built-in atom,
//!   come from the start tag, before the children; an import's IRI is its
//!   text;
//! - a `HasKey`'s properties come flat, without the `( … )` of Functional
//!   syntax: a key list ends where an element of another kind comes;
//! - a `Datatype` after the data properties of `DataSomeValuesFrom` or
//!   `DataAllValuesFrom` is its data range, which Functional syntax cannot
//!   tell from one more property.
//!
//! An element's name is expected where a new one is typed: its tag cut
//! after the name, by the end of the text or the next tag. The slots that
//! may come say which elements may start: the keywords'
//! constructs, the entities' elements, and the leaves their categories take.
//! The IRIs of an attribute value are those of its element's kind
//! (`<Class IRI="…"/>` a class, a `datatypeIRI` a datatype); an `IRI` or
//! `AbbreviatedIRI` element takes any. Nothing is expected in a comment, the
//! prolog, an end tag, an attribute name, a literal's text, any other value
//! (a `nodeID`, a prefix's), a value cut by a line break, an `IRI`
//! element's text past its first run, or outside an `Ontology` in OWL's
//! namespace.

pub(crate) use super::reader::{Element, Leaf};
use super::reader::{Phase, element, takes};
use crate::ofn::expected::{Construct, Iris, Keys, Operand, items};
use crate::ofn::signature::{Cat, Entity, keyword_in, signature};
use crate::ofn::tree::Keyword;
use crate::rdfxml::xml::cursor::{self, At, Document};
use crate::rdfxml::xml::{self, Name};
use crate::syntax::names::newline;
use crate::syntax::{Parsed, Span};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// The namespace of every OWL/XML element.
const OWL: &str = "http://www.w3.org/2002/07/owl#";

/// XML's white space (XML 1.0, 2.3).
const WHITE: &[char] = &[' ', '\t', '\n', '\r'];

/// What may come at an offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Expected {
    Elements(Names),
    /// The name of the innermost open element after `</`.
    EndTag {
        span: Span,
        name: String,
        closed: bool,
    },
    /// Valid attributes for the start tag being written.
    Attributes {
        span: Span,
        attrs: Vec<&'static str>,
    },
    /// An IRI.
    Iri {
        /// What its edit replaces: a closed value between its quotes (an
        /// unclosed one to the first white space after the offset, or to
        /// where the scanner ends it), or an element's text without its
        /// white space.
        span: Span,
        iris: Iris,
        form: Form,
    },
}

/// The elements whose name may be typed after a `<`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Names {
    /// The name typed there, to its end.
    pub(crate) name: Span,
    /// What OWL's elements are written with there: `""` where OWL's
    /// namespace is the default one, else `p:`.
    pub(crate) prefix: String,
    pub(crate) elements: Vec<Element>,
}

/// How an IRI is written where it is typed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Form {
    Full,
    /// `prefix:local`, with a prefix a `<Prefix>` element declares (none is
    /// built in): a new one is declared after the others, or after the
    /// start tag of the root, `root` ([`declare`]).
    Abbreviated {
        root: Span,
    },
}

/// What may come at byte `offset` of `text`; `None` where nothing is typed.
pub(crate) fn at(text: &str, offset: usize) -> Option<Expected> {
    let doc = Document::read(text);
    let c = doc.at(offset);
    if c.open.is_empty() {
        if let At::AttrName(a) = &c.at
            && a.tag.name.is(OWL, "Ontology")
        {
            let attrs = ontology_attrs(&a.tag);
            if !attrs.is_empty() {
                return Some(Expected::Attributes {
                    span: a.span,
                    attrs,
                });
            }
        }
        return None;
    }
    let (root, inner) = c.open.split_first()?;
    if !root.tag.name.is(OWL, "Ontology") {
        return None;
    }
    let form = |abbreviated: bool| {
        if abbreviated {
            Form::Abbreviated {
                root: root.tag.span,
            }
        } else {
            Form::Full
        }
    };
    match &c.at {
        At::Name(name) => {
            // A new element: its tag cut after the name, by the end of the
            // text or the next tag. A name changed in a whole tag would
            // leave its attributes or its end tag behind.
            let rest = text[name.end as usize..].trim_start_matches(WHITE);
            if !(rest.is_empty() || rest.starts_with('<')) {
                return None;
            }
            let elements = match inner.last() {
                None => ontology(text, name.end as usize, root, &c.prefixes),
                Some(e) => within(e)?,
            };
            Some(Expected::Elements(Names {
                name: *name,
                prefix: owl_prefix(&text[name.start as usize..name.end as usize], &c.prefixes),
                elements,
            }))
        }
        At::EndName { span, closed } => {
            let last = c.open.last()?;
            if last.tag.name.entity || !xml::is_name(last.tag.name.raw) {
                return None;
            }
            let outer: Vec<&str> = c.open[..c.open.len() - 1]
                .iter()
                .map(|e| e.tag.name.raw)
                .collect();
            if span.start < span.end {
                let existing = text.get(span.start as usize..span.end as usize)?;
                if outer.contains(&existing) {
                    return None;
                }
            }
            if cursor::is_already_closed(text, span.end as usize, &outer, last.tag.name.raw) {
                return None;
            }
            Some(Expected::EndTag {
                span: *span,
                name: last.tag.name.raw.to_string(),
                closed: *closed,
            })
        }
        At::AttrName(a) => {
            let attrs = tag_attrs(&a.tag)?;
            if attrs.is_empty() {
                return None;
            }
            Some(Expected::Attributes {
                span: a.span,
                attrs,
            })
        }
        At::Value(v) => {
            let (iris, abbreviated) = value(&v.tag.name, &v.attr.name)?;
            // A value with no closing quote runs to where the scanner ends
            // it (a `<`, the end of the text): the reader reads it whole.
            let closed = v.attr.span.end > v.attr.value.span.end;
            let end = v.span.end.max(v.attr.value.span.end);
            let span = typed(text, Span::new(v.span.start, end), offset, closed)?;
            Some(Expected::Iri {
                span,
                iris,
                form: form(abbreviated),
            })
        }
        At::Text(span) => {
            let e = inner.last()?;
            // Its first run only, after its start tag: past a wrong end tag
            // the element is still open, and its text is all its runs.
            if span.start != e.tag.span.end {
                return None;
            }
            let abbreviated = match owl(&e.tag.name)? {
                Element::Leaf(Leaf::Iri) => false,
                Element::Leaf(Leaf::AbbreviatedIri) => true,
                _ => return None,
            };
            let s = &text[span.start as usize..span.end as usize];
            let start = span.start + (s.len() - s.trim_start_matches(WHITE).len()) as u32;
            let end = span.start + s.trim_end_matches(WHITE).len() as u32;
            let o = offset as u32;
            let span = typed(text, Span::new(start.min(o), end.max(o)), offset, true)?;
            Some(Expected::Iri {
                span,
                iris: Iris::Any,
                form: form(abbreviated),
            })
        }
        At::Elsewhere => None,
    }
}

/// `span`, where an IRI is typed up to `offset`: whole when `closed` (a
/// single-line value between its quotes, or an element's single-line trimmed
/// text), else cut at the first white space after `offset`. `None` when
/// there is white space before `offset` (a value not closed, the offset on
/// the next line), when a closed value spans lines (LSP completion edits are
/// single-line), or when an unclosed value starts with white space before
/// more text (cutting there would leave that text after the inserted IRI).
fn typed(text: &str, span: Span, offset: usize, closed: bool) -> Option<Span> {
    let (start, end) = (span.start as usize, span.end as usize);
    if text[start..offset].contains(WHITE) {
        return None;
    }
    if closed {
        if text[start..end].contains(['\n', '\r']) {
            return None;
        }
        return Some(span);
    }
    let rest = &text[offset..end];
    let cut = rest.find(WHITE).unwrap_or(rest.len());
    if offset == start && cut == 0 && !rest.trim_start_matches(WHITE).is_empty() {
        return None;
    }
    Some(Span::new(span.start, (offset + cut) as u32))
}

/// The element `name` names in OWL's namespace, if it is one.
fn owl(name: &Name<'_>) -> Option<Element> {
    if name.ns() != Some(OWL) {
        return None;
    }
    element(name.local())
}

fn has_attr(tag: &xml::Tag<'_>, name: &str) -> bool {
    match name.strip_prefix("xml:") {
        Some(local) => tag
            .attrs
            .iter()
            .any(|a| a.name.ns() == Some(xml::XML_NS) && a.name.local() == local),
        None => tag
            .attrs
            .iter()
            .any(|a| a.name.prefix().is_none() && a.name.raw == name),
    }
}

fn ontology_attrs(tag: &xml::Tag<'_>) -> Vec<&'static str> {
    ["ontologyIRI", "versionIRI", "xml:base", "xml:lang"]
        .into_iter()
        .filter(|n| !has_attr(tag, n))
        .collect()
}

fn tag_attrs(tag: &xml::Tag<'_>) -> Option<Vec<&'static str>> {
    let el = owl(&tag.name)?;
    let list: &[&str] = match el {
        Element::Keyword(k)
            if Entity::of(k).is_some() || matches!(k, Keyword::Variable | Keyword::BuiltInAtom) =>
        {
            if has_attr(tag, "IRI") || has_attr(tag, "abbreviatedIRI") {
                &[]
            } else {
                &["IRI", "abbreviatedIRI"]
            }
        }
        Element::Keyword(Keyword::Prefix) => &["name", "IRI"],
        Element::Keyword(k) if from_tag(k) == Some(Operand::Integer) => {
            &["cardinality", "xml:lang"]
        }
        Element::Leaf(Leaf::Literal) => &["datatypeIRI", "xml:lang"],
        Element::Leaf(Leaf::FacetRestriction) => &["facet", "xml:lang"],
        Element::Leaf(Leaf::AnonymousIndividual) => &["nodeID"],
        Element::Keyword(_) | Element::Leaf(_) => &["xml:lang"],
    };
    Some(list.iter().copied().filter(|n| !has_attr(tag, n)).collect())
}

/// The IRIs a value of attribute `attr` of element `tag` may be, and
/// whether it is abbreviated: an entity's `IRI` or `abbreviatedIRI`, a
/// literal's `datatypeIRI`.
fn value(tag: &Name<'_>, attr: &Name<'_>) -> Option<(Iris, bool)> {
    if attr.prefix().is_some() {
        return None;
    }
    match (owl(tag)?, attr.raw) {
        (Element::Keyword(k), "IRI" | "abbreviatedIRI") => {
            let e = Entity::of(k)?;
            Some((Iris::Of(vec![e]), attr.raw == "abbreviatedIRI"))
        }
        (Element::Leaf(Leaf::Literal), "datatypeIRI") => {
            Some((Iris::Of(vec![Entity::Datatype]), false))
        }
        _ => None,
    }
}

/// How OWL's elements are written where `typed` is: with its own prefix if
/// that is OWL's, else without one if OWL's is the default namespace, else
/// with a prefix bound to it.
fn owl_prefix(typed: &str, prefixes: &BTreeMap<&str, Cow<'_, str>>) -> String {
    let is_owl = |p: &str| prefixes.get(p).is_some_and(|ns| ns == OWL);
    let own = typed.split_once(':').map(|(p, _)| p).filter(|p| is_owl(p));
    let p = own
        .or_else(|| is_owl("").then_some(""))
        .or_else(|| prefixes.iter().find(|(_, ns)| *ns == OWL).map(|(p, _)| *p))
        .unwrap_or("");
    if p.is_empty() {
        String::new()
    } else {
        format!("{p}:")
    }
}

/// The children of `Ontology` that may come at byte `after`: from the part
/// of the schema's order the last child before it is in, up to the earliest
/// in-order child after it.
fn ontology(
    text: &str,
    after: usize,
    root: &cursor::Element<'_>,
    prefixes: &BTreeMap<&str, Cow<'_, str>>,
) -> Vec<Element> {
    let min = root
        .children
        .iter()
        .filter_map(|c| owl(c).and_then(Phase::of))
        .max()
        .unwrap_or(Phase::Prefixes);
    let max = next_phase(text, after, root.tag.name.raw, min, prefixes);
    std::iter::once(Keyword::Prefix)
        .chain(items())
        .map(Element::Keyword)
        .filter(|&el| Phase::of(el).is_some_and(|p| (min..=max).contains(&p)))
        .collect()
}

/// The earliest phase of an in-order top-level child of `root` after byte
/// `from` (one whose phase is at least `min`, since any earlier child is
/// already misplaced). `Phase::Axioms` when none follows.
fn next_phase(
    text: &str,
    from: usize,
    root: &str,
    min: Phase,
    prefixes: &BTreeMap<&str, Cow<'_, str>>,
) -> Phase {
    if min == Phase::Axioms {
        return Phase::Axioms;
    }
    let b = text.as_bytes();
    let mut i = from;
    let mut stack: Vec<&str> = Vec::new();
    let mut skip = 0usize;
    while let Some(lt) = text[i..].find('<') {
        let at = i + lt;
        let rest = &text[at..];
        if let Some((open, close)) = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
            .into_iter()
            .find(|(open, _)| rest.starts_with(open))
        {
            let body = &rest[open.len()..];
            i = at + open.len() + body.find(close).map_or(body.len(), |e| e + close.len());
            continue;
        }
        if let Some(after) = rest.strip_prefix("</") {
            let len = after.find(|c| !xml::is_name_char(c)).unwrap_or(after.len());
            let end = &after[..len];
            let tail = &after[len..];
            i = at
                + 2
                + len
                + tail.find(['<', '>']).map_or(tail.len(), |p| {
                    if tail.as_bytes()[p] == b'>' { p + 1 } else { p }
                });
            if skip > 0 {
                skip -= 1;
            } else if let Some(k) = stack.iter().rposition(|n| *n == end) {
                stack.truncate(k);
            } else if stack.is_empty() && end == root {
                break;
            }
            continue;
        }
        let after = &rest[1..];
        if !after.starts_with(xml::is_name_start_char) {
            i = at + 1;
            continue;
        }
        let name_len = after.find(|c| !xml::is_name_char(c)).unwrap_or(after.len());
        let qname = &after[..name_len];
        let attrs_start = at + 1 + name_len;
        let mut j = attrs_start;
        let mut quote = None;
        let mut prev = 0u8;
        let mut empty = false;
        while j < b.len() {
            match (quote, b[j]) {
                (_, b'<') => break,
                (Some(q), c) if c == q => quote = None,
                (Some(_), _) => {}
                (None, c @ (b'"' | b'\'')) => quote = Some(c),
                (None, b'>') => {
                    empty = prev == b'/';
                    j += 1;
                    break;
                }
                _ => {}
            }
            prev = b[j];
            j += 1;
        }
        let attrs_end = if j > attrs_start && b[j - 1] == b'>' {
            j - 1 - usize::from(empty)
        } else {
            j
        };
        if stack.is_empty() && skip == 0 {
            let attrs = &text[attrs_start..attrs_end];
            if let Some(local) = owl_local(qname, attrs, prefixes)
                && let Some(p) = element(local).and_then(Phase::of)
                && p >= min
            {
                return p;
            }
        }
        if !empty {
            if stack.len() < xml::MAX_DEPTH {
                stack.push(qname);
            } else {
                skip += 1;
            }
        }
        i = j;
    }
    Phase::Axioms
}

/// The local name of start tag `qname` (with raw attributes `attrs`) if it is
/// in OWL's namespace, using its own `xmlns` declarations and `prefixes`.
fn owl_local<'a>(
    qname: &'a str,
    attrs: &str,
    prefixes: &BTreeMap<&str, Cow<'_, str>>,
) -> Option<&'a str> {
    let (pfx, local) = qname.split_once(':').unwrap_or(("", qname));
    let own = attrs
        .contains("xmlns")
        .then(|| {
            if pfx.is_empty() {
                attribute(attrs, "xmlns")
            } else {
                attribute(attrs, &format!("xmlns:{pfx}"))
            }
        })
        .flatten();
    let is_owl = match own {
        Some(ns) => ns == OWL,
        None => prefixes.get(pfx).is_some_and(|ns| ns == OWL),
    };
    is_owl.then_some(local)
}

/// The elements that may come next in the open element `e`; `None` in an
/// element of no construct.
fn within(e: &cursor::Element<'_>) -> Option<Vec<Element>> {
    let k = match owl(&e.tag.name)? {
        Element::Keyword(k) => k,
        // Its one literal.
        Element::Leaf(Leaf::FacetRestriction) if e.children.is_empty() => {
            return Some(vec![Element::Leaf(Leaf::Literal)]);
        }
        Element::Leaf(_) => return Some(vec![]),
    };
    let mut c = Construct::new(k);
    if let Some(op) = from_tag(k) {
        c.take(op);
    }
    let children: Vec<Option<Element>> = e.children.iter().map(owl).collect();
    for &child in &children {
        feed(&mut c, child);
    }
    let (keywords, iris) = match c.keys() {
        // Before the first data property, either kind.
        Some(Keys::Object) => {
            let (mut keywords, iris) = Keys::Object.expects();
            let (data, more) = Keys::Data.expects();
            keywords.extend(data);
            (keywords, iris.with(more))
        }
        Some(Keys::Data) => Keys::Data.expects(),
        None if ranged(&c, children.last().copied().flatten()) => return Some(vec![]),
        None => c.expects(),
    };
    let mut out: Vec<Element> = keywords.into_iter().map(Element::Keyword).collect();
    if let Iris::Of(kinds) = iris {
        out.extend(kinds.into_iter().map(|e| Element::Keyword(entity(e))));
    }
    if c.keys().is_none() {
        for (cat, _) in c.slots() {
            out.extend(
                Leaf::ALL
                    .into_iter()
                    .filter(|&l| takes(cat, l))
                    .map(Element::Leaf),
            );
        }
    }
    let mut seen = Vec::with_capacity(out.len());
    out.retain(|el| {
        let new = !seen.contains(el);
        seen.push(*el);
        new
    });
    Some(out)
}

/// The operand of `k` its start tag gives, read before its children: a
/// cardinality; the IRI of an entity, a variable or a built-in atom; an
/// import's IRI, its text.
fn from_tag(k: Keyword) -> Option<Operand> {
    match signature(k).first() {
        Some((Cat::Integer, _)) => Some(Operand::Integer),
        _ if Entity::of(k).is_some()
            || matches!(
                k,
                Keyword::Variable | Keyword::BuiltInAtom | Keyword::Import
            ) =>
        {
            Some(Operand::Iri)
        }
        _ => None,
    }
}

/// Feed `c` the child element `el` (`None`: one not of OWL/XML).
fn feed(c: &mut Construct, el: Option<Element>) {
    // A key list ends where an element not of its kind comes.
    while let Some(keys) = c.keys() {
        if el.is_some_and(|el| of_keys(keys, el)) {
            return;
        }
        c.take(Operand::Group);
    }
    match el {
        Some(Element::Keyword(k)) => {
            let construct = c.slots().any(|(cat, _)| keyword_in(k, cat));
            let iri = !construct && Entity::of(k).is_some();
            c.take(if iri {
                Operand::Iri
            } else {
                Operand::Construct(k)
            });
        }
        Some(Element::Leaf(leaf)) => match leaf {
            Leaf::Iri | Leaf::AbbreviatedIri => c.take(Operand::Iri),
            Leaf::Literal => c.take(Operand::Literal),
            Leaf::AnonymousIndividual => c.take(Operand::NodeId),
            Leaf::FacetRestriction => {
                c.take(Operand::Iri);
                c.take(Operand::Literal);
            }
        },
        None => c.take(Operand::Reported),
    }
}

/// Whether `el` is a property of key list `keys`.
fn of_keys(keys: Keys, el: Element) -> bool {
    let Element::Keyword(k) = el else {
        return false;
    };
    let (keywords, iris) = keys.expects();
    keywords.contains(&k)
        || matches!((iris, Entity::of(k)), (Iris::Of(kinds), Some(e)) if kinds.contains(&e))
}

/// Whether the data properties of `c` are followed by their data range, a
/// `Datatype` element, the `last` child.
fn ranged(c: &Construct, last: Option<Element>) -> bool {
    last == Some(Element::Keyword(Keyword::Datatype))
        && c.slots()
            .next()
            .is_some_and(|(cat, _)| cat == Cat::DataPropertiesThenRange)
}

/// The keyword of the entity element of kind `e`.
fn entity(e: Entity) -> Keyword {
    Keyword::ALL
        .iter()
        .copied()
        .find(|&k| Entity::of(k) == Some(e))
        .expect("an element per kind of entity")
}

/// The local name of `el`.
pub(crate) fn name(el: Element) -> &'static str {
    match el {
        Element::Keyword(k) => k.as_str(),
        Element::Leaf(l) => l.name(),
    }
}

/// The attributes `el` is written with, and whether it is empty (no
/// children, no text): an entity, a variable or a built-in atom has its
/// IRI, a cardinality its `cardinality`, a prefix its name and IRI, a facet
/// its `facet`, an anonymous individual its `nodeID`.
pub(crate) fn form(el: Element) -> (&'static [&'static str], bool) {
    match el {
        Element::Keyword(k) if Entity::of(k).is_some() || k == Keyword::Variable => {
            (&["IRI"], true)
        }
        Element::Keyword(Keyword::BuiltInAtom) => (&["IRI"], false),
        Element::Keyword(Keyword::Prefix) => (&["name", "IRI"], true),
        Element::Keyword(k) if from_tag(k) == Some(Operand::Integer) => (&["cardinality"], false),
        Element::Leaf(Leaf::FacetRestriction) => (&["facet"], false),
        Element::Leaf(Leaf::AnonymousIndividual) => (&["nodeID"], true),
        Element::Keyword(_) | Element::Leaf(_) => (&[], false),
    }
}

/// The insertion that declares the prefix `name` as `ns`: a `<Prefix>`
/// element, in the prefix form of the root's name (`<owl:Prefix>` in
/// `<owl:Ontology>`), on a line of its own after the last one the reader
/// declared, indented as it is; with none, after the root's start tag
/// `root`, indented as the line after it. Only for a name [`declarable`]
/// says yes to.
pub(crate) fn declare(
    text: &str,
    parsed: &Parsed,
    root: Span,
    name: &str,
    ns: &str,
) -> (u32, String) {
    let tag = text
        .get(root.start as usize + 1..root.end as usize)
        .unwrap_or("");
    let mut qualified = tag.split(|c: char| WHITE.contains(&c) || c == '>' || c == '/');
    let owl = match qualified.next().and_then(|q| q.rsplit_once(':')) {
        Some((p, _)) => format!("{p}:"),
        None => String::new(),
    };
    let element = format!(
        "<{owl}Prefix name=\"{}\" IRI=\"{}\"/>",
        escaped(name),
        escaped(ns)
    );
    let root = root.end;
    let (at, indent) = match parsed.prefixes.last() {
        Some(p) => {
            let start = p.span.start as usize;
            let line = text[..start].rfind(['\n', '\r']).map_or(0, |i| i + 1);
            let indent = &text[line..start];
            let blank = indent.trim_start_matches([' ', '\t']).is_empty();
            (p.span.end, if blank { indent } else { "" })
        }
        None => {
            let after = &text[root as usize..];
            let gap = &after[..after.len() - after.trim_start_matches(WHITE).len()];
            let indent = gap.rfind(['\n', '\r']).map_or("", |i| &gap[i + 1..]);
            (root, indent)
        }
    };
    (at, format!("{}{indent}{element}", newline(text)))
}

/// Whether a `<Prefix>` element may be added to `text` for a prefix: `None`
/// for none at all, when the root's start tag `root` is not closed by `>`
/// (a `<` in a value cut it, and the element would go into the value);
/// else the names it may not be added for, those a `<Prefix>` element of
/// `text` names, misplaced or with an error included: a second one would be
/// declared twice.
pub(crate) fn declarable(text: &str, root: Span) -> Option<Vec<&str>> {
    if !text
        .get(..root.end as usize)
        .is_some_and(|t| t.ends_with('>'))
    {
        return None;
    }
    let named = text.match_indices("Prefix").filter_map(|(i, _)| {
        // `<Prefix` or `<p:Prefix`, then its attributes up to `>`.
        let head = &text[..i];
        let head = head.strip_suffix(':').map_or(head, |h| {
            h.trim_end_matches(|c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
        });
        if !head.ends_with('<') {
            return None;
        }
        let tag = &text[i + "Prefix".len()..];
        let tag = &tag[..tag.find('>').unwrap_or(tag.len())];
        attribute(tag, "name")
    });
    Some(named.collect())
}

/// The value of attribute `name` in `attrs`, the text of a start tag after
/// its name, as written.
fn attribute<'t>(attrs: &'t str, name: &str) -> Option<&'t str> {
    attrs.match_indices(name).find_map(|(i, _)| {
        if !attrs[..i].ends_with(WHITE) {
            return None;
        }
        let rest = attrs[i + name.len()..].trim_start_matches(WHITE);
        let rest = rest.strip_prefix('=')?.trim_start_matches(WHITE);
        let quote = rest.chars().next().filter(|&c| c == '"' || c == '\'')?;
        let value = &rest[1..];
        value.find(quote).map(|end| &value[..end])
    })
}

/// `s` as XML text or an attribute value between either quote.
pub(crate) fn escaped(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '<', '"', '\'']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Keyword as K;

    const HEAD: &str = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\n";

    /// What is expected at the `|` of `body`, inside the ontology of
    /// [`HEAD`], cut there.
    fn at_(body: &str) -> Option<Expected> {
        at_text(&format!("{HEAD}{body}"))
    }

    fn at_text(marked: &str) -> Option<Expected> {
        let offset = marked.find('|').expect("a `|` marks the offset");
        at(&marked.replacen('|', "", 1), offset)
    }

    /// The elements expected at the `|` of `body`.
    fn elements(body: &str) -> Vec<Element> {
        match at_(body) {
            Some(Expected::Elements(Names { elements, .. })) => elements,
            other => panic!("{body:?}: {other:?}"),
        }
    }

    fn keywords(ks: &[Keyword]) -> Vec<Element> {
        ks.iter().copied().map(Element::Keyword).collect()
    }

    fn has(body: &str, want: &[Element], not: &[Element]) {
        let got = elements(body);
        for w in want {
            assert!(got.contains(w), "{body:?}: {w:?} not in {got:?}");
        }
        for n in not {
            assert!(!got.contains(n), "{body:?}: {n:?} in {got:?}");
        }
    }

    const CLASS: Element = Element::Keyword(K::Class);
    const LITERAL: Element = Element::Leaf(Leaf::Literal);
    const IRI: Element = Element::Leaf(Leaf::Iri);
    const ANONYMOUS: Element = Element::Leaf(Leaf::AnonymousIndividual);

    /// Stories 1 and 2: the children of the ontology, in the schema's order.
    #[test]
    fn the_ontology_takes_prefixes_then_imports_annotations_and_axioms() {
        let all = elements("  <|");
        assert_eq!(&all[..3], keywords(&[K::Prefix, K::Import, K::Annotation]));
        for k in [
            K::Declaration,
            K::SubClassOf,
            K::AnnotationAssertion,
            K::DLSafeRule,
        ] {
            assert!(all.contains(&Element::Keyword(k)), "{k:?}");
        }
        for not in [K::Ontology, K::Class, K::ObjectSomeValuesFrom, K::Body] {
            assert!(!all.contains(&Element::Keyword(not)), "{not:?}");
        }
        let import = "  <Import>http://example.org/i</Import>\n  <|";
        has(
            import,
            &keywords(&[K::Import, K::Annotation, K::SubClassOf]),
            &keywords(&[K::Prefix]),
        );
        let axiom = "  <Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration>\n  <|";
        has(
            axiom,
            &keywords(&[K::SubClassOf]),
            &keywords(&[K::Prefix, K::Import, K::Annotation]),
        );
        // What is typed is the name's span; the elements are written bare.
        let text = format!("{HEAD}  <SubCl|");
        let start = text.find("SubCl").unwrap() as u32;
        let Some(Expected::Elements(Names { name, prefix, .. })) = at_text(&text) else {
            panic!()
        };
        assert_eq!((name, prefix.as_str()), (Span::new(start, start + 5), ""));
    }

    /// Stories 3 and 4: a construct's signature as filled so far.
    #[test]
    fn a_construct_takes_what_its_signature_takes_next() {
        let classes = [
            CLASS,
            Element::Keyword(K::ObjectIntersectionOf),
            Element::Keyword(K::ObjectSomeValuesFrom),
        ];
        has(
            "  <SubClassOf>\n    <|",
            &classes,
            &[
                Element::Keyword(K::ObjectProperty),
                Element::Keyword(K::SubClassOf),
                LITERAL,
            ],
        );
        // Annotations come first.
        has("  <SubClassOf>\n    <|", &keywords(&[K::Annotation]), &[]);
        has(
            "  <SubClassOf><Class IRI=\"http://example.org/o#A\"/><|",
            &classes,
            &keywords(&[K::Annotation]),
        );
        // Two class expressions: nothing more.
        let both = "  <SubClassOf><Class abbreviatedIRI=\":A\"/><ObjectUnionOf><Class abbreviatedIRI=\":B\"/><Class abbreviatedIRI=\":C\"/></ObjectUnionOf><|";
        assert_eq!(elements(both), []);
        // A property, then a class expression.
        let some = "  <SubClassOf><Class abbreviatedIRI=\":A\"/><ObjectSomeValuesFrom>";
        has(
            &format!("{some}<|"),
            &keywords(&[K::ObjectProperty, K::ObjectInverseOf]),
            &[CLASS],
        );
        has(
            &format!("{some}<ObjectProperty abbreviatedIRI=\":p\"/><|"),
            &classes,
            &keywords(&[K::ObjectProperty]),
        );
        // In a declaration, the entities' elements are constructs.
        let declared = elements("  <Declaration><|");
        for k in [
            K::Annotation,
            K::Class,
            K::Datatype,
            K::ObjectProperty,
            K::NamedIndividual,
        ] {
            assert!(declared.contains(&Element::Keyword(k)), "{k:?}");
        }
        assert!(elements("  <Declaration><Class abbreviatedIRI=\":A\"/><|").is_empty());
        // A cardinality is an attribute: a property first.
        let card = "  <SubClassOf><Class abbreviatedIRI=\":A\"/><ObjectMinCardinality cardinality=\"1\"><|";
        has(card, &keywords(&[K::ObjectProperty]), &[CLASS]);
    }

    /// The leaves their categories take, beside the keywords and entities.
    #[test]
    fn literals_anonymous_individuals_iris_and_facets() {
        let assertion = "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":note\"/>";
        // The subject: an IRI or an anonymous individual.
        has(
            &format!("{assertion}<|"),
            &[IRI, Element::Leaf(Leaf::AbbreviatedIri), ANONYMOUS],
            &[LITERAL, CLASS],
        );
        // The value: a literal too.
        let value = format!("{assertion}<IRI>http://example.org/o#A</IRI><|");
        has(&value, &[IRI, ANONYMOUS, LITERAL], &[CLASS]);
        has(
            "  <DataPropertyAssertion><DataProperty abbreviatedIRI=\":d\"/><NamedIndividual abbreviatedIRI=\":i\"/><|",
            &[LITERAL],
            &[IRI, CLASS],
        );
        has(
            "  <ClassAssertion><Class abbreviatedIRI=\":A\"/><|",
            &[ANONYMOUS, Element::Keyword(K::NamedIndividual)],
            &[LITERAL],
        );
        // A restriction's facets, each with its one literal.
        let restriction = "  <DatatypeDefinition><Datatype abbreviatedIRI=\":t\"/><DatatypeRestriction><Datatype abbreviatedIRI=\":u\"/>";
        let facet = Element::Leaf(Leaf::FacetRestriction);
        assert_eq!(elements(&format!("{restriction}<|")), [facet]);
        let one = "<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minInclusive\"><Literal>1</Literal></FacetRestriction>";
        assert_eq!(elements(&format!("{restriction}{one}<|")), [facet]);
        let open = "<FacetRestriction facet=\"http://www.w3.org/2001/XMLSchema#minInclusive\">";
        assert_eq!(elements(&format!("{restriction}{open}<|")), [LITERAL]);
        assert_eq!(
            elements(&format!("{restriction}{open}<Literal>1</Literal><|")),
            []
        );
    }

    /// The data properties of a restriction, then its data range; a key's
    /// properties, flat.
    #[test]
    fn data_ranges_and_keys() {
        let some = "  <SubClassOf><Class abbreviatedIRI=\":A\"/><DataSomeValuesFrom>";
        assert_eq!(elements(&format!("{some}<|")), keywords(&[K::DataProperty]));
        let after = format!("{some}<DataProperty abbreviatedIRI=\":d\"/><|");
        has(
            &after,
            &keywords(&[K::DataProperty, K::Datatype, K::DataOneOf]),
            &[CLASS],
        );
        let ranged = format!(
            "{some}<DataProperty abbreviatedIRI=\":d\"/><Datatype abbreviatedIRI=\":t\"/><|"
        );
        assert_eq!(elements(&ranged), []);
        let key = "  <HasKey><Class abbreviatedIRI=\":A\"/>";
        let either = keywords(&[K::ObjectInverseOf, K::ObjectProperty, K::DataProperty]);
        assert_eq!(elements(&format!("{key}<|")), either);
        let objects = format!(
            "{key}<ObjectProperty abbreviatedIRI=\":p\"/><ObjectInverseOf><ObjectProperty abbreviatedIRI=\":q\"/></ObjectInverseOf><|"
        );
        assert_eq!(elements(&objects), either);
        let data = format!(
            "{key}<ObjectProperty abbreviatedIRI=\":p\"/><DataProperty abbreviatedIRI=\":d\"/><|"
        );
        assert_eq!(elements(&data), keywords(&[K::DataProperty]));
        // Before the class expression: annotations and classes.
        has(
            "  <HasKey><|",
            &[CLASS, Element::Keyword(K::Annotation)],
            &keywords(&[K::ObjectProperty]),
        );
    }

    #[test]
    fn rules_atoms_and_arguments() {
        has(
            "  <DLSafeRule><|",
            &keywords(&[K::Annotation, K::Body]),
            &keywords(&[K::Head]),
        );
        has(
            "  <DLSafeRule><Body><|",
            &keywords(&[K::ClassAtom, K::BuiltInAtom]),
            &[CLASS],
        );
        let atom = "  <DLSafeRule><Body><ClassAtom><Class abbreviatedIRI=\":A\"/><|";
        has(
            atom,
            &keywords(&[K::Variable, K::NamedIndividual]),
            &[ANONYMOUS, CLASS],
        );
        // A built-in's IRI is its attribute: then its data arguments.
        let builtin =
            "  <DLSafeRule><Body><BuiltInAtom IRI=\"http://www.w3.org/2003/11/swrlb#equal\"><|";
        has(builtin, &[Element::Keyword(K::Variable), LITERAL], &[IRI]);
        // In an entity's element, a variable's, a literal's: nothing.
        for body in [
            "  <SubClassOf><Class IRI=\"x\"><|",
            "  <DLSafeRule><Body><ClassAtom><Class abbreviatedIRI=\":A\"/><Variable IRI=\"urn:x\"><|",
            "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><IRI>x</IRI><Literal><|",
            "  <Import><|",
            "  <Prefix name=\"a\" IRI=\"http://example.org/a#\"><|",
        ] {
            assert_eq!(elements(body), [], "{body}");
        }
    }

    /// Story 15: a document half typed, with errors, is read as far as it
    /// goes.
    #[test]
    fn a_document_with_errors_is_read_as_far_as_it_goes() {
        // Not closed above; an element of no slot; one in another namespace.
        let cases = [
            "  <SubClassOf>\n    <ObjectSomeValuesFrom>\n      <ObjectProperty abbreviatedIRI=\":p\"/>\n      <|",
            "  <SubClassOf><Clas abbreviatedIRI=\":A\"/><|",
            "  <SubClassOf><x:Class xmlns:x=\"http://example.org/x#\"/><|",
            "  <SubClassOf><Class abbreviatedIRI=\":A\"/>\n  <|\n  <SubClassOf><Class abbreviatedIRI=\":B\"/>",
        ];
        for body in cases {
            has(
                body,
                &[CLASS, Element::Keyword(K::ObjectSomeValuesFrom)],
                &keywords(&[K::SubClassOf]),
            );
        }
    }

    /// Story 5 and 7's form: an `owl:` prefix is kept.
    #[test]
    fn elements_are_written_in_the_document_s_prefix_form() {
        let text = "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\">\n  <owl:SubClassOf>\n    <owl:Cl|";
        let Some(Expected::Elements(Names {
            prefix, elements, ..
        })) = at_text(text)
        else {
            panic!()
        };
        assert_eq!(prefix, "owl:");
        assert!(elements.contains(&CLASS));
        // A bare name typed where OWL's namespace has a prefix: the prefix.
        let text = "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\">\n  <|";
        let Some(Expected::Elements(Names { prefix, .. })) = at_text(text) else {
            panic!()
        };
        assert_eq!(prefix, "owl:");
        // A root of another vocabulary: nothing.
        assert_eq!(
            at_text("<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  <|"),
            None
        );
        assert_eq!(at_text("<Ontology>\n  <|"), None);
    }

    /// The IRI of a value or an element's text, of what was typed
    /// (stories 8, 9, 11, 13, 14).
    fn iri(body: &str) -> (String, Iris, Form) {
        let text = format!("{HEAD}{body}");
        let plain = text.replacen('|', "", 1);
        match at_text(&text) {
            Some(Expected::Iri { span, iris, form }) => (
                plain[span.start as usize..span.end as usize].to_string(),
                iris,
                form,
            ),
            other => panic!("{body:?}: {other:?}"),
        }
    }

    #[test]
    fn a_value_takes_the_iris_of_its_element_s_kind() {
        let start = HEAD.find("<Ontology").unwrap() as u32;
        let end = HEAD.find(">\n  <Prefix").unwrap() as u32 + 1;
        let abbreviated = Form::Abbreviated {
            root: Span::new(start, end),
        };
        let of = |e| Iris::Of(vec![e]);
        let cases = [
            (
                "  <SubClassOf><Class IRI=\"http://example.org/o#P|er\"/>",
                "http://example.org/o#Per",
                of(Entity::Class),
                Form::Full,
            ),
            (
                "  <SubClassOf><Class abbreviatedIRI=\":|\"/>",
                ":",
                of(Entity::Class),
                abbreviated,
            ),
            (
                "  <ObjectPropertyDomain><ObjectProperty IRI=\"|",
                "",
                of(Entity::ObjectProperty),
                Form::Full,
            ),
            (
                "  <DataPropertyRange><DataProperty IRI=\"|\"",
                "",
                of(Entity::DataProperty),
                Form::Full,
            ),
            (
                "  <Declaration><Datatype abbreviatedIRI='x|'/>",
                "x",
                of(Entity::Datatype),
                abbreviated,
            ),
            (
                "  <ClassAssertion><Class abbreviatedIRI=\":A\"/><NamedIndividual IRI=\"|\"/>",
                "",
                of(Entity::NamedIndividual),
                Form::Full,
            ),
            (
                "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:la|bel\"/>",
                "rdfs:label",
                of(Entity::AnnotationProperty),
                abbreviated,
            ),
            (
                "  <DataPropertyAssertion><DataProperty abbreviatedIRI=\":d\"/><NamedIndividual abbreviatedIRI=\":i\"/><Literal datatypeIRI=\"http://www.w3.org/2001/XMLSchema#|",
                "http://www.w3.org/2001/XMLSchema#",
                of(Entity::Datatype),
                Form::Full,
            ),
            // Story 14: the text of an IRI element, without its white space.
            (
                "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><IRI> http://example.org/o#A|  </IRI>",
                "http://example.org/o#A",
                Iris::Any,
                Form::Full,
            ),
            (
                "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><AbbreviatedIRI>|",
                "",
                Iris::Any,
                abbreviated,
            ),
        ];
        for (body, typed, iris, form) in cases {
            assert_eq!(iri(body), (typed.to_string(), iris, form), "{body}");
        }
    }

    /// Story 16, and a value cut by a line break (the offset on the next
    /// line): nothing.
    #[test]
    fn nothing_where_no_iri_or_element_is_typed() {
        for body in [
            "  <!-- <SubClassOf><Cla| -->",
            "  <SubClassOf><Class abbreviatedIRI=\":A\"/><Class abbreviatedIRI=\"\n    |",
            "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><IRI>x</IRI><Literal>a pu|</Literal>",
            "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><AnonymousIndividual nodeID=\"a|\"/>",
            "  <Prefix name=\"a|\" IRI=\"http://example.org/a#\"/>",
            "  <Prefix name=\"a\" IRI=\"http://example.org/a#|\"/>",
            "  <Import>http://example.org/|</Import>",
            "  <SubClassOf>\n    |\n  </SubClassOf>",
            "  <SubClassOf><Class ab|breviatedIRI=\":A\"/>",
            "  <SubClassOf><Class xml:base=\"|\"/>",
        ] {
            assert_eq!(at_(body), None, "{body}");
        }
        for text in [
            "<?xml version=\"1|.0\"?>\n<Ontology/>",
            "<!DOCTYPE Ontology [ <!ENTITY o \"http://ex|\"> ]>\n<Ontology/>",
            "|",
        ] {
            assert_eq!(at_text(text), None, "{text}");
        }
        // Closed single-line values are replaced whole between their quotes
        // (even with spaces inside); closed values spanning lines offer
        // nothing (LSP completion edits are single-line); unclosed values cut
        // at the first white space after the offset.
        assert_eq!(
            at_("  <SubClassOf><Class IRI=\"http://example.org/o#A|\nB\"/>"),
            None
        );
        assert_eq!(
            at_(
                "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><IRI>http://example.org/o#A|\nB</IRI></AnnotationAssertion>"
            ),
            None
        );
        let (typed, ..) = iri("  <SubClassOf><Class IRI=\"http://example.org/o#A|\nB");
        assert_eq!(typed, "http://example.org/o#A");
        let (typed, ..) =
            iri("  <Declaration><Class IRI=\"| http://example.org/B \"/></Declaration>");
        assert_eq!(typed, " http://example.org/B ");
        let (typed, ..) = iri(
            "  <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\":n\"/><IRI>| http://example.org/B </IRI></AnnotationAssertion>",
        );
        assert_eq!(typed, " http://example.org/B");
        // An unclosed value whose leading white space precedes more text
        // offers nothing (cutting there would leave that text behind).
        assert_eq!(
            at_("  <Declaration><Class IRI=\"| http://example.org/B</Declaration>"),
            None
        );
        // An unclosed empty value followed only by indentation before the
        // next tag keeps the empty span at the offset.
        let (typed, ..) =
            iri("  <SubClassOf><Class IRI=\"|\n    <Class IRI=\"http://example.org/B\"/>");
        assert_eq!(typed, "");
    }

    #[test]
    fn ontology_children_respect_elements_after_the_cursor() {
        let bare = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n";
        let elems = |body: &str| match at_text(&format!("{bare}{body}\n</Ontology>\n")) {
            Some(Expected::Elements(Names { elements, .. })) => elements,
            other => panic!("{body:?}: {other:?}"),
        };
        // Before a `<Prefix>`: only `Prefix`.
        assert_eq!(
            elems("  <|\n  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>"),
            keywords(&[K::Prefix])
        );
        assert_eq!(
            elems("  <Im|\n  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>"),
            keywords(&[K::Prefix])
        );
        assert_eq!(
            elems("  <|<Prefix name=\"ex\" IRI=\"http://example.org/\"/>"),
            keywords(&[K::Prefix])
        );
        // Between two `<Prefix>` elements: only `Prefix`.
        assert_eq!(
            elems(
                "  <Prefix name=\"a\" IRI=\"http://example.org/a#\"/>\n  <|\n  <Prefix name=\"b\" IRI=\"http://example.org/b#\"/>"
            ),
            keywords(&[K::Prefix])
        );
        // Before an `<Import>`: `Prefix` and `Import` only.
        assert_eq!(
            elems(
                "  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n  <|\n  <Import>http://example.org/i</Import>"
            ),
            keywords(&[K::Prefix, K::Import])
        );
        // Before an `<Annotation>`: `Prefix`, `Import` and `Annotation` only.
        assert_eq!(
            elems(
                "  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n  <|\n  <Annotation><AnnotationProperty abbreviatedIRI=\"ex:p\"/><Literal>x</Literal></Annotation>"
            ),
            keywords(&[K::Prefix, K::Import, K::Annotation])
        );
        // Between `<Import>` and `<Annotation>`: `Import` and `Annotation`.
        assert_eq!(
            elems(
                "  <Import>http://example.org/i</Import>\n  <|\n  <Annotation><AnnotationProperty abbreviatedIRI=\"ex:p\"/><Literal>x</Literal></Annotation>"
            ),
            keywords(&[K::Import, K::Annotation])
        );
        // After all `<Prefix>` elements, before an axiom: everything fits.
        let after_prefixes = elems(
            "  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n  <|\n  <Declaration><Class IRI=\"http://example.org/A\"/></Declaration>",
        );
        for k in [K::Prefix, K::Import, K::Annotation, K::SubClassOf] {
            assert!(after_prefixes.contains(&Element::Keyword(k)), "{k:?}");
        }
        // Comments are skipped; a `<Prefix>` in a comment does not cap the phase.
        let after_comment = elems(
            "  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n  <|\n  <!-- <Prefix name=\"x\" IRI=\"http://example.org/x#\"/> -->\n  <Declaration><Class IRI=\"http://example.org/A\"/></Declaration>",
        );
        assert!(after_comment.contains(&Element::Keyword(K::Import)));
        assert!(after_comment.contains(&Element::Keyword(K::SubClassOf)));
        // An already-misplaced `<Prefix>` after an axiom does not block axioms.
        let after_axiom = elems(
            "  <Declaration><Class IRI=\"http://example.org/A\"/></Declaration>\n  <|\n  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>",
        );
        assert!(after_axiom.contains(&Element::Keyword(K::SubClassOf)));
        assert!(!after_axiom.contains(&Element::Keyword(K::Prefix)));
        // Prefixed `<owl:Ontology>` with `<owl:Prefix>`.
        let prefixed = "<owl:Ontology xmlns:owl=\"http://www.w3.org/2002/07/owl#\">\n  <owl:|\n  <owl:Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n</owl:Ontology>\n";
        let Some(Expected::Elements(Names {
            prefix, elements, ..
        })) = at_text(prefixed)
        else {
            panic!()
        };
        assert_eq!(prefix, "owl:");
        assert_eq!(elements, keywords(&[K::Prefix]));
        // Inside an existing complete tag name before `<Prefix>`: nothing.
        assert_eq!(
            at_text(&format!(
                "{bare}  <Pr|efix name=\"a\" IRI=\"http://example.org/a#\"/>\n  <Prefix name=\"b\" IRI=\"http://example.org/b#\"/>\n</Ontology>\n"
            )),
            None
        );
    }

    #[test]
    fn a_prefix_is_declared_after_the_others_or_first_in_the_ontology() {
        let parsed = |text: &str| super::super::parse(text, None);
        let check = |text: &str, want: &str| {
            let p = parsed(text);
            let ontology = text.find("<Ontology").unwrap();
            let end = ontology + text[ontology..].find('>').unwrap() + 1;
            let root = Span::new(ontology as u32, end as u32);
            let (at, new) = declare(
                text,
                &p,
                root,
                "skos",
                "http://www.w3.org/2004/02/skos/core#",
            );
            let after = format!("{}{new}{}", &text[..at as usize], &text[at as usize..]);
            assert_eq!(after, want);
            let re = parsed(&after);
            assert!(re.prefixes.iter().any(|d| d.name == "skos"), "{after}");
            assert_eq!(re.errors.len(), p.errors.len(), "{after}");
        };
        let skos = "<Prefix name=\"skos\" IRI=\"http://www.w3.org/2004/02/skos/core#\"/>";
        let body =
            "  <Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration>\n</Ontology>\n";
        check(&format!("{HEAD}{body}"), &format!("{HEAD}  {skos}\n{body}"));
        let bare = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\n    <Declaration><Class IRI=\"http://example.org/o#A\"/></Declaration>\n</Ontology>\n";
        check(bare, &bare.replacen(">\n", &format!(">\n    {skos}\n"), 1));
        // A misplaced declaration, after an axiom, is not one.
        let late = format!(
            "{HEAD}{}  <Prefix name=\"x\" IRI=\"http://example.org/x#\"/>\n</Ontology>\n",
            &body[..body.len() - 12]
        );
        let p = parsed(&late);
        let root = Span::new(0, 0);
        let (at, _) = declare(
            &late,
            &p,
            root,
            "skos",
            "http://www.w3.org/2004/02/skos/core#",
        );
        assert_eq!(at as usize, HEAD.len() - 1);
        // CRLF, and what XML escapes.
        let crlf = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\">\r\n  <Prefix name=\"\" IRI=\"http://example.org/o#\"/>\r\n</Ontology>";
        let (_, new) = declare(crlf, &parsed(crlf), root, "a", "http://example.org/a?b&c#");
        assert_eq!(
            new,
            "\r\n  <Prefix name=\"a\" IRI=\"http://example.org/a?b&amp;c#\"/>"
        );
    }

    #[test]
    fn the_snippet_forms_follow_the_reader() {
        assert_eq!(form(CLASS), (&["IRI"][..], true));
        assert_eq!(
            form(Element::Keyword(K::ObjectMinCardinality)),
            (&["cardinality"][..], false)
        );
        assert_eq!(
            form(Element::Keyword(K::DataExactCardinality)),
            (&["cardinality"][..], false)
        );
        assert_eq!(form(Element::Keyword(K::SubClassOf)), (&[][..], false));
        assert_eq!(
            form(Element::Keyword(K::Prefix)),
            (&["name", "IRI"][..], true)
        );
        assert_eq!(form(ANONYMOUS), (&["nodeID"][..], true));
        for leaf in Leaf::ALL {
            assert_eq!(element(leaf.name()), Some(Element::Leaf(leaf)));
        }
    }
}
