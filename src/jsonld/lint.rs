//! JSON-LD lints, from what [`super::parse_document`] records:
//! - `jsonld-undefined-term` (warning): a key the active context does not
//!   define, or a key, `@id` or `@type` value that looks like a keyword
//!   but is not one. JSON-LD drops both silently. A term the context maps
//!   to `null` is dropped on purpose and is not reported;
//! - `invalid-iri` (warning, as Turtle reports an invalid IRI) at the IRI
//!   whose triples are dropped. A relative IRI with no base to resolve it
//!   is not reported: in the editor and `check`, the document's URL is the
//!   base;
//! - `invalid-language-tag` (error, as in Turtle) at the tag whose literal
//!   is dropped.
//!
//! [`vocabulary_iris`] says where the shared vocabulary lints
//! (`unknown-term`, `deprecated-term`) look: a term of the context is
//! checked once, at the IRI its definition writes, not at each use.

use super::{DropReason, Dropped, UsedTerm};
use crate::syntax::{Form, Iri, Ref, Severity, Span};

pub const UNDEFINED_TERM: &str = "jsonld-undefined-term";
pub const INVALID_IRI: &str = "invalid-iri";
pub const INVALID_LANGUAGE_TAG: &str = "invalid-language-tag";

/// One lint result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub span: Span,
    pub severity: Severity,
    pub code: &'static str,
    pub message: String,
}

fn at(text: &str, span: Span) -> &str {
    text.get(span.start as usize..span.end as usize)
        .unwrap_or_default()
}

/// The lints for what JSON-LD dropped from `text`.
pub fn dropped(text: &str, dropped: &[Dropped]) -> Vec<Finding> {
    dropped
        .iter()
        .filter_map(|d| {
            let written = at(text, d.span);
            let (severity, code, message) = match d.reason {
                DropReason::UndefinedTerm => (
                    Severity::Warning,
                    UNDEFINED_TERM,
                    format!(
                        "'{written}' is not defined by the context (no term, prefix or @vocab maps it), so JSON-LD drops it"
                    ),
                ),
                DropReason::KeywordLike => (
                    Severity::Warning,
                    UNDEFINED_TERM,
                    format!(
                        "'{written}' looks like a keyword but is not one, so JSON-LD ignores it"
                    ),
                ),
                DropReason::InvalidIri => {
                    let mut m = format!("Invalid IRI '{written}'");
                    if let Err(e) = oxiri::Iri::parse(written) {
                        m.push_str(&format!(": {e}"));
                    }
                    m.push_str(". JSON-LD drops what it is in");
                    (Severity::Warning, INVALID_IRI, m)
                }
                DropReason::InvalidLanguageTag => {
                    let mut m = format!("Invalid language tag '{written}'");
                    if let Err(e) = oxilangtag::LanguageTag::parse(written) {
                        m.push_str(&format!(": {e}"));
                    }
                    m.push_str(". JSON-LD drops the literal");
                    (Severity::Error, INVALID_LANGUAGE_TAG, m)
                }
                DropReason::NullTerm | DropReason::RelativeIri | DropReason::BlankNodePredicate => {
                    return None;
                }
            };
            Some(Finding {
                span: d.span,
                severity,
                code,
                message,
            })
        })
        .collect()
}

/// A vocabulary IRI where it is written: a reference, or a term
/// definition's IRI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrittenIri {
    pub span: Span,
    pub iri: Iri,
    /// A compact IRI with a defined prefix, or anything else written in
    /// full (an absolute IRI, a `@vocab` word, a relative IRI); never `a`.
    pub form: Form,
}

/// The IRIs of `refs` for the vocabulary lints, in text order: each
/// reference that spells its IRI itself and, for those that are terms of
/// the context (`used`, sorted by span), the IRI their definition writes,
/// once. A term of a remote context is left out: its definition is not in
/// the document.
pub fn vocabulary_iris(text: &str, refs: &[Ref], used: &[UsedTerm]) -> Vec<WrittenIri> {
    let mut out = Vec::new();
    let mut definitions = rustc_hash::FxHashSet::default();
    let key = |s: Span| (s.start, s.end);
    let mut u = 0;
    for r in refs.iter().filter(|r| r.form != Form::A) {
        while u < used.len() && key(used[u].span) < key(r.span) {
            u += 1;
        }
        match used.get(u).filter(|t| t.span == r.span) {
            None => out.push(WrittenIri {
                span: r.span,
                iri: r.iri,
                form: r.form,
            }),
            Some(UsedTerm {
                definition: Some(def),
                ..
            }) => {
                if definitions.insert((*def, r.iri)) {
                    out.push(WrittenIri {
                        span: *def,
                        iri: r.iri,
                        form: form_of(at(text, *def), &r.iri),
                    });
                }
            }
            Some(_) => {}
        }
    }
    out.sort_by_key(|w| key(w.span));
    out
}

/// How `written` spells `iri`: a compact IRI when what follows its colon
/// ends the IRI, else in full.
fn form_of(written: &str, iri: &str) -> Form {
    match written.find(':') {
        Some(i)
            if i > 0
                && written != iri
                && !written[i + 1..].starts_with("//")
                && iri.ends_with(&written[i + 1..]) =>
        {
            Form::PName {
                prefix_len: i as u32,
            }
        }
        _ => Form::Iri,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jsonld::{NoNetwork, parse_document};

    const BASE: &str = "file:///x/a.jsonld";

    fn findings(text: &str) -> Vec<(&'static str, Severity, Span)> {
        let d = parse_document(text, Some(BASE), &NoNetwork::default());
        dropped(text, &d.dropped)
            .into_iter()
            .map(|f| (f.code, f.severity, f.span))
            .collect()
    }

    fn iris(text: &str) -> Vec<(&str, String, Form)> {
        let d = parse_document(text, Some(BASE), &NoNetwork::default());
        vocabulary_iris(text, &d.parsed.refs, &d.used_terms)
            .into_iter()
            .map(|w| (at(text, w.span), w.iri.to_string(), w.form))
            .collect()
    }

    /// The span of the only occurrence of `s` in `text`.
    fn span_of(text: &str, s: &str) -> Span {
        assert_eq!(text.matches(s).count(), 1, "{s}");
        let i = text.find(s).unwrap() as u32;
        Span::new(i, i + s.len() as u32)
    }

    // LINTS of test/jsonld-fixtures.mjs.
    const LINTS: &str = r#"{
  "@context": {
    "ex": "https://jsonld.example.org/def/",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "vcard": "http://www.w3.org/2006/vcard/ns#",
    "weight": "ex:weight",
    "title": "rdfs:lable",
    "seeAlso": { "@id": "rdfs:seeAlso", "@type": "@id" }
  },
  "@id": "ex:w11",
  "@type": "vcard:Email",
  "weight": 3,
  "wieght": 4,
  "rdfs:labl": "Widget eleven",
  "title": "Eleven",
  "seeAlso": "http://exa mple.org/x",
  "rdfs:comment": { "@value": "Hallo", "@language": "de_DE" }
}
"#;

    #[test]
    fn dropped_keys_iris_and_tags_at_their_spans() {
        use Severity::*;
        assert_eq!(
            findings(LINTS),
            [
                (UNDEFINED_TERM, Warning, Span::new(344, 350)),
                (INVALID_IRI, Warning, Span::new(423, 444)),
                (INVALID_LANGUAGE_TAG, Error, Span::new(500, 505)),
            ]
        );
        assert_eq!(span_of(LINTS, "wieght"), Span::new(344, 350));
        assert_eq!(span_of(LINTS, "http://exa mple.org/x"), Span::new(423, 444));
        assert_eq!(span_of(LINTS, "de_DE"), Span::new(500, 505));
    }

    #[test]
    fn keyword_like_is_reported_and_null_terms_are_not() {
        let text = r#"{"@context": {"@vocab": "http://ex.org/", "gone": null}, "@foo": 1, "gone": 2, "p": {"@id": "@bar"}, "@type": "@baz", "q": 3}"#;
        let w = Severity::Warning;
        assert_eq!(
            findings(text),
            [
                (UNDEFINED_TERM, w, span_of(text, "@foo")),
                (UNDEFINED_TERM, w, span_of(text, "@bar")),
                (UNDEFINED_TERM, w, span_of(text, "@baz")),
            ]
        );
    }

    #[test]
    fn keys_the_context_defines_are_not_undefined() {
        // LINTS_CLEAN and VOCAB_KEYS of test/jsonld-fixtures.mjs.
        let clean = r#"{
  "@context": { "ex": "https://jsonld.example.org/def/", "weight": "ex:weight" },
  "@id": "ex:w18",
  "@type": "ex:Widget",
  "weight": 18,
  "ex:colour": "green",
  "http://www.w3.org/2000/01/rdf-schema#label": "w18"
}"#;
        let vocab = r#"{
  "@context": { "@vocab": "https://jsonld.example.org/def/" },
  "@id": "https://jsonld.example.org/def/w12",
  "colour": "red",
  "colur": "blue",
  "http://www.w3.org/2000/01/rdf-schema#label": "w12"
}"#;
        assert_eq!(findings(clean), []);
        assert_eq!(findings(vocab), []);
    }

    #[test]
    fn relative_iris_without_a_base_are_not_reported() {
        let text = r#"{"@context": {"@vocab": "http://ex.org/"}, "@id": "relative", "p": {"@id": "other"}}"#;
        let d = parse_document(text, None, &NoNetwork::default());
        assert!(!d.dropped.is_empty());
        assert_eq!(dropped(text, &d.dropped), []);
    }

    #[test]
    fn a_term_is_checked_once_at_the_iri_its_definition_writes() {
        let pn = |n| Form::PName { prefix_len: n };
        let rdfs = "http://www.w3.org/2000/01/rdf-schema#";
        let got: Vec<_> = iris(LINTS);
        assert_eq!(
            got,
            [
                (
                    "ex:weight",
                    "https://jsonld.example.org/def/weight".to_string(),
                    pn(2)
                ),
                ("rdfs:lable", format!("{rdfs}lable"), pn(4)),
                ("rdfs:seeAlso", format!("{rdfs}seeAlso"), pn(4)),
                ("ex:w11", "https://jsonld.example.org/def/w11".into(), pn(2)),
                (
                    "vcard:Email",
                    "http://www.w3.org/2006/vcard/ns#Email".into(),
                    pn(5)
                ),
                ("rdfs:labl", format!("{rdfs}labl"), pn(4)),
                ("rdfs:comment", format!("{rdfs}comment"), pn(4)),
            ]
        );
        // A term used twice is one definition; @vocab words are their uses.
        let text = r#"{"@context": {"l": {"@id": "http://www.w3.org/2000/01/rdf-schema#lable"}, "@vocab": "http://v.org/"}, "@graph": [{"@id": "http://ex.org/a", "l": 1, "T": 2}, {"@id": "http://ex.org/b", "l": 2, "@type": "Persn"}]}"#;
        assert_eq!(
            iris(text),
            [
                (
                    "http://www.w3.org/2000/01/rdf-schema#lable",
                    format!("{rdfs}lable"),
                    Form::Iri
                ),
                ("http://ex.org/a", "http://ex.org/a".into(), Form::Iri),
                ("T", "http://v.org/T".into(), Form::Iri),
                ("http://ex.org/b", "http://ex.org/b".into(), Form::Iri),
                ("Persn", "http://v.org/Persn".into(), Form::Iri),
            ]
        );
    }

    #[test]
    fn terms_their_key_spells_are_checked_at_each_use() {
        // A compact IRI term without @id is checked where it is used; a
        // @reverse definition at its value.
        let text = r#"{"@context": {"rdfs": "http://www.w3.org/2000/01/rdf-schema#", "rdfs:labl": {"@language": "en"}, "back": {"@reverse": "rdfs:seeAlsoo"}}, "@id": "http://ex.org/a", "rdfs:labl": "x", "back": {"@id": "http://ex.org/b"}}"#;
        let got: Vec<_> = iris(text).into_iter().map(|(s, _, f)| (s, f)).collect();
        assert_eq!(
            got,
            [
                ("rdfs:seeAlsoo", Form::PName { prefix_len: 4 }),
                ("http://ex.org/a", Form::Iri),
                ("rdfs:labl", Form::Iri),
                ("http://ex.org/b", Form::Iri),
            ]
        );
    }

    #[test]
    fn terms_of_a_remote_context_are_not_checked() {
        let text = r#"{"@context": "https://schema.org/", "@id": "http://ex.org/a", "name": "x"}"#;
        let d = parse_document(text, Some(BASE), &crate::jsonld::loader::Bundled);
        assert_eq!(
            d.used_terms,
            [UsedTerm {
                span: span_of(text, "name"),
                definition: None
            }]
        );
        let got: Vec<_> = vocabulary_iris(text, &d.parsed.refs, &d.used_terms)
            .into_iter()
            .map(|w| at(text, w.span))
            .collect();
        assert_eq!(got, ["http://ex.org/a"]);
    }
}
