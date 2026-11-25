//! What is open and typed at an offset of an XML text: the shared seam of
//! completion in OWL/XML and RDF/XML.
//!
//! [`Document::at`] reads the text with the [`Scanner`] up to the
//! construct the offset is in (a tag, a text run, a comment…), reads that
//! construct whole, and stops: the work is the text before the offset and
//! that one construct. It answers the elements open there, each with its
//! children closed before the offset ([`Element`]), where the
//! offset is ([`At`]), and the namespaces in scope. The offset is checked
//! here, once: one past the end or inside a UTF-8 character is
//! [`At::Elsewhere`], with nothing open.
//!
//! A construct the offset ends is the one being typed when the end of the
//! text, or a `<`, cut it (`<owl:Cl`, `rdf:about="http://ex`); one its own
//! terminator closes (`>`, `-->`, `?>`, `]]>`) is behind the offset, which
//! is then in the content that follows.
//!
//! A bare `<` before the root element is read by [`Prolog::read`] as text
//! before the root, so it is [`At::Elsewhere`]; `<O` there is the root's
//! name being typed.

use super::{Attr, Event, Name, Prolog, Scanner, Tag};
use crate::syntax::Span;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// An XML text and its prolog, read once; the cursors at its offsets
/// borrow both (a tag read from an entity's replacement text borrows the
/// prolog).
pub(crate) struct Document<'t> {
    text: &'t str,
    prolog: Prolog,
}

/// What is open and typed at an offset.
#[derive(Debug)]
pub(crate) struct Cursor<'a> {
    /// The elements open at the offset, outermost first. The start tag the
    /// offset is in is not one of them.
    pub(crate) open: Vec<Element<'a>>,
    pub(crate) at: At<'a>,
    /// The namespaces in scope at the offset, by prefix (`""` for the
    /// default namespace): those the open elements declare, and the start
    /// tag the offset is in, as the scanner applies them (one Namespaces in
    /// XML reserves is not; an empty one undeclares).
    pub(crate) prefixes: BTreeMap<&'a str, Cow<'a, str>>,
}

/// An open element.
#[derive(Debug)]
pub(crate) struct Element<'a> {
    pub(crate) tag: Tag<'a>,
    /// The names of its child elements closed before the offset, in order
    /// (read from an entity's replacement text, with the reference's span).
    pub(crate) children: Vec<Name<'a>>,
}

/// Where the offset is.
#[derive(Debug)]
pub(crate) enum At<'a> {
    /// In an element name after `<`: the name's span, to its end; empty
    /// after a `<` that no name follows yet.
    Name(Span),
    /// In an attribute value of the start tag being written.
    Value(Box<AttrValue<'a>>),
    /// In the text content of the innermost open element: the text node
    /// (to the next `<`), empty between two tags.
    Text(Span),
    /// In the prolog, a comment, a processing instruction, a CDATA section,
    /// an end tag, a tag's attribute names, an entity reference, outside
    /// the root element, or at no offset of the text.
    Elsewhere,
}

/// An attribute value being written: [`At::Value`].
#[derive(Debug)]
pub(crate) struct AttrValue<'a> {
    /// The start tag, without `attr`.
    pub(crate) tag: Tag<'a>,
    /// The attribute, its value as the scanner reads it: an unclosed value
    /// runs to the next `<`, white space included.
    pub(crate) attr: Attr<'a>,
    /// What was typed, and where the value's edit goes: from after the
    /// quote to the closing quote, or to the offset when there is none.
    pub(crate) span: Span,
}

impl<'t> Document<'t> {
    pub(crate) fn read(text: &'t str) -> Document<'t> {
        Document {
            text,
            prolog: Prolog::read(text),
        }
    }

    /// The replacement text of the DOCTYPE's internal entity `name`
    /// (character references expanded, entity references kept); `None`
    /// when it is not declared or the scanner does not expand it.
    pub(crate) fn entity(&self, name: &str) -> Option<&str> {
        self.prolog
            .entities
            .get(name)
            .filter(|e| !e.refused)
            .map(|e| &*e.text)
    }

    /// What is open and typed at byte `offset`.
    pub(crate) fn at(&self, offset: usize) -> Cursor<'_> {
        let text = self.text;
        let mut cursor = Cursor {
            open: Vec::new(),
            at: At::Elsewhere,
            prefixes: BTreeMap::new(),
        };
        if !text.is_char_boundary(offset) {
            return cursor;
        }
        let mut s = Scanner::new(text, &self.prolog);
        // The last construct of the document read: where it starts, its
        // events (applied once the next one starts before the offset), and
        // whether it expanded an entity's replacement text.
        let mut last = None;
        let mut held: Vec<Event<'_>> = Vec::new();
        let mut entered = false;
        loop {
            tick();
            if let Some(e) = s.pending.pop_front() {
                held.push(e);
                continue;
            }
            if s.pos >= s.b.len() {
                match s.frames.pop() {
                    Some(frame) => {
                        s.leave(frame);
                        continue;
                    }
                    None => break,
                }
            }
            if s.frames.is_empty() {
                if s.pos >= offset {
                    break;
                }
                for e in held.drain(..) {
                    apply(&mut cursor.open, e);
                }
                last = Some(s.pos);
                entered = false;
            }
            if let Some(e) = s.step() {
                held.push(e);
            }
            entered |= !s.frames.is_empty();
        }
        // Inside an element deeper than the scanner reads, or in the prolog.
        let Some(start) = last.filter(|_| s.skip == 0) else {
            return cursor;
        };
        let end = s.pos;
        let raw = &text.as_bytes()[start..end];
        let o = offset as u32;
        let after = offset == end && (entered || closed(raw));
        let mut events = held.into_iter();
        // The start tag the offset is in, whose declarations apply there.
        let mut typed = None;
        cursor.at = match events.next() {
            Some(Event::Start(mut tag)) if !entered => {
                tick();
                if let Some(i) = tag.attrs.iter().position(|a| a.value.span.contains(o)) {
                    let attr = tag.attrs.remove(i);
                    let closed = attr.span.end > attr.value.span.end;
                    let span = Span::new(
                        attr.value.span.start,
                        if closed { attr.value.span.end } else { o },
                    );
                    At::Value(Box::new(AttrValue { tag, attr, span }))
                } else if tag.name.span.contains(o) {
                    let span = tag.name.span;
                    typed = Some(tag);
                    At::Name(span)
                } else if after {
                    for e in std::iter::once(Event::Start(tag)).chain(events) {
                        apply(&mut cursor.open, e);
                    }
                    content(&cursor.open, text, offset)
                } else {
                    At::Elsewhere
                }
            }
            // A text run; right after a `<` that starts no tag, an element
            // name not typed yet.
            Some(Event::Text { span, .. }) if !entered && span.start as usize == start => {
                if raw[0] == b'<' && offset == start + 1 {
                    At::Name(Span::new(o, o))
                } else {
                    At::Text(span)
                }
            }
            first if after => {
                for e in first.into_iter().chain(events) {
                    apply(&mut cursor.open, e);
                }
                content(&cursor.open, text, offset)
            }
            _ => At::Elsewhere,
        };
        let typed = match &cursor.at {
            At::Value(v) => Some(&v.tag),
            _ => typed.as_ref(),
        };
        cursor.prefixes = prefixes(&s, cursor.open.iter().map(|e| &e.tag).chain(typed));
        cursor
    }
}

/// One event read before the offset: an element opens, or closes as a
/// child of the one it is in.
fn apply<'a>(open: &mut Vec<Element<'a>>, e: Event<'a>) {
    tick();
    match e {
        Event::Start(tag) => open.push(Element {
            tag,
            children: Vec::new(),
        }),
        Event::End(_) => {
            if let Some(closed) = open.pop()
                && let Some(parent) = open.last_mut()
            {
                parent.children.push(closed.tag.name);
            }
        }
        Event::Text { .. } | Event::Pi(..) | Event::Eof => {}
    }
}

/// At `offset`, between constructs: the text content of the innermost
/// open element, to the next `<`.
fn content<'a>(open: &[Element<'_>], text: &str, offset: usize) -> At<'a> {
    if open.is_empty() {
        return At::Elsewhere;
    }
    let end = text.as_bytes()[offset..]
        .iter()
        .position(|&c| c == b'<')
        .map_or(text.len(), |p| offset + p);
    At::Text(Span::new(offset as u32, end as u32))
}

/// The construct `raw` ends with its own terminator, not where the text or
/// a `<` cut it: a comment, CDATA section, processing instruction or tag.
fn closed(raw: &[u8]) -> bool {
    let (open, close): (&[u8], &[u8]) = if raw.starts_with(b"<!--") {
        (b"<!--", b"-->")
    } else if raw.starts_with(b"<![CDATA[") {
        (b"<![CDATA[", b"]]>")
    } else if raw.starts_with(b"<?") {
        (b"<?", b"?>")
    } else if raw.starts_with(b"<") {
        (b"<", b">")
    } else {
        return false;
    };
    raw.len() >= open.len() + close.len() && raw.ends_with(close)
}

/// The namespaces `tags` declare, outermost first, as the scanner applies
/// them.
fn prefixes<'a, 'b>(
    s: &Scanner<'a>,
    tags: impl Iterator<Item = &'b Tag<'a>>,
) -> BTreeMap<&'a str, Cow<'a, str>>
where
    'a: 'b,
{
    let mut out = BTreeMap::new();
    for a in tags.flat_map(|t| &t.attrs) {
        tick();
        let prefix = match a.name.raw.strip_prefix("xmlns") {
            Some("") => "",
            Some(rest) => match rest.strip_prefix(':') {
                Some(p) => p,
                None => continue,
            },
            None => continue,
        };
        if s.reserved(prefix, a.name.span, &a.value).is_some() {
            continue;
        }
        if a.value.text.is_empty() {
            out.remove(prefix);
        } else {
            out.insert(prefix, a.value.text.clone());
        }
    }
    out
}

#[cfg(test)]
thread_local! {
    /// The steps [`Document::at`] takes on this thread: see [`tick`].
    static STEPS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// One step of the cursor's work, counted in the tests: each turn of the
/// scanning loop (an event, or a construct read), each event applied, each
/// attribute read for a namespace, each tag searched for the offset.
fn tick() {
    #[cfg(test)]
    STEPS.with(|c| c.set(c.get() + 1));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rdfxml::xml::Ns;

    const RDF: &str = "http://www.w3.org/1999/02/22-rdf-syntax-ns#";
    const OWL: &str = "http://www.w3.org/2002/07/owl#";
    const XSD: &str = "http://www.w3.org/2001/XMLSchema#";
    const RDF_HEAD: &str = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\" xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:ex=\"http://example.org/\">\n";
    const OWX_HEAD: &str = "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n  <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n";

    /// The cursor at the `|` of `marked`, with the text without it.
    fn check(marked: &str, f: impl FnOnce(&str, usize, Cursor<'_>)) {
        let offset = marked.find('|').expect("a `|` marks the offset");
        let text = marked.replacen('|', "", 1);
        let doc = Document::read(&text);
        f(&text, offset, doc.at(offset));
    }

    fn slice(text: &str, span: Span) -> &str {
        &text[span.start as usize..span.end as usize]
    }

    fn open<'a>(c: &Cursor<'a>) -> Vec<&'a str> {
        c.open.iter().map(|e| e.tag.name.raw).collect()
    }

    fn children<'a>(e: &Element<'a>) -> Vec<&'a str> {
        e.children.iter().map(|c| c.raw).collect()
    }

    fn elsewhere(marked: &str) {
        check(marked, |_, _, c| {
            assert!(matches!(c.at, At::Elsewhere), "{marked:?}: {:?}", c.at);
        });
    }

    fn text_at(marked: &str, want: &str) {
        check(marked, |text, o, c| match c.at {
            At::Text(span) => {
                assert!(span.contains(o as u32), "{marked:?}");
                assert_eq!(slice(text, span), want, "{marked:?}");
            }
            other => panic!("{marked:?}: {other:?}"),
        });
    }

    #[test]
    fn a_bare_lt_starts_an_element_name() {
        for tail in ["", "\n</rdf:RDF>\n", "</rdf:RDF>"] {
            check(&format!("{RDF_HEAD}  <|{tail}"), |_, o, c| {
                assert!(matches!(c.at, At::Name(s) if s == Span::new(o as u32, o as u32)));
                assert_eq!(open(&c), ["rdf:RDF"]);
            });
        }
    }

    #[test]
    fn an_element_name_cut_by_the_offset_is_typed_whole() {
        for (marked, name) in [
            (format!("{RDF_HEAD}  <owl:Cl|"), "owl:Cl"),
            (format!("{RDF_HEAD}  <owl:Cl|\n</rdf:RDF>"), "owl:Cl"),
            (
                format!("{RDF_HEAD}  <owl:Cl|ass rdf:about=\"x\"/>"),
                "owl:Class",
            ),
            (format!("{RDF_HEAD}  <|owl:Class/>"), "owl:Class"),
        ] {
            check(&marked, |text, _, c| {
                let At::Name(span) = c.at else {
                    panic!("{marked:?}: {:?}", c.at)
                };
                assert_eq!(slice(text, span), name);
                assert_eq!(open(&c), ["rdf:RDF"]);
                assert_eq!(c.prefixes["owl"], OWL);
            });
        }
    }

    /// A value whose closing quote is missing stops at the next `<`, as
    /// the scanner reads it: what was typed ends at the offset.
    #[test]
    fn a_value_cut_by_the_next_line_ends_at_the_offset() {
        let marked = format!(
            "{OWX_HEAD}  <SubClassOf>\n    <Class abbreviatedIRI=\"skos:Con|\n    <Class IRI=\"http://example.org/B\"/>\n  </SubClassOf>\n</Ontology>\n"
        );
        check(&marked, |text, o, c| {
            let At::Value(v) = &c.at else {
                panic!("{:?}", c.at)
            };
            assert_eq!(v.tag.name.raw, "Class");
            assert!(v.tag.attrs.is_empty());
            assert_eq!(v.attr.name.raw, "abbreviatedIRI");
            assert_eq!(v.attr.name.ns, Ns::Unbound);
            assert_eq!(slice(text, v.span), "skos:Con");
            assert_eq!(v.span.end as usize, o);
            // The scanner's reading runs to the `<`, white space included.
            assert_eq!(v.attr.value.text, "skos:Con     ");
            assert_eq!(open(&c), ["Ontology", "SubClassOf"]);
            assert_eq!(children(&c.open[0]), ["Prefix"]);
            assert!(c.open[1].children.is_empty());
        });
    }

    #[test]
    fn a_value_at_the_end_of_the_text() {
        check(
            &format!("{RDF_HEAD}  <owl:Class rdf:about=\"|"),
            |text, o, c| {
                let At::Value(v) = &c.at else {
                    panic!("{:?}", c.at)
                };
                assert_eq!(o, text.len());
                assert!(v.tag.name.is(OWL, "Class"));
                assert!(v.attr.name.is(RDF, "about"));
                assert_eq!(v.span, Span::new(o as u32, o as u32));
                assert_eq!(open(&c), ["rdf:RDF"]);
            },
        );
        // The quote of a value cut there ends nothing.
        check(&format!("{RDF_HEAD}<r a=\"x>|"), |text, _, c| {
            let At::Value(v) = &c.at else {
                panic!("{:?}", c.at)
            };
            assert_eq!(slice(text, v.span), "x>");
        });
    }

    #[test]
    fn a_value_with_a_doctype_entity() {
        let marked = format!(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n  <!ENTITY xsd \"{XSD}\">\n]>\n{}  <owl:DatatypeProperty rdf:about=\"http://example.org/p\">\n    <rdfs:range rdf:resource=\"&xsd;|",
            RDF_HEAD.split_once('\n').unwrap().1
        );
        let offset = marked.find('|').unwrap();
        let text = marked.replacen('|', "", 1);
        let doc = Document::read(&text);
        assert_eq!(doc.entity("xsd"), Some(XSD));
        assert_eq!(doc.entity("rdf"), None);
        let c = doc.at(offset);
        let At::Value(v) = &c.at else {
            panic!("{:?}", c.at)
        };
        assert!(v.attr.name.is(RDF, "resource"));
        assert_eq!(slice(&text, v.span), "&xsd;");
        assert_eq!(v.attr.value.text, XSD);
        let (lead, len) = v.attr.value.lead.unwrap();
        assert_eq!(slice(&text, lead), "&xsd;");
        assert_eq!(len as usize, XSD.len());
        assert_eq!(open(&c), ["rdf:RDF", "owl:DatatypeProperty"]);
    }

    #[test]
    fn a_value_in_a_well_formed_document_runs_to_its_quote() {
        let marked = format!(
            "{RDF_HEAD}  <owl:Class rdf:about=\"http://example.org/A|nimal\" ex:n='1'/>\n</rdf:RDF>\n"
        );
        check(&marked, |text, _, c| {
            let At::Value(v) = &c.at else {
                panic!("{:?}", c.at)
            };
            assert_eq!(slice(text, v.span), "http://example.org/Animal");
            assert_eq!(v.attr.value.text, "http://example.org/Animal");
            assert!(v.tag.empty);
            // The other attributes, after the offset too.
            assert_eq!(v.tag.attrs.len(), 1);
            assert_eq!(v.tag.attrs[0].name.raw, "ex:n");
            assert_eq!(open(&c), ["rdf:RDF"]);
            assert!(c.open[0].children.is_empty());
        });
    }

    #[test]
    fn comments_instructions_and_cdata_are_elsewhere() {
        for inner in [
            "<!-- a <owl:Cl| -->",
            "<!-- a=\"| -->",
            "<?p a=\"|\"?>",
            "<![CDATA[<a|]]>",
        ] {
            elsewhere(&format!("{RDF_HEAD}{inner}\n</rdf:RDF>"));
        }
        // Cut by the end of the text, though they end with their
        // terminator's bytes.
        for cut in ["<!--->|", "<?>|", "<!-- >|", "<![CDATA[]]|", "<!--|"] {
            elsewhere(&format!("{RDF_HEAD}{cut}"));
        }
        // Closed: the offset is in the content after them.
        for closed in ["<!---->|", "<?p?>|", "<![CDATA[x]]>|", "<!-- a -->|"] {
            text_at(&format!("{RDF_HEAD}{closed}"), "");
        }
    }

    #[test]
    fn the_prolog_is_elsewhere() {
        for marked in [
            "|<r/>",
            "<?xml ver|sion=\"1.0\"?>\n<r/>",
            "<?xml version=\"1.0\"?>|\n<r/>",
            "<!DOCTYPE r [ <!ENTITY e \"htt|p://example.org/\"> ]>\n<r/>",
            "<?xml version=\"1.0\"?>\n|<r/>",
        ] {
            elsewhere(marked);
        }
    }

    #[test]
    fn attribute_names_and_end_tags_are_elsewhere() {
        for marked in [
            format!("{RDF_HEAD}  <owl:Class rdf:ab|out=\"x\"/>\n</rdf:RDF>"),
            format!("{RDF_HEAD}  <owl:Class rdf:about=\"x\" |/>\n</rdf:RDF>"),
            format!("{RDF_HEAD}  <owl:Class rdf:about=\"x\"|/>\n</rdf:RDF>"),
            format!("{RDF_HEAD}  <owl:Class |"),
            format!("{RDF_HEAD}  <owl:Class rdf:about=|"),
            format!("{RDF_HEAD}  <owl:Class>\n  </owl:Cl|ass>\n</rdf:RDF>"),
            format!("{RDF_HEAD}  <owl:Class>\n  </owl:Class|"),
        ] {
            elsewhere(&marked);
        }
        // After an end tag: the parent's content, the element closed in it.
        check(
            &format!("{RDF_HEAD}  <owl:Class>\n  </owl:Class>|\n</rdf:RDF>"),
            |text, _, c| {
                let At::Text(span) = c.at else {
                    panic!("{:?}", c.at)
                };
                assert_eq!(slice(text, span), "\n");
                assert_eq!(open(&c), ["rdf:RDF"]);
                assert_eq!(children(&c.open[0]), ["owl:Class"]);
                assert!(c.open[0].children[0].is(OWL, "Class"));
            },
        );
    }

    #[test]
    fn an_inner_element_redeclares_a_prefix() {
        let doc = "<r xmlns:p=\"http://example.org/a#\"><s xmlns:p=\"http://example.org/b#\"><p:x/>|</s>|<t q=\"|\" xmlns:p=\"http://example.org/c#\"/></r>";
        let marks: Vec<usize> = doc.match_indices('|').map(|(i, _)| i).collect();
        let text = doc.replace('|', "");
        let doc = Document::read(&text);
        let inside = doc.at(marks[0]);
        assert_eq!(inside.prefixes["p"], "http://example.org/b#");
        assert_eq!(open(&inside), ["r", "s"]);
        assert!(inside.open[1].children[0].is("http://example.org/b#", "x"));
        let after = doc.at(marks[1] - 1);
        assert_eq!(after.prefixes["p"], "http://example.org/a#");
        assert_eq!(children(&after.open[0]), ["s"]);
        // The tag the offset is in declares for itself.
        let typed = doc.at(marks[2] - 2);
        assert!(matches!(typed.at, At::Value(_)));
        assert_eq!(typed.prefixes["p"], "http://example.org/c#");
    }

    #[test]
    fn the_default_namespace_is_the_empty_prefix() {
        check(&format!("{OWX_HEAD}  <Declaration>|"), |_, _, c| {
            assert_eq!(c.prefixes[""], OWL);
            assert_eq!(c.prefixes.len(), 1);
            assert!(c.open[0].tag.name.is(OWL, "Ontology"));
            assert!(c.open[1].tag.name.is(OWL, "Declaration"));
        });
        // An empty one undeclares it; a reserved one is not applied.
        check(
            "<a xmlns=\"http://example.org/\" xmlns:xml=\"http://example.org/x\"><b xmlns=\"\">|</b></a>",
            |_, _, c| {
                assert!(c.prefixes.is_empty(), "{:?}", c.prefixes);
                assert_eq!(c.open[1].tag.name.ns, Ns::Unbound);
            },
        );
    }

    #[test]
    fn the_text_of_an_iri_element() {
        let head = format!(
            "{OWX_HEAD}  <AnnotationAssertion>\n    <AnnotationProperty abbreviatedIRI=\"rdfs:label\"/>\n    <IRI>"
        );
        for (marked, want) in [
            (
                format!("{head}http://example.org/A|</IRI>"),
                "http://example.org/A",
            ),
            (
                format!("{head}http://exa|mple.org/A</IRI>"),
                "http://example.org/A",
            ),
            (format!("{head}|</IRI>"), ""),
            (
                format!("{head}|http://example.org/A</IRI>"),
                "http://example.org/A",
            ),
            (format!("{head}|"), ""),
            (
                format!("{head}http://example.org/A|"),
                "http://example.org/A",
            ),
        ] {
            text_at(&marked, want);
            check(&marked, |_, _, c| {
                assert_eq!(open(&c), ["Ontology", "AnnotationAssertion", "IRI"]);
                assert_eq!(children(&c.open[1]), ["AnnotationProperty"]);
            });
        }
    }

    /// Each open element has the names of its children closed before the
    /// offset.
    #[test]
    fn closed_children() {
        let marked = format!(
            "{OWX_HEAD}  <Declaration><Class IRI=\"http://example.org/A\"/></Declaration>\n  <SubClassOf><Class IRI=\"http://example.org/A\"/>|"
        );
        check(&marked, |_, _, c| {
            assert_eq!(open(&c), ["Ontology", "SubClassOf"]);
            assert_eq!(children(&c.open[0]), ["Prefix", "Declaration"]);
            assert_eq!(children(&c.open[1]), ["Class"]);
        });
        check("<r|", |_, _, c| assert!(c.open.is_empty()));
    }

    /// A bare `<` before the root is text before it, as the prolog reads
    /// it; a name typed there is the root's.
    #[test]
    fn before_the_root() {
        elsewhere("<?xml version=\"1.0\"?>\n<|");
        check("<?xml version=\"1.0\"?>\n<O|", |text, _, c| {
            let At::Name(span) = c.at else {
                panic!("{:?}", c.at)
            };
            assert_eq!(slice(text, span), "O");
            assert!(c.open.is_empty());
        });
        // After the root.
        elsewhere("<r/>\n|");
        elsewhere("<r/>\nab|c");
    }

    #[test]
    fn an_offset_off_the_text_is_elsewhere() {
        let text = format!("{RDF_HEAD}  <owl:Class rdf:about=\"http://example.org/é\"/>");
        let doc = Document::read(&text);
        let inside = text.find('é').unwrap() + 1;
        for offset in [inside, text.len() + 1, usize::MAX] {
            let c = doc.at(offset);
            assert!(matches!(c.at, At::Elsewhere));
            assert!(c.open.is_empty() && c.prefixes.is_empty());
        }
    }

    /// In an entity reference read as markup, nothing is typed; after it,
    /// the elements it held are closed children.
    #[test]
    fn entity_references_in_content() {
        let head = "<!DOCTYPE r [ <!ENTITY c \"<Class IRI='http://example.org/A'/>\"> ]>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\"><SubClassOf>";
        elsewhere(&format!("{head}&c|;</SubClassOf></Ontology>"));
        elsewhere(&format!("{head}&|c;</SubClassOf></Ontology>"));
        check(
            &format!("{head}&c;|</SubClassOf></Ontology>"),
            |text, o, c| {
                assert!(matches!(c.at, At::Text(s) if s == Span::new(o as u32, o as u32)));
                let child = &c.open[1].children[0];
                assert!(child.is(OWL, "Class") && child.entity);
                assert_eq!(slice(text, child.span), "&c;");
            },
        );
    }

    const WELL_FORMED: &str = "<?xml version=\"1.0\"?>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns=\"http://example.org/\">\n  <!-- classes -->\n  <owl:Class rdf:about=\"http://example.org/A\">\n    <label xml:lang=\"en\">A &amp; B</label>\n    <![CDATA[x < y]]>\n  </owl:Class>\n  <?pi data?>\n  <rdf:Description rdf:about=\"http://example.org/b\" xmlns:p=\"http://example.org/p#\"><p:q rdf:resource=\"http://example.org/c\"/></rdf:Description>\n</rdf:RDF>\n";
    const BROKEN: &str = "<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\"\n  <Prefix name=\"ex\" IRI=\"http://example.org/\n  <SubClassOf><Class abbreviatedIRI=ex:A/><Class\n  </Declaration>< x <!-- open\n  <IRI>http://example.org/</IR\n  <?pi\n  ]]> &nope; &#xZZ; <a b='c' b='d' xmlns:q=\"\">\n";
    const WITH_DOCTYPE: &str = "\u{FEFF}<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE rdf:RDF [\n  <!ENTITY ex \"http://example.org/é#\">\n  <!ENTITY m \"<owl:Class rdf:about='&ex;日本'/>\">\n  <!ENTITY loop \"&loop;\">\n]>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\" xmlns:owl=\"http://www.w3.org/2002/07/owl#\" xmlns:ex=\"&ex;\">\n  <owl:Class rdf:about=\"&ex;Ünïcode🦀\">&m;<ex:p>日本語 &ex; 🦀</ex:p>&loop;</owl:Class>\n  <ex:q rdf:resource=\"&ex;é\n</rdf:RDF>";

    /// At every byte of three texts, and one past: no panic, every span on
    /// the text's character boundaries, the offset in the span of what is
    /// typed, and the innermost element's name resolved by the prefixes
    /// in scope.
    #[test]
    fn every_offset_of_three_texts() {
        for text in [WELL_FORMED, BROKEN, WITH_DOCTYPE] {
            let doc = Document::read(text);
            let mut seen = [0; 4];
            for offset in 0..=text.len() + 1 {
                let c = doc.at(offset);
                let ok = |span: Span| {
                    assert!(
                        span.start <= span.end
                            && text.get(span.start as usize..span.end as usize).is_some(),
                        "{offset}: {span:?} in {text:?}"
                    )
                };
                let tag_ok = |tag: &Tag<'_>| {
                    ok(tag.span);
                    ok(tag.name.span);
                    for a in &tag.attrs {
                        ok(a.span);
                        ok(a.value.span);
                    }
                };
                if !text.is_char_boundary(offset) {
                    assert!(matches!(c.at, At::Elsewhere) && c.open.is_empty());
                    continue;
                }
                for e in &c.open {
                    tag_ok(&e.tag);
                    for k in &e.children {
                        ok(k.span);
                    }
                }
                let o = offset as u32;
                match &c.at {
                    At::Name(span) | At::Text(span) => {
                        ok(*span);
                        assert!(span.contains(o), "{offset}: {:?}", c.at);
                    }
                    At::Value(v) => {
                        tag_ok(&v.tag);
                        ok(v.attr.span);
                        ok(v.span);
                        assert!(v.span.contains(o), "{offset}: {:?}", c.at);
                    }
                    At::Elsewhere => {}
                }
                seen[match c.at {
                    At::Name(_) => 0,
                    At::Value(_) => 1,
                    At::Text(_) => 2,
                    At::Elsewhere => 3,
                }] += 1;
                if let (At::Text(_), Some(e)) = (&c.at, c.open.last())
                    && !e.tag.name.entity
                {
                    let name = &e.tag.name;
                    let want = match name.prefix() {
                        Some("xml" | "xmlns") => continue,
                        Some(p) => c
                            .prefixes
                            .get(p)
                            .map_or(Ns::Unknown, |n| Ns::Bound((&**n).into())),
                        None => c
                            .prefixes
                            .get("")
                            .map_or(Ns::Unbound, |n| Ns::Bound((&**n).into())),
                    };
                    assert_eq!(name.ns, want, "{offset}: {}", name.raw);
                }
            }
            assert!(seen.iter().all(|&n| n > 0), "{seen:?} in {text:?}");
        }
    }

    /// The steps [`Document::at`] takes on `text` at `offset` ([`tick`]).
    fn steps(text: &str, offset: usize) -> u64 {
        let doc = Document::read(text);
        STEPS.set(0);
        drop(doc.at(offset));
        STEPS.get()
    }

    fn items(from: usize, n: usize) -> String {
        (from..from + n)
            .map(|k| format!("  <owl:Class rdf:about=\"http://example.org/c{k}\">\n    <rdfs:subClassOf rdf:resource=\"http://example.org/d{k}\"/>\n    <!-- {k} -->\n  </owl:Class>\n"))
            .collect()
    }

    /// The cursor's work is linear in the offset: at the end of 2,000
    /// items, then of 8,000, it takes as many steps per item within 1%
    /// (as `owx::timing`'s `linear_work` bounds the reader's work); and
    /// what follows the offset costs nothing, past the construct the offset
    /// is in. The steps are the cursor's own ([`tick`]): the scanner's work
    /// inside a construct, the construct at the offset read to its end, and
    /// the caller's [`Prolog::read`] are not counted. (The counting
    /// allocator of `owx::timing` is that module's own.)
    #[test]
    fn the_work_is_linear_in_the_offset() {
        let open = "  <owl:Class rdf:about=\"http://example.org/";
        let make = |n: usize, tail: usize| {
            let before = format!("{RDF_HEAD}{}{open}", items(0, n));
            let text = format!("{before}x\"/>\n{}</rdf:RDF>\n", items(n, tail));
            (text, before.len())
        };
        let per_item = |n: usize| {
            let (text, offset) = make(n, 0);
            steps(&text, offset) as f64 / n as f64
        };
        let (small, big) = (per_item(2_000), per_item(8_000));
        eprintln!("cursor: {small} steps per item at 1x, {big} at 4x");
        assert!(
            (big - small).abs() <= small * 0.01,
            "{big} steps per item at 4x, {small} at 1x"
        );
        let [short, long] = [2_000, 8_000].map(|tail| {
            let (text, offset) = make(2_000, tail);
            steps(&text, offset)
        });
        assert_eq!(short, long);
        // Each item is read: a step per construct and per event at least.
        assert!(small >= 10.0, "{small}");
    }
}
