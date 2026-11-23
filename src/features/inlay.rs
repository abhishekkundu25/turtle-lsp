//! Label inlay hints: the label of a term shown after it, when the name it is
//! written with does not already say it (`ex:P0042` ‹pump pressure›).

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::settings::InlayLabels;
use crate::syntax::{Form, Syntax};
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
    if mode == InlayLabels::Off || !hinted(a.parsed.syntax) {
        return vec![];
    }
    let from = a.offset(enc, range.start).unwrap_or(0);
    let to = a.offset(enc, range.end).unwrap_or(u32::MAX).max(from);
    let refs = &a.parsed.refs;
    let first = refs.partition_point(|r| r.span.end < from);
    let mut out = Vec::new();
    for r in refs[first..].iter().take_while(|r| r.span.start <= to) {
        let local = match r.form {
            // No name is written at an anchor for the label to stand beside.
            Form::A | Form::Anchor => continue,
            Form::PName { prefix_len } => {
                &a.text[(r.span.start + prefix_len + 1) as usize..r.span.end as usize]
            }
            Form::Iri => local_of(&r.iri),
        };
        out.extend(label_hint(k, &r.iri, local, mode, || {
            a.position(enc, r.span.end)
        }));
    }
    out
}

/// Whether a document of `syntax` gets label hints. Not RDF/XML: a hint
/// after an attribute value would sit inside its quotes, and one after an
/// element name between it and its attributes; the editor's XML support
/// owns that text. Nor OWL Functional syntax, which is read-only as RDF/XML
/// is: no text is put between its terms.
fn hinted(syntax: Syntax) -> bool {
    match syntax {
        Syntax::Turtle | Syntax::TriG | Syntax::NTriples | Syntax::NQuads | Syntax::JsonLd => true,
        Syntax::RdfXml | Syntax::OwlFunctional => false,
    }
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

    /// RDF/XML gets no hint, whether its references are exact (the
    /// frontend's) or spelt as in Turtle: the syntax decides, not the form.
    #[test]
    fn rdfxml_gets_no_hint() {
        use crate::features::rdfxml_fixture as fx;
        let uri = "file:///w/onto.owl";
        let hints = |a: Analysis| {
            let a = Arc::new(a);
            let mut k = Knowledge::default();
            k.set(uri, Source::Open, a.clone());
            let all = Range::new(Position::new(0, 0), Position::new(99, 0));
            inlay_hints(&a, &k, Encoding::Utf16, all, InlayLabels::All).len()
        };
        assert!(hints(fx::turtle(uri)) > 0);
        let read = fx::read(uri);
        assert!(
            read.parsed
                .refs
                .iter()
                .any(|r| r.iri.as_str() == "http://example.org/A"
                    && matches!(r.form, Form::PName { .. }))
        );
        assert_eq!(hints(read), 0);
        assert_eq!(hints(fx::written(uri, fx::TURTLE)), 0);
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
