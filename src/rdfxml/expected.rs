//! What may come at an offset of an RDF/XML text, for completion: a node
//! element's name, a property element's name, the value of `rdf:about`,
//! `rdf:resource` or `rdf:datatype`, or nothing; and where a new `xmlns:p`
//! declaration goes.
//!
//! The elements open at the offset ([`xml::cursor`]) are walked from the
//! root with the striping of RDF/XML 1.1 (section 2.2), as [`super::grammar`]
//! reads it: the children of `rdf:RDF` are node elements (a root that is
//! not `rdf:RDF` is one itself); a node element's children are property
//! elements; a property element holds one node element, or, with
//! `rdf:parseType="Resource"`, property elements (of a blank node), with
//! `"Collection"` node elements, with `"Triple"` one node element, and with
//! `"Literal"` (or a value the grammar reads as it) XML that is no RDF/XML.
//! A property element that has its object already (a node element closed in
//! it, non-white text, `rdf:resource`, `rdf:nodeID`, `rdf:datatype`, or a
//! property attribute) takes no element. A root named in no namespace is no
//! RDF/XML: nothing comes in it.
//!
//! An element name is not offered where replacing it would break the
//! document: in the start tag of an element whose end tag names it too.
//! The end tags a new element would meet up to the one that closes it, as
//! the scanner closes elements, are [`Expected::taken`]: completion offers
//! none of their names. This reads the text after the offset, to the end
//! tag that closes the element being named. A value is offered only while
//! what it replaces may be an IRI: it holds no white space (a value not
//! closed runs over the line end; one closed by a later attribute's quote
//! holds that attribute's name).

use super::xml::cursor::{At, Document, Element};
use super::xml::{self, Tag};
use crate::syntax::{RDF, Span, has_scheme};
use std::borrow::Cow;
use std::collections::BTreeMap;

/// What may come at an offset, and what completion needs around it.
pub(crate) struct Expected<'d> {
    pub(crate) slot: Slot<'d>,
    /// The element name or attribute value typed there, whole: what an
    /// offer replaces.
    pub(crate) span: Span,
    /// The namespaces in scope, by prefix (`""` for the default namespace).
    pub(crate) prefixes: BTreeMap<&'d str, Cow<'d, str>>,
    /// The names of the end tags that a new element named at the offset
    /// would meet before the one that closes it, and that one ([`end_tags`]):
    /// a new element of one of these names would take that end tag.
    pub(crate) taken: Vec<&'d str>,
    /// Whether `rdf:Description` may be offered in node-element position:
    /// false when replacing a self-closing typed node inside
    /// `rdf:parseType="Triple"` (removing its `rdf:type` triple leaves zero).
    pub(crate) allow_description: bool,
    /// The root element's start tag, where a declaration goes: `None` when
    /// it is broken (not closed by `>`, its last value not closed) or read
    /// from an entity, where none can.
    root: Option<Tag<'d>>,
}

/// What may come at an offset.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Slot<'d> {
    /// The name of a node element: `rdf:Description` or a class.
    NodeElement,
    /// The name of a property element of `Node`.
    PropertyElement(Node),
    /// The name of the innermost open element after `</`.
    EndTag { name: &'d str, closed: bool },
    /// Valid syntax attributes for the start tag being typed.
    Attribute(Vec<AttrKind>),
    /// An attribute value.
    Value {
        of: Value,
        /// The quote around it.
        quote: u8,
        /// The entity of the DOCTYPE whose reference the value starts with:
        /// its name and its replacement text.
        entity: Option<(&'d str, &'d str)>,
    },
}

/// An RDF/XML syntax attribute offered on a start tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttrKind {
    About,
    Id,
    NodeId,
    Type,
    Resource,
    Datatype,
    ParseType,
    XmlLang,
}

/// Which attribute value.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Value {
    /// `rdf:about` of a node element: a subject.
    About,
    /// `rdf:type` attribute of a node element: a class.
    Type,
    /// `rdf:resource` of a property element of `node`: the object of
    /// `property` (`None` when the element's name is no IRI).
    Resource {
        property: Option<String>,
        node: Node,
    },
    /// `rdf:datatype` of a property element.
    Datatype,
    /// `rdf:parseType` of a property element; `allow_triple` is false when
    /// the tag is self-closing or already carries another valid `parseType`
    /// on an empty closed element (switching to `Triple` with no child would
    /// report zero triples).
    ParseType { allow_triple: bool },
}

/// A node element: the subject of the property elements in it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Node {
    /// Its `rdf:about`, when it is an absolute IRI.
    pub(crate) about: Option<String>,
    /// The classes it names: its element name (but `rdf:Description`) and
    /// its `rdf:type` attribute. None for the blank node of
    /// `rdf:parseType="Resource"`.
    pub(crate) types: Vec<String>,
}

/// What the children of an open element are.
enum Content {
    /// Node elements.
    Nodes,
    /// Node element of an `rdf:parseType="Triple"` property element, which
    /// holds exactly one triple.
    TripleNode,
    /// Property elements of a node.
    Properties(Node),
    /// No RDF/XML element: an XML literal, or a property element that has
    /// its object.
    None,
}

const ITS: &str = "http://www.w3.org/2005/11/its";

/// What may come at byte `offset` of `doc`, which read `text`; `None` where
/// nothing does.
pub(crate) fn at<'d>(text: &'d str, doc: &'d Document<'_>, offset: usize) -> Option<Expected<'d>> {
    let c = doc.at(offset);
    if let At::EndName { span, closed } = c.at {
        let root_name = &c.open.first()?.tag.name;
        if !root_name.is(RDF, "RDF") && root_name.ns().is_none() {
            return None;
        }
        let last = c.open.last()?;
        if last.tag.name.entity || !xml::is_name(last.tag.name.raw) {
            return None;
        }
        // Do not overwrite an end tag that already closes an outer open element,
        // or close `last` early when a matching closing tag already follows.
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
        if super::xml::cursor::is_already_closed(text, span.end as usize, &outer, last.tag.name.raw)
        {
            return None;
        }
        return Some(Expected {
            slot: Slot::EndTag {
                name: last.tag.name.raw,
                closed,
            },
            span,
            prefixes: c.prefixes,
            taken: Vec::new(),
            allow_description: true,
            root: None,
        });
    }
    let content = match content(&c.open) {
        Some(content) => content,
        None if c.open.is_empty() => match &c.at {
            // A standalone document may start with a node element instead of
            // `rdf:RDF` (RDF/XML 1.1, 7.2.1): its start tag is in node position.
            At::Value(v) if v.tag.name.ns().is_some() && !v.tag.name.is(RDF, "RDF") => {
                Content::Nodes
            }
            At::AttrName(a) if a.tag.name.ns().is_some() && !a.tag.name.is(RDF, "RDF") => {
                Content::Nodes
            }
            _ => return None,
        },
        None => return None,
    };
    // The end tags a new element at the offset would meet, up to the one
    // that closes it.
    let mut taken = Vec::new();
    let mut self_root = None;
    let mut allow_description = true;
    let (slot, span) = match c.at {
        At::Name(span) => {
            let typed = text.get(span.start as usize..span.end as usize)?;
            let mut open: Vec<&str> = c.open.iter().map(|e| e.tag.name.raw).collect();
            open.push(typed);
            taken = end_tags(text, span.end as usize, &open);
            // Not in the start tag of an element whose end tag names it too.
            if taken.last() == Some(&typed) {
                return None;
            }
            // Inside `rdf:parseType="Triple"`, changing an existing
            // `rdf:Description` to a typed class adds an `rdf:type` triple,
            // and changing a self-closing typed node to `rdf:Description`
            // removes its `rdf:type` triple.
            if matches!(content, Content::TripleNode) {
                if is_rdf_description(typed, &c.prefixes) {
                    return None;
                }
                if !typed.is_empty() && scan_tag(text, span.end as usize).1 {
                    allow_description = false;
                }
            }
            (
                match content {
                    Content::Nodes | Content::TripleNode => Slot::NodeElement,
                    Content::Properties(node) => Slot::PropertyElement(node),
                    Content::None => return None,
                },
                span,
            )
        }
        At::Value(v) => {
            // What the offer replaces: the value whole. One with no closing
            // quote, which the grammar reads to where the scanner ends it (a
            // `<`, the end of the text), up to there or to the white space
            // after the offset, which it holds already. A value closed by
            // the quote of a later attribute holds its name, white space
            // first.
            let rest =
                text.get(v.span.end as usize..v.attr.value.span.end.max(v.span.end) as usize)?;
            let run = rest
                .find(|c: char| c.is_ascii_whitespace())
                .unwrap_or(rest.len());
            let span = Span::new(v.span.start, v.span.end + run as u32);
            let value = text.get(span.start as usize..span.end as usize)?;
            if value.contains(|c: char| c.is_ascii_whitespace()) {
                return None;
            }
            let name = &v.attr.name;
            let of = match content {
                Content::Nodes | Content::TripleNode if name.is(RDF, "about") => Value::About,
                Content::Nodes | Content::TripleNode if name.is(RDF, "type") => Value::Type,
                Content::Properties(node) if name.is(RDF, "resource") => Value::Resource {
                    property: iri(&v.tag.name),
                    node,
                },
                Content::Properties(_) if name.is(RDF, "datatype") => Value::Datatype,
                Content::Properties(_) if name.is(RDF, "parseType") => parse_type_offer(text, &v)?,
                _ => return None,
            };
            let quote = *text.as_bytes().get(v.span.start.checked_sub(1)? as usize)?;
            // The reference the value starts with, typed whole.
            let entity = v
                .attr
                .value
                .lead
                .filter(|(lead, _)| lead.start == v.span.start && lead.end as usize <= offset)
                .and_then(|(lead, _)| {
                    let name = text.get(lead.start as usize + 1..lead.end as usize - 1)?;
                    doc.entity(name).map(|ns| (name, ns))
                });
            if c.open.is_empty() && !v.attr.name.entity && v.attr.span.end > v.attr.value.span.end {
                self_root = Some(v.tag);
            }
            (Slot::Value { of, quote, entity }, span)
        }
        At::AttrName(a) => {
            let attrs = tag_attrs(&content, &a.tag);
            if attrs.is_empty() {
                return None;
            }
            (Slot::Attribute(attrs), a.span)
        }
        At::EndName { .. } | At::Text(_) | At::Elsewhere => return None,
    };
    let root = c
        .open
        .into_iter()
        .next()
        .map(|e| e.tag)
        .or(self_root)
        .filter(|root| {
            let closed = text
                .get(root.span.start as usize..root.span.end as usize)
                .is_some_and(|t| t.ends_with('>'))
                && root
                    .attrs
                    .last()
                    .is_none_or(|a| !a.name.entity && a.span.end > a.value.span.end);
            closed && !root.name.entity
        });
    Some(Expected {
        slot,
        span,
        prefixes: c.prefixes,
        taken,
        allow_description,
        root,
    })
}

/// What the children of the innermost of `open` are; `None` when nothing is
/// open, or the root is no RDF/XML.
fn content(open: &[Element<'_>]) -> Option<Content> {
    let (root, inner) = open.split_first()?;
    let name = &root.tag.name;
    let mut content = if name.is(RDF, "RDF") {
        Content::Nodes
    } else if name.ns().is_some() {
        Content::Properties(node(&root.tag))
    } else {
        return None;
    };
    for e in inner {
        content = match content {
            Content::Nodes | Content::TripleNode => Content::Properties(node(&e.tag)),
            Content::Properties(_) => property_content(e),
            Content::None => Content::None,
        };
    }
    Some(content)
}

/// What a property element holds, by its `rdf:parseType`.
fn property_content(e: &Element<'_>) -> Content {
    let parse_type = e
        .tag
        .attrs
        .iter()
        .find(|a| a.name.is(RDF, "parseType"))
        .map(|a| &*a.value.text);
    match parse_type {
        None => {
            let has_object =
                !e.children.is_empty() || e.has_text || e.tag.attrs.iter().any(fixes_object);
            if has_object {
                Content::None
            } else {
                Content::Nodes
            }
        }
        Some("Resource") => Content::Properties(Node::default()),
        Some("Collection") => Content::Nodes,
        Some("Triple") => Content::TripleNode,
        // "Literal", and any other value, which reads as it.
        Some(_) => Content::None,
    }
}

/// Whether `rdf:parseType` on `v.tag` may be offered: not when the property
/// element already has non-white body content (changing `parseType` changes
/// how its children read and can break them).
fn parse_type_offer(text: &str, v: &super::xml::cursor::AttrValue<'_>) -> Option<Value> {
    let raw_tag = text.get(v.tag.span.start as usize..v.tag.span.end as usize)?;
    let self_closing = raw_tag.ends_with("/>");
    let tag_closed = raw_tag.ends_with('>');
    if !self_closing {
        let after = text
            .get(v.tag.span.end as usize..)?
            .trim_start_matches(|c: char| c.is_ascii_whitespace());
        if !after.is_empty() && !after.starts_with("</") {
            return None;
        }
    }
    let already_valid_other = matches!(&*v.attr.value.text, "Resource" | "Collection" | "Literal");
    let allow_triple = !self_closing && !(tag_closed && already_valid_other);
    Some(Value::ParseType { allow_triple })
}

/// The RDF/XML syntax attributes valid on `tag` in `content` that are not
/// already present or mutually exclusive with attributes on `tag`.
fn tag_attrs(content: &Content, tag: &Tag<'_>) -> Vec<AttrKind> {
    let has_rdf = |local: &str| tag.attrs.iter().any(|a| a.name.is(RDF, local));
    let has_xml_lang = tag
        .attrs
        .iter()
        .any(|a| a.name.ns() == Some(xml::XML_NS) && a.name.local() == "lang");
    let mut out = Vec::new();
    match content {
        Content::Nodes | Content::TripleNode => {
            if !has_rdf("about") && !has_rdf("ID") && !has_rdf("nodeID") {
                out.extend([AttrKind::About, AttrKind::Id, AttrKind::NodeId]);
            }
            if !has_rdf("type") {
                out.push(AttrKind::Type);
            }
            if !has_xml_lang {
                out.push(AttrKind::XmlLang);
            }
        }
        Content::Properties(_) => {
            let has_object = has_rdf("resource")
                || has_rdf("nodeID")
                || has_rdf("datatype")
                || has_rdf("parseType")
                || tag.attrs.iter().any(fixes_object);
            if !has_object {
                out.extend([
                    AttrKind::Resource,
                    AttrKind::Datatype,
                    AttrKind::ParseType,
                    AttrKind::NodeId,
                ]);
            }
            if !has_rdf("ID") {
                out.push(AttrKind::Id);
            }
            if !has_xml_lang {
                out.push(AttrKind::XmlLang);
            }
        }
        Content::None => {}
    }
    out
}

/// Whether raw element name `typed` names `rdf:Description` under `prefixes`.
fn is_rdf_description(typed: &str, prefixes: &BTreeMap<&str, Cow<'_, str>>) -> bool {
    match typed.split_once(':') {
        Some((p, l)) => l == "Description" && prefixes.get(p).map(Cow::as_ref) == Some(RDF),
        None => typed == "Description" && prefixes.get("").map(Cow::as_ref) == Some(RDF),
    }
}

/// Whether `attr` on a property element with no `rdf:parseType` fixes its
/// object, as [`super::grammar`] reads it: `rdf:resource`, `rdf:nodeID`,
/// `rdf:datatype`, `rdf:type`, or any property attribute.
fn fixes_object(attr: &xml::Attr<'_>) -> bool {
    let n = &attr.name;
    if n.ns() == Some(xml::XML_NS)
        || n.raw.starts_with("xml")
        || n.is(ITS, "dir")
        || n.is(ITS, "version")
    {
        return false;
    }
    if n.ns() == Some(RDF) {
        return !matches!(
            n.local(),
            "ID" | "about"
                | "parseType"
                | "version"
                | "annotation"
                | "annotationNodeID"
                | "aboutEach"
                | "aboutEachPrefix"
                | "bagID"
                | "li"
                | "RDF"
        );
    }
    true
}

/// The node a node element's start tag describes.
fn node(tag: &Tag<'_>) -> Node {
    let about = tag
        .attrs
        .iter()
        .find(|a| a.name.is(RDF, "about"))
        .map(|a| a.value.text.to_string())
        .filter(|v| has_scheme(v));
    let named = iri(&tag.name).filter(|_| !tag.name.is(RDF, "Description"));
    let typed = tag
        .attrs
        .iter()
        .filter(|a| a.name.is(RDF, "type"))
        .map(|a| a.value.text.to_string())
        .filter(|v| has_scheme(v));
    Node {
        about,
        types: named.into_iter().chain(typed).collect(),
    }
}

/// The IRI a name stands for; `None` in no namespace.
fn iri(name: &xml::Name<'_>) -> Option<String> {
    name.ns().map(|ns| format!("{ns}{}", name.local()))
}

/// Scan past the attributes of a start tag from byte `from` (after its name)
/// as [`super::xml`] does: returns `(next_pos, empty)` where `empty` is true
/// only when the tag ends with `/>` in attribute position (not inside a
/// quoted attribute value or an unquoted attribute word).
fn scan_tag(text: &str, from: usize) -> (usize, bool) {
    let b = text.as_bytes();
    let mut i = from;
    let skip_space = |i: &mut usize| {
        while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\r' | b'\n') {
            *i += 1;
        }
    };
    let ends_name = |c: u8| {
        matches!(
            c,
            b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/' | b'<' | b'=' | b'"' | b'\'' | b'?'
        )
    };
    loop {
        skip_space(&mut i);
        match b.get(i) {
            None | Some(b'<') => return (i, false),
            Some(b'>') => return (i + 1, false),
            Some(b'/') if b.get(i + 1) == Some(&b'>') => return (i + 2, true),
            _ => {}
        }
        let name_len = b[i..]
            .iter()
            .position(|&c| ends_name(c))
            .unwrap_or(b.len() - i);
        if name_len == 0 {
            i += text[i..].chars().next().map_or(1, char::len_utf8);
            continue;
        }
        i += name_len;
        skip_space(&mut i);
        if b.get(i) != Some(&b'=') {
            continue;
        }
        i += 1;
        skip_space(&mut i);
        let Some(&q) = b.get(i).filter(|c| matches!(c, b'"' | b'\'')) else {
            // Unquoted attribute word: `xml.rs` skips up to space, `<` or `>`,
            // including a `/` before `>`, so `/>` after it reads as `>`.
            while i < b.len() && !matches!(b[i], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'<') {
                i += 1;
            }
            continue;
        };
        i += 1;
        while i < b.len() && b[i] != q && b[i] != b'<' {
            i += 1;
        }
        if b.get(i) == Some(&q) {
            i += 1;
        }
    }
}

/// The names of the end tags after the start tag whose name ends at byte
/// `from` that no element opened after it takes, up to the first that
/// names one of `open` (the elements open there, outermost first, that one
/// last): the one that closes it, as the scanner closes elements. None when
/// the start tag ends with `/>`. The start tag may end with `>` (which an
/// editor may have written with the `<`), or be cut by a `<` (which also
/// ends a value) or the end of the text. Comments, CDATA sections and
/// processing instructions are passed over.
fn end_tags<'t>(text: &'t str, from: usize, open: &[&str]) -> Vec<&'t str> {
    let (mut i, empty) = scan_tag(text, from);
    if empty {
        return Vec::new();
    }
    let name = |s: &'t str| &s[..s.find(|c| !xml::is_name_char(c)).unwrap_or(s.len())];
    // The elements opened after it and still open, as the scanner closes
    // them: an end tag that names one further out closes those inside it,
    // one that names none is passed over.
    let mut inner: Vec<&str> = Vec::new();
    let mut ends = Vec::new();
    while let Some(lt) = text[i..].find('<') {
        let at = i + lt;
        let rest = &text[at..];
        if let Some(end) = rest.strip_prefix("</") {
            let end = name(end);
            match inner.iter().rposition(|n| *n == end) {
                Some(k) => inner.truncate(k),
                None => {
                    ends.push(end);
                    if open.contains(&end) {
                        return ends;
                    }
                }
            }
            i = at + 2;
        } else if let Some((open, close)) = [("<!--", "-->"), ("<![CDATA[", "]]>"), ("<?", "?>")]
            .into_iter()
            .find(|(open, _)| rest.starts_with(open))
        {
            let body = &rest[open.len()..];
            i = at + open.len() + body.find(close).map_or(body.len(), |e| e + close.len());
        } else if rest[1..].starts_with(xml::is_name_start_char) {
            let n = name(&rest[1..]);
            let (next, empty) = scan_tag(text, at + 1 + n.len());
            if !empty {
                inner.push(n);
            }
            i = next;
        } else {
            i = at + 1;
        }
    }
    ends
}

impl Expected<'_> {
    /// Whether a declaration of `prefix` may be added to the root: its
    /// start tag can take one ([`Expected::root`]), and declares no such
    /// prefix (one that an element inside redeclares, or undeclares, is not
    /// in scope, and a second declaration on the root would be an error).
    pub(crate) fn may_declare(&self, prefix: &str) -> bool {
        !prefix.is_empty()
            && !self.prefixes.contains_key(prefix)
            && self.root.as_ref().is_some_and(|root| {
                !root
                    .attrs
                    .iter()
                    .any(|a| a.name.prefix() == Some("xmlns") && a.name.local() == prefix)
            })
    }

    /// The insertion that declares `prefix` as `ns` on the root's start
    /// tag: after its last attribute, on a line of its own indented like it
    /// when the root's attributes are one per line, else after a space.
    /// `None` where [`Expected::may_declare`] says no.
    pub(crate) fn declare(&self, text: &str, prefix: &str, ns: &str) -> Option<(u32, String)> {
        if !self.may_declare(prefix) {
            return None;
        }
        let root = self.root.as_ref()?;
        let at = root.attrs.last().map_or(root.name.span.end, |a| a.span.end);
        let attr = format!("xmlns:{prefix}=\"{}\"", super::rename::escaped(ns, b'"'));
        let attrs = &root.attrs;
        let lined = attrs.len() >= 2
            && attrs.windows(2).all(|w| {
                text.get(w[0].span.end as usize..w[1].span.start as usize)
                    .is_some_and(|gap| gap.contains('\n'))
            });
        let last = attrs.last().map(|a| a.span.start as usize);
        let indent = last.and_then(|start| {
            let line = text[..start].rfind('\n')? + 1;
            let indent = &text[line..start];
            let crlf = text[..line].ends_with("\r\n");
            Some((indent, if crlf { "\r\n" } else { "\n" }))
        });
        Some(match indent {
            Some((indent, nl)) if lined && xml::is_whitespace(indent) => {
                (at, format!("{nl}{indent}{attr}"))
            }
            _ => (at, format!(" {attr}")),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EX: &str = "http://example.org/";
    const OWL: &str = "http://www.w3.org/2002/07/owl#";
    const RDFS: &str = "http://www.w3.org/2000/01/rdf-schema#";
    const HEAD: &str = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n         xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n         xmlns:owl=\"http://www.w3.org/2002/07/owl#\"\n         xmlns:ex=\"http://example.org/\">\n";

    /// The slot at the `|` of `marked`, and the text it replaces.
    fn slot(marked: &str) -> Option<(String, String)> {
        let offset = marked.find('|').expect("a `|` marks the offset");
        let text = marked.replacen('|', "", 1);
        let doc = Document::read(&text);
        let e = at(&text, &doc, offset)?;
        let typed = text[e.span.start as usize..e.span.end as usize].to_string();
        Some((format!("{:?}", e.slot), typed))
    }

    fn node(about: Option<&str>, types: &[&str]) -> Node {
        Node {
            about: about.map(str::to_string),
            types: types.iter().map(|t| t.to_string()).collect(),
        }
    }

    fn element(marked: &str) -> Option<String> {
        slot(marked).map(|s| s.0)
    }

    #[test]
    fn the_children_of_rdf_rdf_are_node_elements() {
        for marked in [
            format!("{HEAD}  <|"),
            format!("{HEAD}  <owl:Cl|"),
            format!("{HEAD}  <owl:Cl|\n</rdf:RDF>\n"),
            format!("{HEAD}  <owl:Cl|/>\n</rdf:RDF>\n"),
            format!("{HEAD}  <owl:Class rdf:about=\"{EX}A\"/>\n  <|\n</rdf:RDF>\n"),
        ] {
            assert_eq!(slot(&marked).unwrap().0, "NodeElement", "{marked}");
        }
        let (_, typed) = slot(&format!("{HEAD}  <owl:Cl|ass/>\n</rdf:RDF>\n")).unwrap();
        assert_eq!(typed, "owl:Class");
    }

    #[test]
    fn a_node_elements_children_are_its_properties() {
        let got = element(&format!(
            "{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <rdfs:la|"
        ));
        let want = Slot::PropertyElement(node(Some(&format!("{EX}A")), &[&format!("{OWL}Class")]));
        assert_eq!(got.unwrap(), format!("{want:?}"));
        // rdf:Description names no class; rdf:type as an attribute does.
        let got = element(&format!(
            "{HEAD}  <rdf:Description rdf:about=\"{EX}a\" rdf:type=\"{EX}T\">\n    <|"
        ));
        let want = Slot::PropertyElement(node(Some(&format!("{EX}a")), &[&format!("{EX}T")]));
        assert_eq!(got.unwrap(), format!("{want:?}"));
        // A relative rdf:about is not resolved.
        let got = element(&format!("{HEAD}  <ex:T rdf:about=\"#a\">\n    <|"));
        let want = Slot::PropertyElement(node(None, &[&format!("{EX}T")]));
        assert_eq!(got.unwrap(), format!("{want:?}"));
    }

    /// The striping at any depth, with each `rdf:parseType`.
    #[test]
    fn the_striping_is_followed_at_any_depth() {
        let open = format!(
            "{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <rdfs:subClassOf>\n      <owl:Restriction>\n        <owl:onProperty>\n"
        );
        assert_eq!(element(&format!("{open}<|")).unwrap(), "NodeElement");
        let deeper = format!("{open}<owl:ObjectProperty rdf:about=\"{EX}p\">\n<|");
        let want = Slot::PropertyElement(node(
            Some(&format!("{EX}p")),
            &[&format!("{OWL}ObjectProperty")],
        ));
        assert_eq!(element(&deeper).unwrap(), format!("{want:?}"));
        let resource = format!(
            "{open}<owl:ObjectProperty rdf:about=\"{EX}p\">\n<ex:q rdf:parseType=\"Resource\">\n<|"
        );
        assert_eq!(
            element(&resource).unwrap(),
            format!("{:?}", Slot::PropertyElement(Node::default()))
        );
        for collection in ["Collection", "Triple"] {
            let marked = format!(
                "{open}<owl:Thing rdf:about=\"{EX}x\">\n<ex:q rdf:parseType=\"{collection}\">\n<ex:A rdf:about=\"{EX}b\"/>\n<|"
            );
            assert_eq!(element(&marked).unwrap(), "NodeElement", "{collection}");
        }
        // A root that is a node element.
        let rooted = format!("<ex:A xmlns:ex=\"{EX}\" xmlns:rdf=\"{RDF}\">\n  <|");
        let want = Slot::PropertyElement(node(None, &[&format!("{EX}A")]));
        assert_eq!(element(&rooted).unwrap(), format!("{want:?}"));
    }

    #[test]
    fn nothing_in_an_xml_literal_or_after_the_object() {
        let open = format!("{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    ");
        for marked in [
            format!("{open}<rdfs:comment rdf:parseType=\"Literal\"><|"),
            format!("{open}<rdfs:comment rdf:parseType=\"Literal\"><b><owl:Cl|"),
            format!("{open}<rdfs:comment rdf:parseType=\"Literal\"><b>\n<ex:q rdf:resource=\"|"),
            format!("{open}<rdfs:comment rdf:parseType=\"Other\"><|"),
            format!("{open}<rdfs:subClassOf rdf:resource=\"{EX}B\"><|"),
            format!("{open}<rdfs:subClassOf><owl:Class/><|"),
            format!("{open}<rdfs:label rdf:datatype=\"{EX}d\"><|"),
        ] {
            assert_eq!(slot(&marked), None, "{marked}");
        }
    }

    #[test]
    fn nothing_in_text_comments_attribute_names_or_a_root_in_no_namespace() {
        for marked in [
            format!("{HEAD}  <owl:Class>\n    <rdfs:label>a|</rdfs:label>"),
            format!("{HEAD}  <!-- <owl:Cl| -->"),
            format!("{HEAD}  <owl:Class rdf:ab|out=\"x\"/>"),
            "<?xml version=\"1.0\"?>\n<rdf:R|".to_string(),
            format!("{HEAD}  <owl:Class rdf:nodeID=\"|"),
            format!("{HEAD}  <owl:Class>\n    <rdfs:label xml:lang=\"|"),
            // rdf:about on a property element, rdf:resource on a node element.
            format!("{HEAD}  <owl:Class>\n    <rdfs:label rdf:about=\"|"),
            format!("{HEAD}  <owl:Class rdf:resource=\"|"),
            // Turtle read as RDF/XML.
            "<http://example.org/a> <h|ttp://example.org/p> <http://example.org/b> .\n".to_string(),
        ] {
            assert_eq!(slot(&marked), None, "{marked}");
        }
    }

    /// Renaming the start tag of an element with an end tag would leave
    /// the end tag naming the old one.
    #[test]
    fn no_name_in_the_start_tag_of_an_element_with_an_end_tag() {
        for marked in [
            format!("{HEAD}  <owl:Cl|ass rdf:about=\"{EX}A\">\n  </owl:Class>\n</rdf:RDF>\n"),
            format!("{HEAD}  <owl:Class| rdf:about=\"a>b\">\n  </owl:Class>\n"),
            format!("{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <rdfs:la|bel>x</rdfs:label>\n"),
            // Cut by a `<`, in a value too, and closed further on.
            format!(
                "{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <|rdfs:label xml:lang=\"en>A</rdfs:label>\n"
            ),
            format!(
                "{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <rdfs:label|\n    </rdfs:label>\n"
            ),
        ] {
            assert_eq!(slot(&marked), None, "{marked}");
        }
        for marked in [
            format!("{HEAD}  <owl:Cl|ass rdf:about=\"{EX}A\"/>\n</rdf:RDF>\n"),
            format!("{HEAD}  <owl:Cl| rdf:about=\"a/>b\"\n</rdf:RDF>\n"),
            // Closed by the `>` an editor writes with the `<`.
            format!("{HEAD}  <|>\n</rdf:RDF>\n"),
            format!("{HEAD}  <owl:Cl|>\n</rdf:RDF>\n"),
            // Another end tag that starts with the name.
            format!(
                "{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <rdfs:la|\n    <rdfs:label>x</rdfs:label>\n"
            ),
        ] {
            assert!(slot(&marked).is_some(), "{marked}");
        }
    }

    /// The end tags a new element would meet: those that name no element
    /// open are passed over by the scanner, and one of their names would
    /// take them too.
    #[test]
    fn the_end_tags_a_new_element_meets() {
        let taken = |marked: &str| {
            let offset = marked.find('|').unwrap();
            let text = marked.replacen('|', "", 1);
            let doc = Document::read(&text);
            let e = at(&text, &doc, offset).unwrap();
            e.taken.iter().map(|t| t.to_string()).collect::<Vec<_>>()
        };
        let open = format!("{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    <rdfs:subClassOf>\n");
        assert_eq!(
            taken(&format!(
                "{open}      <owl:Cl|\n      <owl:Thing><rdfs:label>x</rdfs:label></owl:Thing>\n    </rdfs:subClassOf>\n  </owl:Class>\n"
            )),
            ["rdfs:subClassOf"]
        );
        assert_eq!(
            taken(&format!(
                "{open}      <owl:Cl|>\n    </rdfs:label>\n  </owl:Class>\n"
            )),
            ["rdfs:label", "owl:Class"]
        );
        assert_eq!(
            taken(&format!("{open}      <owl:Cl|/>\n    </rdfs:subClassOf>\n")),
            Vec::<String>::new()
        );
    }

    #[test]
    fn values_of_about_resource_and_datatype() {
        let open = format!("{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    ");
        let (got, typed) =
            slot(&format!("{HEAD}  <owl:Class rdf:about=\"{EX}Wid|get\"/>")).unwrap();
        let want = Slot::Value {
            of: Value::About,
            quote: b'"',
            entity: None,
        };
        assert_eq!((got, typed), (format!("{want:?}"), format!("{EX}Widget")));
        let (got, typed) = slot(&format!("{open}<rdfs:subClassOf rdf:resource='{EX}B|")).unwrap();
        let want = Slot::Value {
            of: Value::Resource {
                property: Some(format!("{RDFS}subClassOf")),
                node: node(Some(&format!("{EX}A")), &[&format!("{OWL}Class")]),
            },
            quote: b'\'',
            entity: None,
        };
        assert_eq!((got, typed), (format!("{want:?}"), format!("{EX}B")));
        let (got, _) = slot(&format!("{open}<ex:n rdf:datatype=\"|\">1</ex:n>")).unwrap();
        let want = Slot::Value {
            of: Value::Datatype,
            quote: b'"',
            entity: None,
        };
        assert_eq!(got, format!("{want:?}"));
    }

    /// A value not closed runs over the line end: past it, nothing.
    #[test]
    fn a_value_not_closed_ends_at_the_line() {
        let open = format!("{HEAD}  <owl:Class rdf:about=\"{EX}A\">\n    ");
        let (_, typed) = slot(&format!(
            "{open}<rdfs:subClassOf rdf:resource=\"{EX}B|\n    <rdfs:label>x</rdfs:label>"
        ))
        .unwrap();
        assert_eq!(typed, format!("{EX}B"));
        assert_eq!(
            slot(&format!(
                "{open}<rdfs:subClassOf rdf:resource=\"{EX}B\n  |  <rdfs:label>x</rdfs:label>"
            )),
            None
        );
        // Closed by the quote of the next attribute: it would delete its name.
        assert_eq!(
            slot(&format!(
                "{open}<ex:n rdf:datatype=\"| xml:lang=\"en\">1</ex:n>"
            )),
            None
        );
    }

    #[test]
    fn a_value_that_starts_with_an_entity() {
        let marked = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n  <!ENTITY xsd \"http://www.w3.org/2001/XMLSchema#\">\n]>\n{}  <owl:DatatypeProperty rdf:about=\"{EX}p\">\n    <rdfs:range rdf:resource=\"&xsd;str|\"/>",
            HEAD.split_once('\n').unwrap().1
        );
        let (got, typed) = slot(&marked).unwrap();
        assert!(
            got.contains("entity: Some((\"xsd\", \"http://www.w3.org/2001/XMLSchema#\"))"),
            "{got}"
        );
        assert_eq!(typed, "&xsd;str");
        // Inside the reference: not typed whole yet.
        let (got, _) = slot(&marked.replace("&xsd;str|", "&xs|d;str")).unwrap();
        assert!(got.contains("entity: None"), "{got}");
    }

    #[test]
    fn a_declaration_goes_after_the_roots_last_attribute() {
        let check = |head: &str, want: &str| {
            let marked = format!("{head}  <|\n</rdf:RDF>\n");
            let offset = marked.find('|').unwrap();
            let text = marked.replacen('|', "", 1);
            let doc = Document::read(&text);
            let e = at(&text, &doc, offset).unwrap();
            assert!(e.may_declare("skos"));
            let (at, new) = e
                .declare(&text, "skos", "http://www.w3.org/2004/02/skos/core#")
                .unwrap();
            let after = format!("{}{new}{}", &text[..at as usize], &text[at as usize..]);
            assert!(after.starts_with(want), "{after}");
        };
        let skos = "xmlns:skos=\"http://www.w3.org/2004/02/skos/core#\"";
        // One per line: a line of its own, indented like them.
        check(
            HEAD,
            &HEAD.replace(
                "example.org/\">",
                &format!("example.org/\"\n         {skos}>"),
            ),
        );
        let crlf = HEAD.replace('\n', "\r\n");
        check(
            &crlf,
            &crlf.replace(
                "example.org/\">",
                &format!("example.org/\"\r\n         {skos}>"),
            ),
        );
        // On one line, or alone: after a space.
        let one = format!("<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"{EX}\">\n");
        check(&one, &one.replace("\">", &format!("\" {skos}>")));
        let alone = format!("<rdf:RDF\n    xmlns:rdf=\"{RDF}\">\n");
        check(&alone, &alone.replace("\">", &format!("\" {skos}>")));
    }

    /// A root tag broken off takes no declaration: it would land in its
    /// value, or past its end.
    #[test]
    fn a_broken_root_takes_no_declaration() {
        for head in [
            format!("<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"http://www.w3.org/1999/02/22\n"),
            format!("<rdf:RDF xmlns:rdf=\"{RDF}\"\n"),
        ] {
            let text = format!("{head}  <rdf:Description>\n    <");
            let doc = Document::read(&text);
            let e = at(&text, &doc, text.len()).unwrap();
            assert!(!e.may_declare("skos"), "{text}");
            assert_eq!(e.declare(&text, "skos", "http://example.org/s#"), None);
        }
    }

    #[test]
    fn a_prefix_the_root_declares_is_not_declared_again() {
        let text = format!(
            "<rdf:RDF xmlns:rdf=\"{RDF}\" xmlns:ex=\"{EX}\">\n  <rdf:Description xmlns:ex=\"\">\n    <\n"
        );
        let doc = Document::read(&text);
        let e = at(&text, &doc, text.len() - 1).unwrap();
        // Undeclared inside, declared on the root.
        assert!(!e.prefixes.contains_key("ex"));
        assert!(!e.may_declare("ex"));
        assert!(!e.may_declare("rdf"));
        assert!(e.may_declare("skos"));
        assert!(!e.may_declare(""));
    }

    #[test]
    fn a_root_node_element_takes_rdf_about_values() {
        let want = format!(
            "{:?}",
            Slot::Value {
                of: Value::About,
                quote: b'"',
                entity: None,
            }
        );
        for (marked, expected_typed) in [
            (
                format!(
                    "<?xml version=\"1.0\"?>\n<owl:Class xmlns:rdf=\"{RDF}\" xmlns:owl=\"{OWL}\" rdf:about=\"{EX}Pe|\"/>\n"
                ),
                format!("{EX}Pe"),
            ),
            (
                format!("<rdf:Description xmlns:rdf=\"{RDF}\" rdf:about=\"{EX}A|B\"/>\n"),
                format!("{EX}AB"),
            ),
            (
                format!("<owl:Class xmlns:rdf=\"{RDF}\" xmlns:owl=\"{OWL}\" rdf:about=\"{EX}Pe|"),
                format!("{EX}Pe"),
            ),
        ] {
            let (got, typed) = slot(&marked).expect(&marked);
            assert_eq!((got, typed), (want.clone(), expected_typed), "{marked}");
        }
        // Declarations on a closed root node element never overlap the value edit.
        let marked =
            format!("<owl:Class xmlns:rdf=\"{RDF}\" xmlns:owl=\"{OWL}\" rdf:about=\"{EX}Pe|\"/>\n");
        let offset = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let doc = Document::read(&text);
        let e = at(&text, &doc, offset).unwrap();
        assert!(e.may_declare("skos"));
        let (ins, _) = e
            .declare(&text, "skos", "http://www.w3.org/2004/02/skos/core#")
            .unwrap();
        assert!(ins < e.span.start || ins > e.span.end);
        // Unclosed value ending in `>` cannot take a declaration.
        let unclosed =
            format!("<owl:Class xmlns:rdf=\"{RDF}\" xmlns:owl=\"{OWL}\" rdf:about=\"{EX}Pe>|");
        let offset = unclosed.find('|').unwrap();
        let text = unclosed.replacen('|', "", 1);
        let doc = Document::read(&text);
        let e = at(&text, &doc, offset).unwrap();
        assert!(!e.may_declare("skos"));
        // Root `rdf:RDF`, a root in no namespace, or `rdf:resource` on a root node: nothing.
        for marked in [
            format!("<rdf:RDF xmlns:rdf=\"{RDF}\" rdf:about=\"{EX}A|\"/>"),
            format!("<Class xmlns:rdf=\"{RDF}\" rdf:about=\"{EX}A|\"/>"),
            format!("<owl:Class xmlns:rdf=\"{RDF}\" xmlns:owl=\"{OWL}\" rdf:resource=\"{EX}A|\"/>"),
        ] {
            assert_eq!(slot(&marked), None, "{marked}");
        }
    }

    #[test]
    fn a_property_with_text_or_property_attributes_takes_no_node_element() {
        let open = format!("{HEAD}  <rdf:Description rdf:about=\"{EX}s\">\n    ");
        for marked in [
            format!("{open}<ex:q>text<|</ex:q>"),
            format!("{open}<ex:q><![CDATA[text]]><|</ex:q>"),
            format!("{open}<ex:q ex:label=\"abc\"><|</ex:q>"),
            format!("{open}<ex:q rdf:type=\"{EX}T\"><|</ex:q>"),
            format!("{open}<ex:q rdf:value=\"x\"><|</ex:q>"),
        ] {
            assert_eq!(slot(&marked), None, "{marked}");
        }
        // Whitespace-only content, comments, and syntax attributes that do not
        // fix the object still allow a node element.
        for marked in [
            format!("{open}<ex:q>  \n  <|</ex:q>"),
            format!("{open}<ex:q><!-- note --><|</ex:q>"),
            format!("{open}<ex:q rdf:ID=\"r1\"><|</ex:q>"),
            format!("{open}<ex:q xml:lang=\"en\"><|</ex:q>"),
            format!("{open}<ex:q rdf:annotation=\"{EX}a\"><|</ex:q>"),
        ] {
            assert_eq!(element(&marked).as_deref(), Some("NodeElement"), "{marked}");
        }
    }

    #[test]
    fn end_tags_matches_scanner_on_unquoted_attrs_and_quoted_gt() {
        // Quoted `>` in a nested self-closing tag (A1): outer tag is still matched.
        assert_eq!(
            slot(&format!(
                "{HEAD}  <|ex:Person><ex:Person ex:q=\">\"/></ex:Person>\n</rdf:RDF>\n"
            )),
            None
        );
        // Unquoted attribute word runs over `/>` in xml.rs, leaving the tag open
        // to match the following `</rdf:Description>` (chaos 2065).
        assert_eq!(
            slot(&format!(
                "{HEAD}  <|rdf:Description rdf:about=rce=\"{EX}b\"/></rdf:Description>\n</rdf:RDF>\n"
            )),
            None
        );
        // Non-name-char attribute name (`!`) with quoted `>` still reads as self-closing.
        assert_eq!(
            slot(&format!(
                "{HEAD}  <|ex:Person><ex:Person !=\"x>\"/></ex:Person>\n</rdf:RDF>\n"
            )),
            None
        );
    }

    #[test]
    fn existing_rdf_description_in_parse_type_triple_is_not_retyped() {
        let open = format!(
            "{HEAD}  <rdf:Description rdf:about=\"{EX}s\">\n    <ex:says rdf:parseType=\"Triple\">\n      "
        );
        assert_eq!(
            slot(&format!(
                "{open}<|rdf:Description rdf:about=\"{EX}a\"><ex:p rdf:resource=\"{EX}b\"/></rdf:Descrip"
            )),
            None
        );
        // Typing a fresh or partial name still offers node elements.
        assert_eq!(
            element(&format!("{open}<rdf:Des|")).as_deref(),
            Some("NodeElement")
        );
        // Replacing a self-closing typed node in `parseType="Triple"` forbids `rdf:Description`.
        let marked = format!("{open}<|ex:Person rdf:about=\"{EX}a\"/></ex:says>");
        let offset = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let doc = Document::read(&text);
        let e = at(&text, &doc, offset).unwrap();
        assert!(!e.allow_description);
    }
}
