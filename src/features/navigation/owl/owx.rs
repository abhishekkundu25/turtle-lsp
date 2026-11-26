//! The OWL/XML writer of the OWL rename planner: the text that names a new
//! IRI where a use of the old one is written, an `IRI` or `abbreviatedIRI`
//! attribute value or the text of an `<IRI>`, `<AbbreviatedIRI>` or other
//! IRI element.
//!
//! A full IRI is written as the new IRI, escaped for where it stands (an
//! attribute value between its quotes, element text, a CDATA section); a
//! relative one stays relative to the `xml:base` in scope when the new IRI
//! starts with the same text the old one adds the reference to, else it is
//! written in full. The `xml:base` comes from the text: an `xml:base` the
//! text does not resolve to an absolute IRI without the document's URL
//! counts as none. An abbreviated IRI keeps its prefix, else takes another
//! declared one (the longest namespace first), else becomes an `IRI`
//! attribute; an `<AbbreviatedIRI>` element with no prefix that covers the
//! new IRI is not rewritten.
//!
//! [`spells`] is the planner's check of a broken document for OWL/XML: a
//! value or text that spells the old IRI only through a reference.

use crate::rdfxml::rename::escaped;
use crate::rdfxml::xml::{Event, Prolog, Refs, Scanner, XML_NS};
use crate::syntax::names::is_pn_local;
use crate::syntax::{Form, PrefixMap, Span, has_scheme};
use std::cmp::Reverse;

/// XML's white space (XML 1.0, 2.3).
const WHITE: &[char] = &[' ', '\t', '\n', '\r'];

/// Where the text of a use stands.
#[derive(Clone, Copy)]
enum Place {
    /// Between the quotes (`"` or `'`) of an attribute value.
    Attr(u8),
    /// In an element's content.
    Text,
    /// In a CDATA section, which takes no reference.
    CData,
}

impl Place {
    /// Where the use at `span` of `text` stands: before it, after XML white
    /// space, an attribute's quote, a tag's `>` or a CDATA section's start.
    fn of(text: &str, span: Span) -> Option<Place> {
        let before = text[..span.start as usize].trim_end_matches(WHITE);
        match before.as_bytes().last()? {
            q @ (b'"' | b'\'') => Some(Place::Attr(*q)),
            b'>' => Some(Place::Text),
            b'[' if before.ends_with("<![CDATA[") => Some(Place::CData),
            _ => None,
        }
    }

    /// `s` written here.
    fn written(self, s: &str) -> String {
        match self {
            Place::Attr(q) => escaped(s, q),
            Place::Text => escaped(s, 0),
            Place::CData => s.to_string(),
        }
    }
}

/// Whether OWL/XML `text` may spell `old` with references: an attribute
/// value or a text that the scanner reads with a reference expanded (a
/// character reference, a predefined or a declared entity) and that
/// [`super::spells`] says may spell it.
pub(super) fn spells(text: &str, old: &str) -> bool {
    if !text.contains('&') {
        return false;
    }
    let prolog = Prolog::read(text);
    let mut scanner = Scanner::new(text, &prolog);
    loop {
        match scanner.next() {
            Event::Start(tag) => {
                let spelled = |v: &crate::rdfxml::xml::Value| {
                    v.refs != Refs::None && super::spells(&v.text, old)
                };
                if tag.attrs.iter().any(|a| spelled(&a.value)) {
                    return true;
                }
            }
            Event::Text { span, text: t } => {
                if t != text[span.start as usize..span.end as usize] && super::spells(&t, old) {
                    return true;
                }
            }
            Event::Eof => return false,
            Event::End(_) | Event::Pi(..) => {}
        }
    }
}

/// The edit that writes `new` for the use of an IRI at `span` (in form
/// `form`) of OWL/XML `text`, with the declared prefixes `prefixes`; `Err`
/// with why no text names `new` there.
pub(super) fn written(
    text: &str,
    span: Span,
    form: Form,
    new: &str,
    prefixes: &PrefixMap,
) -> Result<(Span, String), String> {
    let raw = &text[span.start as usize..span.end as usize];
    let place = || Place::of(text, span).ok_or("the IRI is written where a rename does not write");
    match form {
        Form::Iri => {
            let place = place()?;
            let value = (!has_scheme(raw))
                .then(|| base_at(text, span.start))
                .flatten()
                .and_then(|base| relative(&base, raw, new));
            Ok((span, place.written(value.as_deref().unwrap_or(new))))
        }
        Form::PName { prefix_len } => {
            let place = place()?;
            if let Some(name) = abbreviated(raw, prefix_len, new, prefixes) {
                return Ok((span, place.written(&name)));
            }
            match place {
                Place::Attr(q) => {
                    let start = attribute_start(text, span)
                        .ok_or("the abbreviated IRI is in no abbreviatedIRI attribute")?;
                    let value = escaped(new, q);
                    let q = char::from(q);
                    Ok((Span::new(start, span.end), format!("IRI={q}{value}")))
                }
                Place::Text | Place::CData => Err(format!(
                    "no declared prefix covers <{new}>; write it as <IRI> first"
                )),
            }
        }
        Form::A | Form::Anchor => Err(super::ANCHORED.into()),
    }
}

/// `new` as an abbreviated IRI: with the prefix of `raw` (the first
/// `prefix_len` bytes) if it still covers `new` with a local name the
/// reader takes (`PN_LOCAL` without backslash escapes, or empty), else
/// with another declared prefix that does, the longest namespace first.
fn abbreviated(raw: &str, prefix_len: u32, new: &str, prefixes: &PrefixMap) -> Option<String> {
    let own = &raw[..prefix_len as usize];
    let fits = |p: &str, ns: &str| {
        let local = new
            .strip_prefix(ns)
            .filter(|l| l.is_empty() || is_pn_local(l))?;
        Some(format!("{p}:{local}"))
    };
    if let Some(name) = prefixes.get(own).and_then(|ns| fits(own, ns)) {
        return Some(name);
    }
    let mut others: Vec<(&String, &String)> = prefixes.iter().filter(|(p, _)| *p != own).collect();
    others.sort_by_key(|(p, ns)| (Reverse(ns.len()), p.as_str()));
    others.into_iter().find_map(|(p, ns)| fits(p, ns))
}

/// The start of the `abbreviatedIRI` attribute whose value is at `span`.
fn attribute_start(text: &str, span: Span) -> Option<u32> {
    let before = text[..span.start as usize - 1].trim_end_matches(WHITE);
    let before = before.strip_suffix('=')?.trim_end_matches(WHITE);
    let name = before.strip_suffix("abbreviatedIRI")?;
    name.ends_with(WHITE).then_some(name.len() as u32)
}

/// The relative reference to write for `new` where `raw` resolved against
/// `base`: what `new` adds to the text the resolved `raw` adds `raw` to,
/// when that is relative, not empty, and resolves to `new` from `base`.
fn relative(base: &oxiri::Iri<String>, raw: &str, new: &str) -> Option<String> {
    let old = base.resolve(&oxiri::IriRef::parse(raw).ok()?).ok()?;
    let head = old.as_str().strip_suffix(raw)?;
    let r = new
        .strip_prefix(head)
        .filter(|r| !r.is_empty() && !has_scheme(r))?;
    let resolved = base.resolve(&oxiri::IriRef::parse(r).ok()?).ok()?;
    (resolved.as_str() == new).then(|| r.to_string())
}

/// The base in scope at `offset` of OWL/XML `text` (the element whose tag
/// or content holds it, and the elements around it): the nearest
/// `xml:base`, resolved against the one outside it, when it resolves to
/// an absolute IRI without the document's URL: a relative `xml:base` with
/// no base outside it that the text gives counts as none.
fn base_at(text: &str, offset: u32) -> Option<oxiri::Iri<String>> {
    if !text.contains("xml:base") {
        return None;
    }
    let prolog = Prolog::read(text);
    let mut scanner = Scanner::new(text, &prolog);
    let mut depth = 0u32;
    let mut scopes: Vec<(u32, Option<oxiri::Iri<String>>)> = Vec::new();
    loop {
        match scanner.next() {
            Event::Start(tag) => {
                if tag.span.start > offset {
                    break;
                }
                depth += 1;
                let base = tag
                    .attrs
                    .iter()
                    .find(|a| a.name.ns() == Some(XML_NS) && a.name.local() == "base");
                if let Some(a) = base {
                    let outer = scopes.last().and_then(|(_, b)| b.as_ref());
                    let value: &str = &a.value.text;
                    let resolved = oxiri::IriRef::parse(value).ok().and_then(|r| match outer {
                        Some(o) => o.resolve(&r).ok(),
                        // An absolute IRI, its dot segments removed as
                        // resolving it against the document's URL would.
                        None => oxiri::Iri::parse(value).ok()?.resolve(&r).ok(),
                    });
                    scopes.push((depth, resolved));
                }
                if tag.span.end > offset {
                    break;
                }
            }
            Event::End(_) => {
                depth = depth.saturating_sub(1);
                while scopes.last().is_some_and(|(d, _)| *d > depth) {
                    scopes.pop();
                }
            }
            Event::Text { span, .. } if span.end > offset => break,
            Event::Eof => break,
            Event::Text { .. } | Event::Pi(..) => {}
        }
    }
    scopes.pop().and_then(|(_, b)| b)
}

#[cfg(test)]
mod tests {
    use super::super::plan;
    use crate::analysis::Analysis;
    use crate::rdfxml::rename::{Refusal, apply, renamed};
    use crate::syntax::{Iri, Parsed, Severity, Syntax};

    const URI: &str = "file:///w/onto.owx";
    const EX: &str = "http://example.org/";

    /// `body` in an ontology with `ex:` (http://example.org/) declared.
    fn doc(body: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"{EX}o\">\n\
             <Prefix name=\"ex\" IRI=\"{EX}\"/>\n{body}\n</Ontology>\n"
        )
    }

    fn errors(p: &Parsed) -> Vec<String> {
        p.errors
            .iter()
            .filter(|e| e.severity == Severity::Error)
            .map(|e| e.message().to_string())
            .collect()
    }

    /// The triples of `p` with `from` read as `to`, in order.
    fn graph(p: &Parsed, from: &str, to: &str) -> Vec<String> {
        let (from, to) = (Iri::intern(from), Iri::intern(to));
        p.triples
            .iter()
            .map(|t| format!("{:?}", renamed(t, from, to)))
            .collect()
    }

    /// `text` after renaming `old` to `new`, checked: the text parses again
    /// with no error to the graph before with `old` read as `new`.
    fn renamed_in(text: &str, old: &str, new: &str) -> Result<String, Refusal> {
        let before = crate::owx::parse(text, Some(URI));
        let edits = plan(text, &before, Iri::intern(old), new)?;
        let out = apply(text, &edits);
        let after = crate::owx::parse(&out, Some(URI));
        assert_eq!(errors(&after), Vec::<String>::new(), "{out}");
        assert_eq!(graph(&after, old, new), graph(&before, old, new), "{out}");
        assert!(!graph(&after, new, new).is_empty(), "{out}");
        Ok(out)
    }

    fn renamed_ok(text: &str, old: &str, new: &str) -> String {
        renamed_in(text, old, new).unwrap_or_else(|r| panic!("{r:?}\n{text}"))
    }

    fn refused(text: &str, old: &str, new: &str) -> Refusal {
        match renamed_in(text, old, new) {
            Err(r) => r,
            Ok(out) => panic!("renamed:\n{out}"),
        }
    }

    fn ex(local: &str) -> String {
        format!("{EX}{local}")
    }

    #[test]
    fn an_abbreviated_iri_keeps_its_prefix() {
        let text = doc(
            "<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n\
             <SubClassOf><Class abbreviatedIRI=\"ex:B\"/><Class abbreviatedIRI='ex:A'/></SubClassOf>",
        );
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(out, text.replace("ex:A", "ex:Z"));
    }

    #[test]
    fn a_full_iri_stays_a_full_iri() {
        let text = doc(&format!(
            "<Declaration><Class IRI=\"{EX}A\"/></Declaration>\n\
             <SubClassOf><Class abbreviatedIRI=\"ex:B\"/><Class IRI=\" {EX}A \"/></SubClassOf>"
        ));
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(out, text.replace(&ex("A"), &ex("Z")));
    }

    #[test]
    fn a_new_iri_is_escaped_for_its_quote() {
        let text = doc(&format!(
            "<Declaration><Class IRI=\"{EX}A\"/></Declaration>\n\
             <SubClassOf><Class abbreviatedIRI=\"ex:B\"/><Class IRI='{EX}A'/></SubClassOf>"
        ));
        let out = renamed_ok(&text, &ex("A"), &ex("a?x=1&y='2'"));
        assert_eq!(
            out,
            doc(&format!(
                "<Declaration><Class IRI=\"{EX}a?x=1&amp;y='2'\"/></Declaration>\n\
                 <SubClassOf><Class abbreviatedIRI=\"ex:B\"/><Class IRI='{EX}a?x=1&amp;y=&apos;2&apos;'/></SubClassOf>"
            ))
        );
    }

    #[test]
    fn a_new_iri_with_a_double_quote_is_refused_as_no_iri() {
        let text = doc(&format!(
            "<Declaration><Class IRI=\"{EX}A\"/></Declaration>"
        ));
        let r = refused(&text, &ex("A"), &ex("a\"b"));
        assert_eq!(r.line, 4);
        assert!(
            r.reason
                .starts_with("the new name would make the document wrong here (Invalid IRI"),
            "{r:?}"
        );
    }

    #[test]
    fn a_relative_iri_stays_relative_to_its_base() {
        let text = format!(
            "<?xml version=\"1.0\"?>\n<Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" xml:base=\"{EX}onto\" ontologyIRI=\"{EX}onto\">\n\
             <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n\
             <Declaration><Class IRI=\"#A\"/></Declaration>\n\
             <Declaration><Class IRI=\"A\"/></Declaration>\n\
             <Declaration><Class xml:base=\"{EX}own/\" IRI=\"A\"/></Declaration>\n\
             <Declaration xml:base=\"{EX}inner/x\"><Class IRI=\"y/A\"/></Declaration>\n\
             <Declaration xml:base=\"inner/\"><Class IRI=\"y/A\"/></Declaration>\n\
             <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:comment\"/><IRI>#A</IRI><Literal>a</Literal></AnnotationAssertion>\n\
             </Ontology>\n"
        );
        // `#A` names onto#A; `A` and the nested `y/A` name A and inner/y/A.
        let out = renamed_ok(&text, &ex("onto#A"), &ex("onto#Z"));
        assert_eq!(
            out,
            text.replace("\"#A\"", "\"#Z\"")
                .replace("<IRI>#A</IRI>", "<IRI>#Z</IRI>")
        );
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(
            out,
            text.replace("<Class IRI=\"A\"/>", "<Class IRI=\"Z\"/>")
        );
        let out = renamed_ok(&text, &ex("inner/y/A"), &ex("inner/y/Z"));
        assert_eq!(out, text.replace("\"y/A\"", "\"y/Z\""));
        // An `xml:base` on the use's own element.
        let out = renamed_ok(&text, &ex("own/A"), &ex("own/Z"));
        assert_eq!(out, text.replace("own/\" IRI=\"A\"", "own/\" IRI=\"Z\""));
        // A path in the base's folder.
        let out = renamed_ok(&text, &ex("A"), &ex("sub/Z"));
        assert_eq!(
            out,
            text.replace("<Class IRI=\"A\"/>", "<Class IRI=\"sub/Z\"/>")
        );
        // Out of the base's reach, or where what the new IRI adds would
        // not resolve to it from the base: in full.
        let out = renamed_ok(&text, &ex("onto#A"), "http://other.example/Z");
        assert_eq!(
            out,
            text.replace("\"#A\"", "\"http://other.example/Z\"")
                .replace("<IRI>#A</IRI>", "<IRI>http://other.example/Z</IRI>")
        );
        // The base itself: no empty reference.
        let out = renamed_ok(&text, &ex("onto#A"), &ex("onto"));
        assert_eq!(
            out,
            text.replace("\"#A\"", &format!("\"{EX}onto\""))
                .replace("<IRI>#A</IRI>", &format!("<IRI>{EX}onto</IRI>"))
        );
        let out = renamed_ok(&text, &ex("onto#A"), &ex("ontoZ"));
        assert_eq!(
            out,
            text.replace("\"#A\"", &format!("\"{EX}ontoZ\""))
                .replace("<IRI>#A</IRI>", &format!("<IRI>{EX}ontoZ</IRI>"))
        );
    }

    #[test]
    fn a_relative_iri_with_no_base_in_the_text_is_written_in_full() {
        // The document's URL is the base: the text does not say it.
        let text = doc("<Declaration><Class IRI=\"A\"/></Declaration>");
        let out = renamed_ok(&text, "file:///w/A", "file:///w/Z");
        assert_eq!(out, text.replace("IRI=\"A\"", "IRI=\"file:///w/Z\""));
    }

    #[test]
    fn another_declared_prefix_that_covers_the_new_name_is_taken_the_longest_first() {
        let text = doc("<Prefix name=\"o\" IRI=\"http://other.example/\"/>\n\
             <Prefix name=\"ov\" IRI=\"http://other.example/v/\"/>\n\
             <Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>");
        let out = renamed_ok(&text, &ex("A"), "http://other.example/v/Z");
        assert_eq!(out, text.replace("\"ex:A\"", "\"ov:Z\""));
    }

    #[test]
    fn an_abbreviated_iri_no_prefix_covers_becomes_a_full_one() {
        let text = doc(
            "<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n\
             <SubClassOf><Class abbreviatedIRI = 'ex:A'/><Class abbreviatedIRI=\"ex:B\"/></SubClassOf>",
        );
        let out = renamed_ok(&text, &ex("A"), "http://other.example/Z&1");
        assert_eq!(
            out,
            doc(
                "<Declaration><Class IRI=\"http://other.example/Z&amp;1\"/></Declaration>\n\
                 <SubClassOf><Class IRI='http://other.example/Z&amp;1'/><Class abbreviatedIRI=\"ex:B\"/></SubClassOf>"
            )
        );
        // A local name the reader does not take, in the prefix's namespace.
        let out = renamed_ok(&text, &ex("A"), &ex("Z~1"));
        assert_eq!(
            out,
            text.replace("abbreviatedIRI=\"ex:A\"", &format!("IRI=\"{EX}Z~1\""))
                .replace("abbreviatedIRI = 'ex:A'", &format!("IRI='{EX}Z~1'"))
        );
    }

    #[test]
    fn iri_element_text_is_written_as_a_value() {
        let iris = format!(
            "<Declaration><AnnotationProperty abbreviatedIRI=\"ex:p\"/></Declaration>\n\
             <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"ex:p\"/><IRI>\n  {EX}A\n</IRI><IRI><![CDATA[{EX}A]]></IRI></AnnotationAssertion>"
        );
        let text = doc(&format!(
            "{iris}\n<AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"ex:p\"/><AbbreviatedIRI>ex:A</AbbreviatedIRI><Literal>a</Literal></AnnotationAssertion>"
        ));
        let out = renamed_ok(&text, &ex("A"), &ex("Z"));
        assert_eq!(
            out,
            text.replace(&ex("A"), &ex("Z")).replace("ex:A", "ex:Z")
        );
        // Escaped in content, as written in a CDATA section.
        let text = doc(&iris);
        let out = renamed_ok(&text, &ex("A"), &ex("B&C"));
        assert_eq!(
            out,
            doc(&format!(
                "<Declaration><AnnotationProperty abbreviatedIRI=\"ex:p\"/></Declaration>\n\
                 <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"ex:p\"/><IRI>\n  {EX}B&amp;C\n</IRI><IRI><![CDATA[{EX}B&C]]></IRI></AnnotationAssertion>"
            ))
        );
    }

    #[test]
    fn abbreviated_iri_text_no_prefix_covers_is_refused() {
        let text = doc(
            "<Declaration><AnnotationProperty abbreviatedIRI=\"ex:p\"/></Declaration>\n\
             <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"ex:p\"/><AbbreviatedIRI>ex:A</AbbreviatedIRI><Literal>a</Literal></AnnotationAssertion>",
        );
        let r = refused(&text, &ex("A"), "http://other.example/Z");
        assert_eq!(
            (r.line, r.reason.as_str()),
            (
                5,
                "no declared prefix covers <http://other.example/Z>; write it as <IRI> first"
            )
        );
    }

    #[test]
    fn a_use_spelled_with_a_reference_is_refused_with_its_line() {
        let text = doc(&format!(
            "<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n\
             <SubClassOf><Class abbreviatedIRI=\"ex:B\"/><Class IRI=\"{EX}&#65;\"/></SubClassOf>"
        ));
        let r = refused(&text, &ex("A"), &ex("Z"));
        assert_eq!((r.line, r.reason.as_str()), (5, super::super::ANCHORED));
        // A predefined entity too, in element text.
        let text = doc(&format!(
            "<Declaration><AnnotationProperty abbreviatedIRI=\"ex:p\"/></Declaration>\n\
             <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"ex:p\"/><IRI>{EX}a&amp;b</IRI><Literal>a</Literal></AnnotationAssertion>"
        ));
        let r = refused(&text, &ex("a&b"), &ex("Z"));
        assert_eq!((r.line, r.reason.as_str()), (5, super::super::ANCHORED));
    }

    #[test]
    fn a_broken_document_that_may_spell_the_iri_is_not_edited() {
        let text = doc("<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>\n<SubClassOf>");
        let before = crate::owx::parse(&text, Some(URI));
        assert!(!errors(&before).is_empty());
        let r = plan(&text, &before, Iri::intern(&ex("A")), &ex("Z")).unwrap_err();
        assert!(
            r.reason.starts_with("the document has a syntax error ("),
            "{r:?}"
        );
        assert!(
            r.reason.ends_with("): it is not edited until it is fixed"),
            "{r:?}"
        );
        let first = before.errors.iter().map(|e| e.span.start).min().unwrap();
        assert_eq!(r.line, crate::rdfxml::rename::line(&text, first));
    }

    #[test]
    fn a_broken_document_that_spells_the_iri_through_a_prefix_is_not_edited() {
        // OBO style: the prefix ends inside the local name (`GO:` for
        // `…/GO_`). The broken item gives no use of `GO_1`; only its
        // `abbreviatedIRI` and the quoted prefix IRI spell it.
        let syntax_error = |text: &str, old: &str| {
            let before = crate::owx::parse(text, Some(URI));
            assert!(!errors(&before).is_empty(), "{text}");
            let r = plan(text, &before, Iri::intern(old), &ex("Z")).unwrap_err();
            assert!(
                r.reason.starts_with("the document has a syntax error ("),
                "{old}: {r:?}"
            );
        };
        let prefix = format!("<Prefix name=\"GO\" IRI=\"{EX}GO_\"/>");
        // Only the broken item uses it.
        syntax_error(
            &doc(&format!(
                "{prefix}\n<SubClassOf><Class abbreviatedIRI=\"GO:1\"/>"
            )),
            &ex("GO_1"),
        );
        // A read use too: the refusal is the old error, not the new name.
        syntax_error(
            &doc(&format!(
                "{prefix}\n<Declaration><Class abbreviatedIRI=\"GO:1\"/></Declaration>\n\
                 <SubClassOf><Class abbreviatedIRI=\"GO:1\"/>"
            )),
            &ex("GO_1"),
        );
        // `IRI=""` names the document itself.
        syntax_error(&doc("<SubClassOf><Class IRI=\"\"/>"), URI);
    }

    #[test]
    fn a_broken_document_that_spells_the_iri_with_a_reference_is_not_edited() {
        // The broken item gives no use, and only a reference spells `A`.
        for value in ["&#65;", "&#x41;", "&a;"] {
            let text = doc(&format!(
                "<Declaration><Class abbreviatedIRI=\"ex:B\"/></Declaration>\n\
                 <SubClassOf><Class IRI=\"{EX}{value}\"/><Class abbreviatedIRI=\"ex:B\"/>"
            ))
            .replace(
                "<?xml version=\"1.0\"?>\n",
                "<?xml version=\"1.0\"?>\n<!DOCTYPE Ontology [<!ENTITY a \"&#65;\">]>\n",
            );
            let before = crate::owx::parse(&text, Some(URI));
            assert!(!errors(&before).is_empty());
            let r = plan(&text, &before, Iri::intern(&ex("A")), &ex("Z")).unwrap_err();
            assert!(
                r.reason.starts_with("the document has a syntax error ("),
                "{value}: {r:?}"
            );
            // Nor does it spell another IRI.
            let edits = plan(&text, &before, Iri::intern(&ex("Q")), &ex("Z")).unwrap();
            assert!(edits.is_empty(), "{value}");
        }
    }

    #[test]
    fn a_rename_that_changes_the_graph_is_refused() {
        // A literal of type rdf:PlainLiteral maps to a string with a
        // language tag: renaming its datatype there changes the literal.
        let text = doc(
            "<Declaration><Datatype abbreviatedIRI=\"ex:dt\"/></Declaration>\n\
             <DataPropertyAssertion><DataProperty abbreviatedIRI=\"ex:d\"/><NamedIndividual abbreviatedIRI=\"ex:a\"/><Literal datatypeIRI=\"http://example.org/dt\">x@en</Literal></DataPropertyAssertion>",
        );
        let before = crate::owx::parse(&text, Some(URI));
        assert_eq!(errors(&before), Vec::<String>::new());
        let plain = "http://www.w3.org/1999/02/22-rdf-syntax-ns#PlainLiteral";
        let r = plan(&text, &before, Iri::intern(&ex("dt")), plain).unwrap_err();
        assert!(
            r.reason
                .contains("the document would say something else than the rename means"),
            "{r:?}"
        );
        assert_eq!(r.line, 5);
    }

    #[test]
    fn the_ontology_and_version_iris_are_renamed_as_any_use() {
        let text = doc("<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>").replace(
            &format!("ontologyIRI=\"{EX}o\""),
            &format!("ontologyIRI=\"{EX}o\" versionIRI=\"{EX}v1\""),
        );
        let out = renamed_ok(&text, &ex("o"), &ex("p"));
        assert_eq!(
            out,
            text.replace(&format!("\"{EX}o\""), &format!("\"{EX}p\""))
        );
        let out = renamed_ok(&text, &ex("v1"), &ex("v2"));
        assert_eq!(out, text.replace("v1", "v2"));
    }

    #[test]
    fn an_owl_file_that_holds_owl_xml_is_renamed_as_owl_xml() {
        let text = doc("<Declaration><Class abbreviatedIRI=\"ex:A\"/></Declaration>");
        let uri = "file:///w/onto.owl";
        let a = Analysis::new(uri.into(), &text, std::sync::Arc::default(), true);
        assert_eq!(a.parsed.syntax, Syntax::OwlXml);
        let edits = plan(&text, &a.parsed, Iri::intern(&ex("A")), &ex("Z")).unwrap();
        assert_eq!(apply(&text, &edits), text.replace("ex:A", "ex:Z"));
    }
}
