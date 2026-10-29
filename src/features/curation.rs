//! Curation lints: mistakes in how a workspace's own terms are labelled,
//! defined and published, which the syntax does not catch.
//!
//! On by default: `duplicate-label`, `label-whitespace` and
//! `multiple-labels`. Off until `lint.enable` lists them ([`OPT_IN`]): the
//! house-style checks. `lint.ignore` switches any of them off; the
//! composer ([`crate::diagnose::document`]) applies both lists.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::features::quickfix::FixData;
use crate::jsonld::json;
use crate::knowledge::Knowledge;
use crate::syntax::lexer::{self, Kind};
use crate::syntax::{Iri, Node, Span, Syntax};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};

// shortcut: unused until the label checks land; the slices remove the allow.
#[allow(dead_code)]
pub(crate) const DUPLICATE_LABEL: &str = "duplicate-label";
pub(crate) const LABEL_WHITESPACE: &str = "label-whitespace";
#[allow(dead_code)]
pub(crate) const MULTIPLE_LABELS: &str = "multiple-labels";
pub(crate) const MISSING_LABEL: &str = "missing-label";
pub(crate) const MISSING_DEFINITION: &str = "missing-definition";
pub(crate) const MULTIPLE_DEFINITIONS: &str = "multiple-definitions";
pub(crate) const DUPLICATE_DEFINITION: &str = "duplicate-definition";
pub(crate) const LOWERCASE_DEFINITION: &str = "lowercase-definition";
pub(crate) const MISSING_ONTOLOGY_TITLE: &str = "missing-ontology-title";
pub(crate) const MISSING_ONTOLOGY_DESCRIPTION: &str = "missing-ontology-description";
pub(crate) const MISSING_ONTOLOGY_LICENSE: &str = "missing-ontology-license";

/// The checks that are off until `lint.enable` lists them: house style,
/// which projects that do not follow it should not see.
pub(crate) const OPT_IN: &[&str] = &[
    MISSING_LABEL,
    MISSING_DEFINITION,
    MULTIPLE_DEFINITIONS,
    DUPLICATE_DEFINITION,
    LOWERCASE_DEFINITION,
    MISSING_ONTOLOGY_TITLE,
    MISSING_ONTOLOGY_DESCRIPTION,
    MISSING_ONTOLOGY_LICENSE,
];

/// The curation diagnostics of `a`; the [`OPT_IN`] checks only with
/// `house_style`.
pub(crate) fn diagnostics(
    a: &Analysis,
    _k: &Knowledge,
    enc: Encoding,
    _house_style: bool,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    label_whitespace(a, enc, &mut out);
    out
}

/// The predicates whose literals [`label_whitespace`] checks, on any subject.
const WHITESPACE_CHECKED: [&str; 3] = [
    "http://www.w3.org/2000/01/rdf-schema#label",
    "http://www.w3.org/2004/02/skos/core#prefLabel",
    "http://www.w3.org/2004/02/skos/core#altLabel",
];

/// `label-whitespace`: a label literal whose value starts or ends with
/// whitespace ([`char::is_whitespace`]). The warning covers the literal's
/// quoted string, and its [`FixData::Replace`] is that string trimmed.
fn label_whitespace(a: &Analysis, enc: Encoding, out: &mut Vec<Diagnostic>) {
    let checked: Vec<Iri> = WHITESPACE_CHECKED
        .iter()
        .filter_map(|p| Iri::get(p))
        .collect();
    if checked.is_empty() {
        return;
    }
    // A JSON-LD value can give one literal to several triples.
    let mut seen: Vec<Span> = Vec::new();
    for t in a.parsed.triples.iter().filter(|t| checked.contains(&t.p)) {
        let Node::Literal { value, .. } = &t.o else {
            continue;
        };
        let (start, end) = (
            value.trim_start().len() < value.len(),
            value.trim_end().len() < value.len(),
        );
        let message = match (start, end) {
            (false, false) => continue,
            (true, false) => "Label starts with whitespace",
            (false, true) => "Label ends with whitespace",
            (true, true) => "Label starts and ends with whitespace",
        };
        let string = string_token(a, t.o_span, value);
        let span = string.unwrap_or(t.o_span);
        if seen.contains(&span) {
            continue;
        }
        seen.push(span);
        let mut d = Diagnostic {
            range: a.range(enc, span),
            severity: Some(DiagnosticSeverity::WARNING),
            code: Some(NumberOrString::String(LABEL_WHITESPACE.to_string())),
            source: Some(SOURCE.to_string()),
            message: message.to_string(),
            ..Default::default()
        };
        let json = a.parsed.syntax == Syntax::JsonLd;
        if let Some(s) = string
            && let Some(replace) = trimmed(&a.text[s.start as usize..s.end as usize], json)
        {
            FixData::Replace { replace }.attach(&mut d);
        }
        out.push(d);
    }
}

/// The span of the quoted string that gives the literal at `o_span` its
/// `value`: the literal's first token in the Turtle family, the JSON string
/// itself or a value object's string whose value is `value` in JSON-LD.
/// `None` for a malformed string or one not found.
fn string_token(a: &Analysis, o_span: Span, value: &str) -> Option<Span> {
    let text = a.text.get(o_span.start as usize..o_span.end as usize)?;
    let at = |s: Span| Span::new(o_span.start + s.start, o_span.start + s.end);
    if a.parsed.syntax != Syntax::JsonLd {
        let t = *lexer::tokenize(text).first()?;
        return (t.kind == Kind::String && t.error().is_none())
            .then(|| at(Span::new(t.start, t.end)));
    }
    if text.starts_with('"') {
        return Some(o_span);
    }
    // A value object: the string after a key at its top level that reads as
    // `value` (an `@value` key, or an alias of it).
    let tokens = json::tokens(text);
    let mut depth = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        match t.kind {
            json::TokenKind::LBrace | json::TokenKind::LBracket => depth += 1,
            json::TokenKind::RBrace | json::TokenKind::RBracket => depth = depth.saturating_sub(1),
            json::TokenKind::String
                if depth == 1
                    && i > 0
                    && tokens[i - 1].kind == json::TokenKind::Colon
                    && serde_json::from_str::<String>(
                        &text[t.span.start as usize..t.span.end as usize],
                    )
                    .is_ok_and(|s| s == value) =>
            {
                return Some(at(t.span));
            }
            _ => {}
        }
    }
    None
}

/// The string token `token` (quotes included) without the whitespace at
/// either end of its value, written raw or as an escape (`\t`, `\u0020`);
/// its quotes and other escapes kept. `json` for a JSON string, else a
/// Turtle one in any of its four quote styles. `None` when nothing changes.
fn trimmed(token: &str, json: bool) -> Option<String> {
    let q = token.chars().next().filter(|c| matches!(c, '"' | '\''))?;
    let long = !json && token.len() >= 6 && token[..3].chars().all(|c| c == q);
    let delim = if long { 3 } else { 1 };
    let content = token.get(delim..token.len().checked_sub(delim)?)?;
    // Each character or escape of the content: (start, end, whitespace).
    let mut units: Vec<(usize, usize, bool)> = Vec::new();
    let mut chars = content.char_indices();
    while let Some((i, c)) = chars.next() {
        if c != '\\' {
            units.push((i, i + c.len_utf8(), c.is_whitespace()));
            continue;
        }
        let Some((_, e)) = chars.next() else {
            units.push((i, i + 1, false));
            break;
        };
        let hex = match e {
            'u' => 4,
            'U' => 8,
            _ => 0,
        };
        let decoded = match e {
            't' => Some('\t'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            'f' => Some('\u{c}'),
            _ if hex > 0 => content
                .get(i + 2..i + 2 + hex)
                .filter(|h| h.bytes().all(|b| b.is_ascii_hexdigit()))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .and_then(char::from_u32),
            _ => None,
        };
        let mut end = i + 1 + e.len_utf8();
        if decoded.is_some() && hex > 0 {
            end += hex;
            chars.nth(hex - 1);
        }
        units.push((i, end, decoded.is_some_and(char::is_whitespace)));
    }
    let first = units.iter().position(|u| !u.2);
    let last = units.iter().rposition(|u| !u.2);
    let (kept, quote_last) = match (first, last) {
        (Some(f), Some(l)) => {
            let (s, e) = (units[f].0, units[l].1);
            (
                &content[s..e],
                &content[units[l].0..e] == q.encode_utf8(&mut [0; 4]),
            )
        }
        _ => ("", false),
    };
    if kept.len() == content.len() {
        return None;
    }
    let delim = &token[..delim];
    // A long string's content cannot end with a raw quote of its own kind,
    // which would close it early: escape that quote.
    if long && quote_last {
        let body = &kept[..kept.len() - 1];
        return Some(format!("{delim}{body}\\{q}{delim}"));
    }
    Some(format!("{delim}{kept}{delim}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::quickfix::Fixer;
    use std::sync::Arc;

    const PREFIXES: &str = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n@prefix skos: <http://www.w3.org/2004/02/skos/core#> .\n@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n@prefix ex: <http://example.org/> .\n";

    /// Each `label-whitespace` diagnostic of `text` in file `name`: the text
    /// it covers, its message, and the text after its quick fix (None
    /// without one).
    fn checked(name: &str, text: &str) -> Vec<(String, String, Option<String>)> {
        let a = Analysis::new(format!("file:///x/{name}"), text, Arc::default(), true);
        let k = Knowledge::default();
        let uri: lsp_types::Uri = a.uri.parse().unwrap();
        let fixer = Fixer::new(&a, &k, Encoding::Utf16, &uri);
        diagnostics(&a, &k, Encoding::Utf16, false)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String(LABEL_WHITESPACE.into())))
            .map(|d| {
                assert_eq!(d.severity, Some(DiagnosticSeverity::WARNING));
                let (s, e) = (
                    a.offset(Encoding::Utf16, d.range.start).unwrap() as usize,
                    a.offset(Encoding::Utf16, d.range.end).unwrap() as usize,
                );
                let fixed = fixer.for_diagnostic(&d).first().map(|c| {
                    assert_eq!(c.is_preferred, Some(true));
                    let edits = c.edit.clone().unwrap().changes.unwrap();
                    let edits: Vec<_> = edits.into_values().flatten().collect();
                    crate::features::refactor::apply(&a, Encoding::Utf16, &edits)
                });
                (text[s..e].to_string(), d.message, fixed)
            })
            .collect()
    }

    /// The one diagnostic of Turtle `objects` (the label after `ex:a`): the
    /// text it covers, and the line after its fix.
    fn turtle(objects: &str) -> (String, String) {
        let text = format!("{PREFIXES}ex:a {objects} .\n");
        let found = checked("a.ttl", &text);
        assert_eq!(found.len(), 1, "{found:?}");
        let (at, _, fixed) = found.into_iter().next().unwrap();
        let fixed = fixed.expect("a fix");
        let line = fixed.strip_prefix(PREFIXES).unwrap().to_string();
        (at, line)
    }

    #[test]
    fn leading_trailing_or_both() {
        let text = format!(
            "{PREFIXES}ex:a rdfs:label \" Pump\" .\nex:b rdfs:label \"Pump \" .\nex:c rdfs:label \"  Pump  \" .\n"
        );
        let found: Vec<(String, String)> = checked("a.ttl", &text)
            .into_iter()
            .map(|(at, m, _)| (at, m))
            .collect();
        let f = |a: &str, m: &str| (a.to_string(), m.to_string());
        assert_eq!(
            found,
            vec![
                f("\" Pump\"", "Label starts with whitespace"),
                f("\"Pump \"", "Label ends with whitespace"),
                f("\"  Pump  \"", "Label starts and ends with whitespace"),
            ]
        );
        assert_eq!(
            turtle("rdfs:label \"  Pump  \""),
            ("\"  Pump  \"".into(), "ex:a rdfs:label \"Pump\" .\n".into())
        );
    }

    #[test]
    fn tabs_and_newlines_raw_or_escaped() {
        let t = |o: &str| turtle(o).1;
        assert_eq!(
            t("rdfs:label \"\\tPump\\n\""),
            "ex:a rdfs:label \"Pump\" .\n"
        );
        assert_eq!(t("rdfs:label \"\tPump\""), "ex:a rdfs:label \"Pump\" .\n");
        assert_eq!(
            t("rdfs:label \"\\u0020Pump\\r\\f\\U00000009\""),
            "ex:a rdfs:label \"Pump\" .\n"
        );
        // Escapes inside the value stay; a backslash escape is not `\t`.
        assert_eq!(
            t("rdfs:label \" Pu\\tmp\\\\t\""),
            "ex:a rdfs:label \"Pu\\tmp\\\\t\" .\n"
        );
        assert_eq!(
            t("rdfs:label \"\"\"\n  Pump\n\"\"\""),
            "ex:a rdfs:label \"\"\"Pump\"\"\" .\n"
        );
    }

    #[test]
    fn every_quote_style_is_kept() {
        let t = |o: &str| turtle(o).1;
        assert_eq!(t("rdfs:label ' Pump '"), "ex:a rdfs:label 'Pump' .\n");
        assert_eq!(
            t("rdfs:label ''' Pump '''"),
            "ex:a rdfs:label '''Pump''' .\n"
        );
        assert_eq!(
            t("rdfs:label \"\"\" Pump \"\"\""),
            "ex:a rdfs:label \"\"\"Pump\"\"\" .\n"
        );
        // A long string cannot end with its own quote: the last one is escaped.
        assert_eq!(
            t("rdfs:label \"\"\"say \"hi\" \"\"\""),
            "ex:a rdfs:label \"\"\"say \"hi\\\"\"\"\" .\n"
        );
        assert_eq!(
            t("rdfs:label ''' it''s '''"),
            "ex:a rdfs:label '''it''s''' .\n"
        );
    }

    #[test]
    fn language_tag_and_datatype_are_kept() {
        assert_eq!(
            turtle("skos:prefLabel \" Pumpe\"@de"),
            (
                "\" Pumpe\"".into(),
                "ex:a skos:prefLabel \"Pumpe\"@de .\n".into()
            )
        );
        assert_eq!(
            turtle("skos:altLabel \"Pump \"^^xsd:string").1,
            "ex:a skos:altLabel \"Pump\"^^xsd:string .\n"
        );
    }

    #[test]
    fn a_label_of_only_whitespace_becomes_empty() {
        assert_eq!(turtle("rdfs:label \" \\t \"").1, "ex:a rdfs:label \"\" .\n");
        assert_eq!(
            turtle("rdfs:label \"\"\"  \"\"\"").1,
            "ex:a rdfs:label \"\"\"\"\"\" .\n"
        );
    }

    #[test]
    fn clean_labels_and_other_predicates_are_not_flagged() {
        let text = format!(
            "{PREFIXES}ex:a rdfs:label \"Pump\", \"Two words\", \"\" ;\n  rdfs:comment \" spaced \" ;\n  ex:label \" spaced \" ;\n  skos:prefLabel \"\"\"Pump\"\"\"@en .\n"
        );
        assert_eq!(checked("a.ttl", &text), vec![]);
    }

    #[test]
    fn found_in_trig_and_n_triples() {
        let trig = format!("{PREFIXES}ex:g {{ ex:a rdfs:label \" Pump\" }}\n");
        let found = checked("a.trig", &trig);
        assert_eq!(found.len(), 1);
        assert!(
            found[0]
                .2
                .as_ref()
                .unwrap()
                .contains("rdfs:label \"Pump\" }")
        );
        let nt = "<http://example.org/a> <http://www.w3.org/2000/01/rdf-schema#label> \"Pump\\t\"@en .\n";
        let found = checked("a.nt", nt);
        assert_eq!(
            found[0].2.as_deref(),
            Some(
                "<http://example.org/a> <http://www.w3.org/2000/01/rdf-schema#label> \"Pump\"@en .\n"
            )
        );
    }

    #[test]
    fn json_ld_string_values_are_trimmed() {
        let text = r#"{
  "@context": {
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "skos": "http://www.w3.org/2004/02/skos/core#",
    "value": "@value"
  },
  "@id": "http://example.org/a",
  "rdfs:label": " Pump\t",
  "skos:prefLabel": { "@language": "de", "@value": "Pumpe\n" },
  "skos:altLabel": [{ "value": " Kreisel", "@language": "de" }, "ok"]
}"#;
        let found = checked("a.jsonld", text);
        let at: Vec<&str> = found.iter().map(|f| f.0.as_str()).collect();
        assert_eq!(at, vec![r#"" Pump\t""#, r#""Pumpe\n""#, r#"" Kreisel""#]);
        assert!(
            found[0]
                .2
                .as_ref()
                .unwrap()
                .contains(r#""rdfs:label": "Pump","#)
        );
        assert!(
            found[1]
                .2
                .as_ref()
                .unwrap()
                .contains(r#""@value": "Pumpe" }"#)
        );
        assert!(
            found[2]
                .2
                .as_ref()
                .unwrap()
                .contains(r#""value": "Kreisel","#)
        );
    }
}
