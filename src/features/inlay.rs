//! Label inlay hints: the label of a term shown after it, when the name it is
//! written with does not already say it (`ex:P0042` ‹pump pressure›).

use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Knowledge;
use crate::settings::InlayLabels;
use crate::syntax::Form;
use crate::vocab::VOCABS;
use lsp_types::{InlayHint, InlayHintLabel, InlayHintTooltip, Range};

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
        let local = match r.form {
            Form::A => continue,
            Form::PName { prefix_len } => {
                &a.text[(r.span.start + prefix_len + 1) as usize..r.span.end as usize]
            }
            Form::Iri => local_of(&r.iri),
        };
        let Some(label) = label_of(k, &r.iri) else {
            continue;
        };
        if mode == InlayLabels::Opaque && !is_opaque(local, label) {
            continue;
        }
        let text = display(label);
        if text.is_empty() {
            continue;
        }
        out.push(InlayHint {
            position: a.position(enc, r.span.end),
            label: InlayHintLabel::String(text),
            kind: None,
            text_edits: None,
            tooltip: Some(InlayHintTooltip::String(format!("<{}>", r.iri))),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }
    out
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
fn local_of(iri: &str) -> &str {
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
    use crate::syntax::PrefixMap;
    use lsp_types::Position;
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
