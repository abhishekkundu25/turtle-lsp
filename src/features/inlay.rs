//! Label inlay hints: the label of a term shown after it, when the name it is
//! written with does not already say it (`ex:P0042` ‹pump pressure›).

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::settings::InlayLabels;
use crate::syntax::{Form, Ref, Syntax};
use crate::vocab::VOCABS;
use lsp_types::{InlayHint, InlayHintLabel, InlayHintTooltip, Position, Range};

/// Hints longer than this are cut, with an ellipsis.
const MAX_CHARS: usize = 60;

pub fn inlay_hints(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    range: Range,
    mode: InlayLabels,
) -> Vec<InlayHint> {
    if mode == InlayLabels::Off {
        return vec![];
    }
    let from = a.offset(enc, range.start).unwrap_or(0);
    let to = a.offset(enc, range.end).unwrap_or(u32::MAX).max(from);
    let refs = &a.parsed.refs;
    let first = refs.partition_point(|r| r.span.end < from);
    let mut out = Vec::new();
    for r in refs[first..].iter().take_while(|r| r.span.start <= to) {
        let Some((at, local)) = placed(a.parsed.syntax, &a.text, r) else {
            continue;
        };
        out.extend(label_hint(k, &r.iri, local, mode, || a.position(enc, at)));
    }
    out
}

/// Where the label hint of `r` goes in `text`, a document of `syntax`, and
/// the local name written there; `None` where no name is written for the
/// label to stand beside. In Turtle, JSON-LD and OWL Functional syntax the
/// hint follows the term. In XML it follows the markup that closes the
/// term: an attribute value's closing quote, or the end tag of an
/// element's text (`</IRI>`); none follows a qualified name, an element's
/// or an attribute's, where it would part the name from what comes next.
fn placed<'a>(syntax: Syntax, text: &'a str, r: &'a Ref) -> Option<(u32, &'a str)> {
    let local = match r.form {
        // No name is written at an anchor for the label to stand beside.
        Form::A | Form::Anchor => return None,
        Form::PName { prefix_len } => {
            &text[(r.span.start + prefix_len + 1) as usize..r.span.end as usize]
        }
        Form::Iri => local_of(&r.iri),
    };
    let at = match syntax {
        Syntax::Turtle
        | Syntax::TriG
        | Syntax::NTriples
        | Syntax::NQuads
        | Syntax::JsonLd
        | Syntax::OwlFunctional => r.span.end,
        Syntax::RdfXml | Syntax::OwlXml => {
            let rest = &text[r.span.end as usize..];
            let next = rest.trim_start_matches([' ', '\t', '\n', '\r']);
            let start = r.span.end + (rest.len() - next.len()) as u32;
            if next.starts_with(['"', '\'']) {
                start + 1
            } else if next.starts_with("</") {
                start + next.find('>')? as u32 + 1
            } else {
                return None;
            }
        }
    };
    Some((at, local))
}

/// The label hint for `iri`, written with local name `local` (the part of
/// the name that may say what the label says), at `position()`; `None`
/// when it has no label or (in `opaque` mode) the name says it already.
pub(crate) fn label_hint(
    k: &Knowledge,
    iri: &str,
    local: &str,
    mode: InlayLabels,
    position: impl FnOnce() -> Position,
) -> Option<InlayHint> {
    let label = label_of(k, iri)?;
    if mode == InlayLabels::Off || (mode == InlayLabels::Opaque && !is_opaque(local, label)) {
        return None;
    }
    let text = display(label);
    if text.is_empty() {
        return None;
    }
    Some(InlayHint {
        position: position(),
        label: InlayHintLabel::String(text),
        kind: None,
        text_edits: None,
        tooltip: Some(InlayHintTooltip::String(format!("<{iri}>"))),
        padding_left: Some(true),
        padding_right: None,
        data: None,
    })
}

/// The workspace label, else the vocabulary's.
fn label_of<'a>(k: &'a Knowledge, iri: &str) -> Option<&'a str> {
    k.label(iri).filter(|l| !l.trim().is_empty()).or_else(|| {
        let (v, local) = VOCABS.lookup(iri)?;
        v.term(local)
            .map(|t| t.label.as_str())
            .filter(|l| !l.is_empty())
    })
}

/// The last segment of a full IRI (after `#`, `/` or `:`, ignoring a
/// trailing `/`).
pub(crate) fn local_of(iri: &str) -> &str {
    let s = iri.strip_suffix('/').unwrap_or(iri);
    s.rfind(['#', '/', ':']).map_or(s, |i| &s[i + 1..])
}

/// One line, whitespace collapsed, at most [`MAX_CHARS`] characters.
fn display(label: &str) -> String {
    let one_line = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= MAX_CHARS {
        return one_line;
    }
    let mut s: String = one_line.chars().take(MAX_CHARS - 1).collect();
    s.push('…');
    s
}

/// Lowercased words: split at non-alphanumerics, at camelCase humps
/// (`hasPart`, `HTTPServer`) and between letters and digits.
fn words(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if !cur.is_empty() {
            let p = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            let hump = (p.is_lowercase() && c.is_uppercase())
                || (p.is_uppercase() && c.is_uppercase() && next_lower)
                || (p.is_numeric() != c.is_numeric());
            if hump {
                out.push(std::mem::take(&mut cur));
            }
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whether the written local name fails to say what the label says. It is
/// readable when its normalised words are the label's or a run of them, when
/// it contains every word of the label (`CNC-spindle` for "Spindle",
/// `bearingFront` for "Front bearing"), or (for names of three or more
/// letters) when its letters occur in the label's (`pumppressure`).
fn is_opaque(local: &str, label: &str) -> bool {
    let (lw, bw) = (words(local), words(label));
    if bw.is_empty() {
        return false;
    }
    if lw.is_empty() {
        return true;
    }
    if bw.windows(lw.len()).any(|w| w == lw.as_slice()) || bw.iter().all(|w| lw.contains(w)) {
        return false;
    }
    let (lj, bj) = (lw.concat(), bw.concat());
    !(lj.chars().count() >= 3 && bj.contains(&lj))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::knowledge::Source;
    use crate::syntax::{PrefixMap, Syntax};
    use std::sync::Arc;

    #[test]
    fn normalisation() {
        assert_eq!(words("hasPart"), ["has", "part"]);
        assert_eq!(words("HTTPServer2go"), ["http", "server", "2", "go"]);
        assert_eq!(words("pump_pressure-X"), ["pump", "pressure", "x"]);
        assert_eq!(words("P0042"), ["p", "0042"]);
    }

    #[test]
    fn opacity() {
        assert!(is_opaque("P0042", "pump pressure"));
        assert!(!is_opaque("Asset", "Asset"));
        assert!(!is_opaque("pumpPressure", "Pump pressure"));
        assert!(!is_opaque("pump_pressure", "The pump pressure (bar)"));
        assert!(!is_opaque("pumppressure", "Pump pressure"));
        assert!(!is_opaque("hasPart", "has part"));
        // A one-letter name says nothing even when the label starts with it.
        assert!(is_opaque("a", "Alice"));
        assert!(is_opaque("knows", "is acquainted with"));
        // The name says more than the label.
        assert!(!is_opaque("CNC-spindle", "Spindle"));
        assert!(!is_opaque("Spindle-bearingFront", "Front bearing"));
        assert!(is_opaque("Pump-bearingDE", "Drive-end bearing"));
        assert!(!is_opaque("x", "   "));
        assert_eq!(local_of("http://example.com/things/P1/"), "P1");
        assert_eq!(local_of("urn:x:P2"), "P2");
    }

    /// No hint stands beside a reference whose text is not at its span
    /// ([`Form::Anchor`]): no name is written there.
    #[test]
    fn an_anchored_reference_gets_no_hint() {
        let text = "<http://example.com/ns#P0042> <http://www.w3.org/2000/01/rdf-schema#label> \"pump pressure\" .\n\
                    <http://example.com/ns#x> <http://example.com/ns#p> <http://example.com/ns#P0042> .\n";
        let uri = "file:///t.ttl";
        let hints = |a: Analysis| {
            let a = Arc::new(a);
            let mut k = Knowledge::default();
            k.set(uri, Source::Open, a.clone());
            let all = Range::new(Position::new(0, 0), Position::new(2, 0));
            inlay_hints(&a, &k, Encoding::Utf16, all, InlayLabels::All).len()
        };
        assert!(hints(Analysis::new(uri.into(), text, Arc::default(), true)) > 0);
        assert_eq!(hints(Analysis::anchored(uri, text, Syntax::Turtle)), 0);
    }

    /// The hints of `text`, read as `syntax`, between lines `from` and `to`
    /// in `mode`: line, UTF-16 character and label of each. The labels are
    /// the document's own.
    fn shown(
        text: &str,
        syntax: Syntax,
        mode: InlayLabels,
        from: u32,
        to: u32,
    ) -> Vec<(u32, u32, String)> {
        let uri = "file:///w/onto";
        let a = Arc::new(Analysis::with_syntax(
            uri.into(),
            text,
            Arc::default(),
            true,
            syntax,
        ));
        assert!(a.parsed.errors.is_empty(), "{:?}", a.parsed.errors);
        let mut k = Knowledge::default();
        k.set(uri, Source::Open, a.clone());
        let range = Range::new(Position::new(from, 0), Position::new(to, 0));
        inlay_hints(&a, &k, Encoding::Utf16, range, mode)
            .into_iter()
            .map(|h| match h.label {
                InlayHintLabel::String(s) => (h.position.line, h.position.character, s),
                InlayHintLabel::LabelParts(_) => unreachable!(),
            })
            .collect()
    }

    /// The line and UTF-16 character just after the first `written` in
    /// `text`.
    fn after(text: &str, written: &str) -> (u32, u32) {
        let end = text.find(written).expect(written) + written.len();
        let line = text[..end].matches('\n').count() as u32;
        let start = text[..end].rfind('\n').map_or(0, |i| i + 1);
        (line, text[start..end].encode_utf16().count() as u32)
    }

    /// `(line, character, label)` after each of `at`.
    fn hints_after(text: &str, at: &[(&str, &str)]) -> Vec<(u32, u32, String)> {
        at.iter()
            .map(|(written, label)| {
                let (line, character) = after(text, written);
                (line, character, label.to_string())
            })
            .collect()
    }

    /// In OWL Functional syntax a hint follows the term, as in Turtle: a
    /// prefixed name, or a full IRI after its `>`.
    #[test]
    fn ofn_hints_follow_the_term() {
        let text = "Prefix(ex:=<http://example.org/>)\n\
                    Prefix(rdfs:=<http://www.w3.org/2000/01/rdf-schema#>)\n\
                    Ontology(<http://example.org/o>\n\
                    Declaration(Class(ex:P0042))\n\
                    AnnotationAssertion(rdfs:label ex:P0042 \"pump pressure\")\n\
                    SubClassOf(<http://example.org/P0042> ex:Asset)\n\
                    AnnotationAssertion(rdfs:label ex:Asset \"Asset\")\n\
                    )\n";
        let got = shown(text, Syntax::OwlFunctional, InlayLabels::Opaque, 0, 99);
        let want = hints_after(
            text,
            &[
                ("Class(ex:P0042", "pump pressure"),
                ("label ex:P0042", "pump pressure"),
                ("<http://example.org/P0042>", "pump pressure"),
            ],
        );
        assert_eq!(got, want);
    }

    /// In OWL/XML a hint follows an `IRI` or `abbreviatedIRI` value's
    /// closing quote (either quote, the value padded with spaces or not),
    /// and the end tag of `<IRI>` or `<AbbreviatedIRI>` text, on a later
    /// line too; none follows a value spelt through an entity, nor any
    /// element or attribute name. `opaque` hints opaque names only, `all`
    /// every labelled one, `off` none.
    #[test]
    fn owx_hints_follow_the_closing_quote_or_end_tag() {
        let text = "<?xml version=\"1.0\"?>\n\
            <!DOCTYPE Ontology [ <!ENTITY ex \"http://example.org/\"> ]>\n\
            <Ontology xmlns=\"http://www.w3.org/2002/07/owl#\" ontologyIRI=\"http://example.org/o\">\n\
            <Prefix name=\"ex\" IRI=\"http://example.org/\"/>\n\
            <Prefix name=\"rdfs\" IRI=\"http://www.w3.org/2000/01/rdf-schema#\"/>\n\
            <!-- é😀 --><Declaration><Class IRI=\"http://example.org/P0042\"/></Declaration>\n\
            <Declaration><Class abbreviatedIRI='ex:P0043'/></Declaration>\n\
            <Declaration><Class abbreviatedIRI=\"ex:Asset\"/></Declaration>\n\
            <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><IRI>http://example.org/P0042</IRI><Literal>pump pressure</Literal></AnnotationAssertion>\n\
            <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>\n  ex:P0043\n</AbbreviatedIRI><Literal>flow rate</Literal></AnnotationAssertion>\n\
            <AnnotationAssertion><AnnotationProperty abbreviatedIRI=\"rdfs:label\"/><AbbreviatedIRI>ex:Asset</AbbreviatedIRI><Literal>Asset</Literal></AnnotationAssertion>\n\
            <SubClassOf><Class IRI=\"&ex;P0042\"/><Class IRI=\" http://example.org/P0043 \"/></SubClassOf>\n\
            </Ontology>\n";
        let opaque = hints_after(
            text,
            &[
                ("IRI=\"http://example.org/P0042\"", "pump pressure"),
                ("abbreviatedIRI='ex:P0043'", "flow rate"),
                ("<IRI>http://example.org/P0042</IRI>", "pump pressure"),
                ("ex:P0043\n</AbbreviatedIRI>", "flow rate"),
                ("IRI=\" http://example.org/P0043 \"", "flow rate"),
            ],
        );
        assert_eq!(
            shown(text, Syntax::OwlXml, InlayLabels::Opaque, 0, 99),
            opaque
        );
        // The UTF-16 character: the emoji counts two (65 in bytes).
        assert_eq!(opaque[0].1, 62);
        // A range of one line holds the hints of its references only.
        assert_eq!(
            shown(text, Syntax::OwlXml, InlayLabels::Opaque, 5, 6),
            opaque[..1]
        );
        let all = shown(text, Syntax::OwlXml, InlayLabels::All, 7, 8);
        assert_eq!(
            all,
            hints_after(text, &[("abbreviatedIRI=\"ex:Asset\"", "Asset")])
        );
        assert!(shown(text, Syntax::OwlXml, InlayLabels::Off, 0, 99).is_empty());
    }

    /// In RDF/XML a hint follows an `rdf:about` or `rdf:resource` value's
    /// closing quote; none stands at an element or attribute name (a
    /// qualified name), nor after a value spelt through an entity.
    #[test]
    fn rdfxml_hints_follow_values_not_names() {
        let text = "<?xml version=\"1.0\"?>\n\
            <!DOCTYPE rdf:RDF [ <!ENTITY ex \"http://example.org/\"> ]>\n\
            <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"\n\
            \x20        xmlns:rdfs=\"http://www.w3.org/2000/01/rdf-schema#\"\n\
            \x20        xmlns:ex=\"http://example.org/\">\n\
            \x20 <rdf:Description rdf:about=\"http://example.org/P0042\">\n\
            \x20   <rdfs:label>pump pressure</rdfs:label>\n\
            \x20 </rdf:Description>\n\
            \x20 <rdf:Description rdf:about='http://example.org/Q7' ex:Q7=\"v\">\n\
            \x20   <rdfs:label>quality</rdfs:label>\n\
            \x20 </rdf:Description>\n\
            \x20 <ex:P0042 rdf:about=\"http://example.org/x\">\n\
            \x20   <ex:Q7 rdf:resource=\"http://example.org/P0042\"/>\n\
            \x20   <ex:Q7 rdf:resource=\"&ex;P0042\"/>\n\
            \x20 </ex:P0042>\n\
            </rdf:RDF>\n";
        // The element name ex:P0042 and the attribute name ex:Q7 are
        // references written at their text: their text, not their form,
        // gives them no hint.
        let a = Analysis::with_syntax(
            "file:///w/onto".into(),
            text,
            Arc::default(),
            true,
            Syntax::RdfXml,
        );
        for name in ["<ex:P0042 ", " ex:Q7="] {
            let start = (text.find(name).unwrap() + name.find("ex:").unwrap()) as u32;
            assert!(
                a.parsed
                    .refs
                    .iter()
                    .any(|r| r.span.start == start && matches!(r.form, Form::PName { .. })),
                "{name}"
            );
        }
        let got = shown(text, Syntax::RdfXml, InlayLabels::Opaque, 0, 99);
        let want = hints_after(
            text,
            &[
                ("about=\"http://example.org/P0042\"", "pump pressure"),
                ("about='http://example.org/Q7'", "quality"),
                ("resource=\"http://example.org/P0042\"", "pump pressure"),
            ],
        );
        assert_eq!(got, want);
    }

    #[test]
    fn hints_after_opaque_terms_by_mode() {
        let text = "@prefix ex: <http://example.com/ns#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    ex:P0042 rdfs:label \"pump  pressure\" .\n\
                    ex:Asset rdfs:label \"Asset\" .\n\
                    ex:x ex:p ex:P0042, ex:Asset, <http://example.com/ns#P0042> .\n";
        let a = Arc::new(Analysis::new(
            "file:///t.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            true,
        ));
        let mut k = Knowledge::default();
        k.set("file:///t.ttl", Source::Open, a.clone());
        let line4 = Range::new(Position::new(4, 0), Position::new(5, 0));
        let hints = inlay_hints(&a, &k, Encoding::Utf16, line4, InlayLabels::Opaque);
        let got: Vec<(u32, String)> = hints
            .iter()
            .map(|h| match &h.label {
                InlayHintLabel::String(s) => (h.position.character, s.clone()),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(
            got,
            [
                (18, "pump pressure".to_string()),
                (59, "pump pressure".to_string())
            ]
        );
        assert_eq!(hints[0].padding_left, Some(true));
        let all = inlay_hints(&a, &k, Encoding::Utf16, line4, InlayLabels::All);
        assert_eq!(all.len(), 3);
        let off = inlay_hints(&a, &k, Encoding::Utf16, line4, InlayLabels::Off);
        assert!(off.is_empty());
        // rdfs:label is readable as written; under `all` it gets the vocabulary label.
        let line2 = Range::new(Position::new(2, 0), Position::new(2, 99));
        let all2 = inlay_hints(&a, &k, Encoding::Utf16, line2, InlayLabels::All);
        assert_eq!(all2.len(), 2, "{all2:?}");
    }

    #[test]
    fn long_labels_are_cut() {
        let long = "word ".repeat(30);
        let d = display(&long);
        assert_eq!(d.chars().count(), MAX_CHARS);
        assert!(d.ends_with('…'));
    }
}
