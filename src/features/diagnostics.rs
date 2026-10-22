//! The diagnostics of one RDF document. The quick fixes for them are in
//! [`super::quickfix`], which also owns the data a diagnostic carries for its
//! fix; [`crate::diagnose`] composes these with the other sources.

use super::quickfix::{FixData, PrefixCandidate, add_prefix_note, prefix_candidates};
use crate::analysis::{Analysis, Encoding};
use crate::jsonld;
use crate::knowledge::Knowledge;
use crate::syntax::{Form, Severity, Syntax};
use crate::vocab::VOCABS;
use crate::workspace::{PREFIX_FILE, file_uri_name};
use lsp_types::*;
use rustc_hash::FxHashMap;

pub(crate) const SOURCE: &str = "turtle";

/// Codes of diagnostics that claim something is known nowhere in the
/// workspace, so are only reported once the initial index is in.
pub(crate) const NEEDS_INDEX: &[&str] = crate::features::shacl_lint::NEEDS_INDEX;

pub(crate) fn needs_index(d: &Diagnostic) -> bool {
    matches!(&d.code, Some(NumberOrString::String(c))
        if NEEDS_INDEX.contains(&c.as_str()) || c == crate::features::lint::CODE)
}

/// One written occurrence of an IRI (never `a`), for the knowledge lints
/// RDF and SPARQL documents share. `K` is how the document holds IRIs: an
/// interned [`crate::syntax::Iri`], or a `&str`.
pub(crate) struct TermUse<'a, K> {
    pub(crate) span: crate::syntax::Span,
    pub(crate) iri: K,
    /// The prefix as written, for a prefixed name (`foaf` in `foaf:name`).
    pub(crate) prefix: Option<&'a str>,
    /// JSON-LD: the IRI is written in full in a string, as this text (an
    /// absolute IRI, or a word `@vocab` or the base completes), so a fix
    /// keeps that spelling, without angle brackets.
    pub(crate) bare: Option<&'a str>,
}

/// `unknown-term` (warning, with a "did you mean" and `data.replace` in the
/// document's own spelling) and `deprecated-term` (hint, deprecated tag) for
/// terms of the well-known vocabularies, matched by namespace IRI. `mk`
/// builds a diagnostic at a span.
pub(crate) fn vocabulary_terms<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    k: &Knowledge,
    mk: &impl Fn(crate::syntax::Span, DiagnosticSeverity, &str, String) -> Diagnostic,
    out: &mut Vec<Diagnostic>,
) where
    K: std::ops::Deref<Target = str> + crate::syntax::IriKey,
{
    // Suggestions are memoised: a file can use one unknown term thousands of times.
    let mut suggestions: FxHashMap<(&str, String), Option<&str>> = FxHashMap::default();
    for u in uses {
        let Some((v, local)) = VOCABS.lookup(&u.iri) else {
            continue;
        };
        if local.is_empty() {
            continue;
        }
        match v.term(local) {
            Some(t) if t.deprecated => {
                let mut d = mk(
                    u.span,
                    DiagnosticSeverity::HINT,
                    "deprecated-term",
                    format!("'{}:{local}' is deprecated", v.prefix),
                );
                d.tags = Some(vec![DiagnosticTag::DEPRECATED]);
                out.push(d);
            }
            Some(_) => {}
            // Described by the workspace or a resolved import: newer than the snapshot.
            None if k.is_described(&u.iri) => {}
            None => {
                let mut msg = format!(
                    "'{local}' is not defined in the {} vocabulary <{}>",
                    v.prefix, v.namespace
                );
                let mut d = mk(
                    u.span,
                    DiagnosticSeverity::WARNING,
                    "unknown-term",
                    String::new(),
                );
                let suggestion = *suggestions
                    .entry((v.namespace, local.to_string()))
                    .or_insert_with(|| v.suggest(local).map(|t| t.local.as_str()));
                if let Some(s) = suggestion {
                    msg.push_str(&format!(". Did you mean '{s}'?"));
                    let replacement = match (u.prefix, u.bare) {
                        (Some(p), _) => format!("{p}:{s}"),
                        (None, Some(written)) => bare_replacement(&u.iri, written, v.namespace, s),
                        (None, None) => format!("<{}{s}>", v.namespace),
                    };
                    FixData::Replace {
                        replace: replacement,
                    }
                    .attach(&mut d);
                }
                d.message = msg;
                out.push(d);
            }
        }
    }
}

/// What replaces `written`, a JSON-LD string that spells `iri` in full or
/// as its end (a `@vocab` word), so it spells `namespace` + `local`: the
/// same beginning stays implied, else the full IRI.
fn bare_replacement(iri: &str, written: &str, namespace: &str, local: &str) -> String {
    let target = format!("{namespace}{local}");
    iri.strip_suffix(written)
        .and_then(|implied| target.strip_prefix(implied))
        .filter(|r| !r.is_empty())
        .map_or_else(|| target.clone(), str::to_string)
}

/// Which optional diagnostics to compute (from the client's settings).
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub lint_undescribed_terms: bool,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            lint_undescribed_terms: true,
        }
    }
}

impl Options {
    pub(crate) fn from_settings(s: &crate::settings::Settings) -> Options {
        Options {
            lint_undescribed_terms: s.lint_undescribed_terms,
        }
    }
}

/// [`diagnostics_with`] with default options.
pub fn diagnostics(a: &Analysis, k: &Knowledge, enc: Encoding) -> Vec<Diagnostic> {
    diagnostics_with(a, k, enc, &Options::default())
}

pub fn diagnostics_with(
    a: &Analysis,
    k: &Knowledge,
    enc: Encoding,
    opts: &Options,
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let mk = |span, severity, code: &str, message: String| Diagnostic {
        range: a.range(enc, span),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some(SOURCE.to_string()),
        message,
        ..Default::default()
    };

    let severity = |s| match s {
        Severity::Error => DiagnosticSeverity::ERROR,
        Severity::Warning => DiagnosticSeverity::WARNING,
    };
    for e in &a.parsed.errors {
        out.push(mk(
            e.span,
            severity(e.severity),
            e.code,
            e.message().to_string(),
        ));
    }

    // Undeclared prefixes, with "add @prefix" fixes for every known namespace.
    let mut candidates: FxHashMap<&str, Vec<PrefixCandidate>> = FxHashMap::default();
    // N-Triples and N-Quads have no prefixes at all; the parser already
    // reports the pname.
    let has_prefixes = a.parsed.syntax.has_prefixes();
    for u in a
        .parsed
        .prefix_uses
        .iter()
        .filter(|u| !u.resolved && has_prefixes)
    {
        let shown = if u.name.is_empty() {
            ":".to_string()
        } else {
            format!("{}:", u.name)
        };
        let mut d = mk(
            u.span,
            DiagnosticSeverity::ERROR,
            "undeclared-prefix",
            format!("Undeclared prefix '{shown}'"),
        );
        let cands = candidates
            .entry(u.name.as_str())
            .or_insert_with(|| prefix_candidates(&u.name, k));
        add_prefix_note(&mut d, &u.name, cands, "@prefix");
        out.push(d);
    }

    // Duplicate and unused declarations.
    let mut seen: FxHashMap<&str, &str> = FxHashMap::default();
    for p in &a.parsed.prefixes {
        if let Some(prev) = seen.insert(&p.name, &p.iri) {
            if prev == p.iri {
                let mut d = mk(
                    p.span,
                    DiagnosticSeverity::HINT,
                    "duplicate-prefix",
                    format!("Prefix '{}:' is declared twice", p.name),
                );
                d.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
                out.push(d);
            } else {
                out.push(mk(
                    p.name_span,
                    DiagnosticSeverity::WARNING,
                    "redefined-prefix",
                    format!("Prefix '{}:' is redefined (was <{prev}>)", p.name),
                ));
            }
        }
    }
    let is_prefix_file = file_uri_name(&a.uri).as_deref() == Some(PREFIX_FILE);
    if !is_prefix_file {
        let used: rustc_hash::FxHashSet<&str> = a
            .parsed
            .prefix_uses
            .iter()
            .map(|u| u.name.as_str())
            .collect();
        for p in a
            .parsed
            .prefixes
            .iter()
            .filter(|p| !used.contains(p.name.as_str()))
        {
            let mut d = mk(
                p.span,
                DiagnosticSeverity::HINT,
                "unused-prefix",
                format!("Prefix '{}:' is never used", p.name),
            );
            d.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
            out.push(d);
        }
    }

    // Terms of well-known vocabularies (matched by namespace IRI, not prefix name).
    let text_at = |span: crate::syntax::Span, len: u32| {
        a.text.get(span.start as usize..(span.start + len) as usize)
    };
    if a.parsed.syntax == Syntax::JsonLd {
        for f in jsonld::lint::dropped(&a.text, &a.dropped) {
            out.push(mk(f.span, severity(f.severity), f.code, f.message));
        }
        // `rdfs:lable` with no `rdfs` in the context, for a prefix the
        // workspace knows: an IRI with scheme `rdfs:`.
        let mut namespaces: FxHashMap<&str, Option<String>> = FxHashMap::default();
        for (span, word) in jsonld::lint::undefined_prefixes(&a.text, &a.source_index) {
            let ns = namespaces.entry(word).or_insert_with(|| {
                prefix_candidates(word, k)
                    .into_iter()
                    .next()
                    .map(|c| c.namespace)
            });
            let Some(ns) = ns else { continue };
            let written = text_at(span, span.end - span.start).unwrap_or_default();
            let mut d = mk(
                span,
                DiagnosticSeverity::WARNING,
                jsonld::lint::UNDEFINED_PREFIX,
                format!(
                    "'{word}' is not defined in the @context, so '{written}' is an IRI with scheme '{word}:'"
                ),
            );
            FixData::UndefinedPrefix {
                prefix: word.to_string(),
                namespace: ns.clone(),
            }
            .attach(&mut d);
            out.push(d);
        }
        // A term of the context at its definition; anything else as written.
        let iris = jsonld::lint::vocabulary_iris(&a.text, &a.parsed.refs, &a.used_terms);
        let uses = iris.iter().map(|w| match w.form {
            Form::PName { prefix_len } => TermUse {
                span: w.span,
                iri: w.iri,
                prefix: text_at(w.span, prefix_len),
                bare: None,
            },
            _ => TermUse {
                span: w.span,
                iri: w.iri,
                prefix: None,
                bare: text_at(w.span, w.span.end - w.span.start),
            },
        });
        vocabulary_terms(uses, k, &mk, &mut out);
    } else {
        let uses = a.parsed.refs.iter().filter_map(|r| match r.form {
            Form::A => None,
            Form::PName { prefix_len } => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: text_at(r.span, prefix_len),
                bare: None,
            }),
            Form::Iri => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: None,
                bare: None,
            }),
        });
        vocabulary_terms(uses, k, &mk, &mut out);
    }

    for (span, message) in crate::features::literals::invalid_literals(a) {
        out.push(mk(
            span,
            DiagnosticSeverity::WARNING,
            crate::features::literals::CODE,
            message,
        ));
    }

    for f in crate::features::shacl_lint::lint(a, k) {
        let sev = if f.error {
            DiagnosticSeverity::ERROR
        } else {
            DiagnosticSeverity::WARNING
        };
        out.push(mk(f.span, sev, f.code, f.message));
    }
    if opts.lint_undescribed_terms {
        out.extend(crate::features::lint::undescribed_terms(a, k, enc));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Each diagnostic as (code, severity, the text it covers, data.replace).
    fn jsonld(text: &str) -> Vec<(String, DiagnosticSeverity, String, Option<String>)> {
        let a = Analysis::with_syntax(
            "file:///x/a.jsonld".to_string(),
            text,
            Arc::default(),
            false,
            Syntax::JsonLd,
        );
        diagnostics(&a, &Knowledge::default(), Encoding::Utf8)
            .into_iter()
            .map(|d| {
                let start = a.offset(Encoding::Utf8, d.range.start).unwrap();
                let end = a.offset(Encoding::Utf8, d.range.end).unwrap();
                let code = match d.code {
                    Some(NumberOrString::String(c)) => c,
                    _ => String::new(),
                };
                let replace = d
                    .data
                    .and_then(|x| x["replace"].as_str().map(str::to_string));
                (
                    code,
                    d.severity.unwrap(),
                    text[start as usize..end as usize].to_string(),
                    replace,
                )
            })
            .collect()
    }

    fn d(
        code: &str,
        severity: DiagnosticSeverity,
        at: &str,
        replace: Option<&str>,
    ) -> (String, DiagnosticSeverity, String, Option<String>) {
        (
            code.to_string(),
            severity,
            at.to_string(),
            replace.map(str::to_string),
        )
    }

    #[test]
    fn jsonld_lints_at_their_positions() {
        // LINTS of test/jsonld-fixtures.mjs.
        let text = r#"{
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
        use DiagnosticSeverity as S;
        let mut got = jsonld(text);
        got.sort_by_key(|x| text.find(&format!("\"{}\"", x.2)));
        assert_eq!(
            got,
            [
                // On the definition, not on "title" where it is used.
                d("unknown-term", S::WARNING, "rdfs:lable", Some("rdfs:label")),
                d("deprecated-term", S::HINT, "vcard:Email", None),
                d("jsonld-undefined-term", S::WARNING, "wieght", None),
                d("unknown-term", S::WARNING, "rdfs:labl", Some("rdfs:label")),
                d("invalid-iri", S::WARNING, "http://exa mple.org/x", None),
                d("invalid-language-tag", S::WARNING, "de_DE", None),
            ]
        );
    }

    #[test]
    fn jsonld_fixes_keep_the_spelling_written() {
        use DiagnosticSeverity as S;
        // A definition written as a compact IRI, an absolute IRI and a
        // @vocab word, each fixed in its own spelling.
        let text = r#"{
  "@context": {
    "@vocab": "http://www.w3.org/2000/01/rdf-schema#",
    "rdfs": "http://www.w3.org/2000/01/rdf-schema#",
    "a": "rdfs:labl",
    "b": {"@id": "http://www.w3.org/2000/01/rdf-schema#seeAlsoo"}
  },
  "@id": "http://ex.org/x",
  "a": "x",
  "b": "y",
  "a2": "z",
  "commnt": "w"
}"#;
        let mut got = jsonld(text);
        got.sort_by_key(|x| text.find(&x.2));
        assert_eq!(
            got,
            [
                d("unknown-term", S::WARNING, "rdfs:labl", Some("rdfs:label")),
                d(
                    "unknown-term",
                    S::WARNING,
                    "http://www.w3.org/2000/01/rdf-schema#seeAlsoo",
                    Some("http://www.w3.org/2000/01/rdf-schema#seeAlso")
                ),
                d("unknown-term", S::WARNING, "a2", None),
                d("unknown-term", S::WARNING, "commnt", Some("comment")),
            ]
        );
    }

    #[test]
    fn only_a_file_named_prefixes_ttl_may_leave_prefixes_unused() {
        let text = "@prefix a: <http://a/> .\n";
        let codes = |uri: &str| -> Vec<String> {
            let a = Analysis::new(uri.into(), text, Arc::default(), true);
            diagnostics(&a, &Knowledge::default(), Encoding::Utf8)
                .into_iter()
                .filter_map(|d| match d.code {
                    Some(NumberOrString::String(c)) => Some(c),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(codes("file:///t/myprefixes.ttl"), ["unused-prefix"]);
        assert_eq!(codes("file:///t/dir/prefixes.ttl"), Vec::<String>::new());
    }

    #[test]
    fn a_bare_replacement_keeps_what_is_implied() {
        let schema = "http://schema.org/";
        let r = |iri, written| bare_replacement(iri, written, schema, "Person");
        assert_eq!(r("http://schema.org/Persn", "Persn"), "Person");
        assert_eq!(
            r("http://schema.org/Persn", "http://schema.org/Persn"),
            "http://schema.org/Person"
        );
        // Written escaped, or under another namespace: the full IRI.
        assert_eq!(
            r("http://schema.org/Persn", "Pers\\u006E"),
            "http://schema.org/Person"
        );
        assert_eq!(
            bare_replacement("https://schema.org/Persn", "Persn", schema, "Person"),
            "http://schema.org/Person"
        );
    }
}
