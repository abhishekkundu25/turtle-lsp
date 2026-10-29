//! The diagnostics of one RDF document. The quick fixes for them are in
//! [`super::quickfix`], which also owns the data a diagnostic carries for its
//! fix; [`crate::diagnose`] composes these with the other sources.

use super::quickfix::{FixData, PrefixCandidate, add_prefix_note, prefix_candidates};
use crate::analysis::{Analysis, Encoding};
use crate::jsonld;
use crate::knowledge::Knowledge;
use crate::syntax::lexer::Kind;
use crate::syntax::{Form, Role, Severity, Syntax, SyntaxError};
use crate::vocab::VOCABS;
use crate::workspace::{PREFIX_FILE, file_uri_name};
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};

pub(crate) const SOURCE: &str = "turtle";

/// Codes of diagnostics that claim something is known nowhere in the
/// workspace, so are only reported once the initial index is in.
pub(crate) const NEEDS_INDEX: &[&str] = crate::features::shacl_lint::NEEDS_INDEX;

/// The curation checks that say a term or ontology lacks something any
/// workspace document could give it: they wait for the index too.
const CURATION_NEEDS_INDEX: &[&str] = &[
    crate::features::curation::MISSING_LABEL,
    crate::features::curation::MISSING_DEFINITION,
    crate::features::curation::MISSING_ONTOLOGY_TITLE,
    crate::features::curation::MISSING_ONTOLOGY_DESCRIPTION,
    crate::features::curation::MISSING_ONTOLOGY_LICENSE,
];

pub(crate) fn needs_index(d: &Diagnostic) -> bool {
    matches!(&d.code, Some(NumberOrString::String(c))
        if NEEDS_INDEX.contains(&c.as_str())
            || CURATION_NEEDS_INDEX.contains(&c.as_str())
            || c == crate::features::lint::CODE)
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
/// builds a diagnostic at a span. No hint where `warned`, the spans
/// [`own_deprecations`] reported, has the use's span: that warning says it.
pub(crate) fn vocabulary_terms<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    k: &Knowledge,
    warned: &FxHashSet<crate::syntax::Span>,
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
            // A workspace deprecation too: [`own_deprecations`] warns instead.
            Some(t) if t.deprecated && warned.contains(&u.span) => {}
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

/// `deprecated-term` (warning, deprecated tag) for each use of a term that
/// a workspace file marks `owl:deprecated true`
/// ([`Knowledge::deprecated_here`]), named as written in `text`. The caller
/// leaves out subjects: they describe the term. Needs no index wait: an
/// unindexed file can only hide a deprecation, never invent one. Returns
/// the spans it reported, for [`vocabulary_terms`].
pub(crate) fn own_deprecations<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    text: &str,
    k: &Knowledge,
    mk: &impl Fn(crate::syntax::Span, DiagnosticSeverity, &str, String) -> Diagnostic,
    out: &mut Vec<Diagnostic>,
) -> FxHashSet<crate::syntax::Span>
where
    K: crate::syntax::IriKey,
{
    let mut warned = FxHashSet::default();
    for u in uses.filter(|u| k.deprecated_here(&u.iri)) {
        warned.insert(u.span);
        let written = text
            .get(u.span.start as usize..u.span.end as usize)
            .unwrap_or_default();
        let mut d = mk(
            u.span,
            DiagnosticSeverity::WARNING,
            "deprecated-term",
            format!("'{written}' is deprecated: the workspace marks it owl:deprecated"),
        );
        d.tags = Some(vec![DiagnosticTag::DEPRECATED]);
        out.push(d);
    }
    warned
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

/// How many syntax errors and warnings of a Turtle, TriG, N-Triples or
/// N-Quads document the editor shows. JSON-LD has its own cap
/// ([`crate::jsonld::json::MAX_ERRORS`]).
pub(crate) const MAX_SYNTAX_ERRORS: usize = 1000;

/// Which optional diagnostics to compute (from the client's settings).
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub lint_undescribed_terms: bool,
    /// Some off-by-default curation check is enabled
    /// ([`crate::features::curation::OPT_IN`]); without one they are not
    /// computed. Which codes are reported is decided by the composer.
    pub house_style: bool,
    /// At most this many syntax errors and warnings of a Turtle-family
    /// document: past it, the ones that start first in the text (the first
    /// found, on a tie), in the order found, and one error that says there
    /// are more, empty and at the end of the last one shown. None shows them
    /// all, as `turtle-lsp check` does. The editor's default is 1,000
    /// (`MAX_SYNTAX_ERRORS`).
    pub max_syntax_errors: Option<usize>,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            lint_undescribed_terms: true,
            house_style: false,
            max_syntax_errors: Some(MAX_SYNTAX_ERRORS),
        }
    }
}

impl Options {
    pub(crate) fn from_settings(s: &crate::settings::Settings) -> Options {
        Options {
            lint_undescribed_terms: s.lint_on(crate::features::lint::CODE),
            house_style: crate::features::curation::OPT_IN
                .iter()
                .any(|c| s.lint_on(c)),
            ..Options::default()
        }
    }
}

/// Which of `errors` to show when there are more than `max`: the `max` that
/// start first in the text, the first found on a tie.
fn first_in_text(errors: &[SyntaxError], max: usize) -> Vec<bool> {
    let mut order: Vec<usize> = (0..errors.len()).collect();
    let mut shown = vec![false; errors.len()];
    if max < errors.len() {
        order.select_nth_unstable_by_key(max, |&i| (errors[i].span.start, i));
        order.truncate(max);
    }
    for i in order {
        shown[i] = true;
    }
    shown
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
    let errors = &a.parsed.errors;
    let cap = opts
        .max_syntax_errors
        .filter(|&max| errors.len() > max && a.parsed.syntax != Syntax::JsonLd);
    let shown = cap.map(|max| first_in_text(errors, max));
    let mut last_end = 0;
    for (i, e) in errors.iter().enumerate() {
        if shown.as_ref().is_some_and(|s| !s[i]) {
            continue;
        }
        last_end = last_end.max(e.span.end);
        out.push(mk(
            e.span,
            severity(e.severity),
            e.code,
            e.message().to_string(),
        ));
    }
    if let Some(max) = cap {
        out.push(mk(
            crate::syntax::Span::new(last_end, last_end),
            DiagnosticSeverity::ERROR,
            "syntax",
            format!("More than {max} syntax errors; the rest are not shown"),
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
        let mut used: rustc_hash::FxHashSet<&str> = a
            .parsed
            .prefix_uses
            .iter()
            .map(|u| u.name.as_str())
            .collect();
        // A statement skipped by error recovery records no uses; its prefixed
        // names still keep their prefix. Without a syntax error nothing is
        // skipped, so the recorded uses are all of them.
        if a.parsed
            .errors
            .iter()
            .any(|e| e.severity == Severity::Error)
            && a.parsed
                .prefixes
                .iter()
                .any(|p| !used.contains(p.name.as_str()))
        {
            let tokens = a.tokens_or_lexed();
            let names: rustc_hash::FxHashSet<u32> = a
                .parsed
                .prefixes
                .iter()
                .map(|p| p.name_span.start)
                .collect();
            used.extend(
                tokens
                    .iter()
                    .filter(|t| t.kind == Kind::PName && !names.contains(&t.start))
                    .filter_map(|t| a.text[t.start as usize..t.end as usize].split(':').next()),
            );
        }
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

    // Uses of terms the workspace deprecates, added after the vocabulary
    // checks, which leave out the hint where these warn.
    let uses = a.parsed.refs.iter().filter_map(|r| {
        (r.role != Role::Subject && !matches!(r.form, Form::A)).then_some(TermUse {
            span: r.span,
            iri: r.iri,
            prefix: None,
            bare: None,
        })
    });
    let mut own = Vec::new();
    let warned = own_deprecations(uses, &a.text, k, &mk, &mut own);

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
        vocabulary_terms(uses, k, &warned, &mk, &mut out);
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
        vocabulary_terms(uses, k, &warned, &mk, &mut out);
    }
    out.append(&mut own);

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
    out.extend(crate::features::curation::diagnostics(
        a,
        k,
        enc,
        opts.house_style,
    ));
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
    fn a_prefix_used_in_a_skipped_statement_is_used() {
        // Reduced from a chaos input: `check --fix` deleted `dct:`.
        let text = "@prefix dct: <http://purl.org/dc/terms/> .\n.ex:a) dct:title \"x\" .\n";
        for keep in [true, false] {
            let a = Analysis::new("file:///t/a.ttl".into(), text, Arc::default(), keep);
            let d = diagnostics(&a, &Knowledge::default(), Encoding::Utf8);
            assert!(
                !d.iter()
                    .any(|d| d.code == Some(NumberOrString::String("unused-prefix".into()))),
                "{d:?}"
            );
        }
    }

    /// `n` lines, each a statement that parses and one that does not.
    fn broken_lines(n: usize) -> String {
        (0..n)
            .map(|i| format!("<http://e/s{i}> <http://e/p> <http://e/o{i}> . <http://e/t{i}> <http://e/p> .\n"))
            .collect()
    }

    /// The syntax diagnostics of `text` as a Turtle file, showing at most `max`.
    fn syntax_diagnostics(text: &str, max: Option<usize>) -> (Arc<Analysis>, Vec<Diagnostic>) {
        let a = Arc::new(Analysis::new(
            "file:///x/a.ttl".into(),
            text,
            Arc::default(),
            false,
        ));
        let opts = Options {
            max_syntax_errors: max,
            ..Options::default()
        };
        let d = diagnostics_with(&a, &Knowledge::default(), Encoding::Utf8, &opts)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String("syntax".into())))
            .collect();
        (a, d)
    }

    #[test]
    fn the_editor_shows_the_first_syntax_errors_and_one_note() {
        let text = broken_lines(1500);
        let (a, d) = syntax_diagnostics(&text, Options::default().max_syntax_errors);
        assert_eq!(a.parsed.errors.len(), 1500);
        assert_eq!(d.len(), MAX_SYNTAX_ERRORS + 1);
        for (i, e) in d[..MAX_SYNTAX_ERRORS].iter().enumerate() {
            assert_eq!(e.range.start.line, i as u32, "{e:?}");
            assert_eq!(e.message, "Expected an object, found '.'");
        }
        let note = &d[MAX_SYNTAX_ERRORS];
        assert_eq!(
            note.message,
            "More than 1000 syntax errors; the rest are not shown"
        );
        assert_eq!(note.severity, Some(DiagnosticSeverity::ERROR));
        let end = d[MAX_SYNTAX_ERRORS - 1].range.end;
        assert_eq!(note.range, Range::new(end, end));
    }

    #[test]
    fn up_to_the_cap_every_syntax_error_is_shown() {
        let max = Some(MAX_SYNTAX_ERRORS);
        let (_, d) = syntax_diagnostics(&broken_lines(MAX_SYNTAX_ERRORS), max);
        assert_eq!(d.len(), MAX_SYNTAX_ERRORS);
        assert!(d.iter().all(|e| e.message.starts_with("Expected")));
        let (_, d) = syntax_diagnostics(&broken_lines(MAX_SYNTAX_ERRORS + 1), max);
        assert_eq!(d.len(), MAX_SYNTAX_ERRORS + 1);
        assert!(d[MAX_SYNTAX_ERRORS].message.starts_with("More than 1000"));
    }

    #[test]
    fn without_a_cap_every_syntax_error_is_shown() {
        let (_, d) = syntax_diagnostics(&broken_lines(1500), None);
        assert_eq!(d.len(), 1500);
        assert!(d.iter().all(|e| !e.message.starts_with("More than")));
    }

    #[test]
    fn the_cap_keeps_the_errors_that_come_first_in_the_text() {
        // The lexer's errors are found before the parser's; here they come
        // last in the text, so the parser's are the ones shown.
        let mut text = broken_lines(30);
        for i in 0..30 {
            text.push_str(&format!("<http://e/u{i}> <http://e/p> \"open\n"));
        }
        let (_, d) = syntax_diagnostics(&text, Some(10));
        assert_eq!(d.len(), 11);
        let lines: Vec<u32> = d[..10].iter().map(|e| e.range.start.line).collect();
        assert_eq!(lines, (0..10).collect::<Vec<u32>>());
        assert!(
            d[..10]
                .iter()
                .all(|e| e.message == "Expected an object, found '.'")
        );
        assert_eq!(
            d[10].message,
            "More than 10 syntax errors; the rest are not shown"
        );
        // On a tie the first found: line 30's lexer error, not the parser's.
        let mut tied = broken_lines(30);
        tied.push_str("<http://e/u> <http://e/p> \"open\n");
        let (_, d) = syntax_diagnostics(&tied, Some(31));
        assert_eq!(d.len(), 32);
        let on_30: Vec<&str> = d[..31]
            .iter()
            .filter(|e| e.range.start.line == 30)
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(on_30, ["Unterminated string"]);
    }

    #[test]
    fn json_ld_keeps_its_own_cap() {
        let text = format!("[{}]", "1 ".repeat(50));
        let shown = |max| {
            let a = Analysis::with_syntax(
                "file:///x/a.jsonld".to_string(),
                &text,
                Arc::default(),
                false,
                Syntax::JsonLd,
            );
            let opts = Options {
                max_syntax_errors: max,
                ..Options::default()
            };
            diagnostics_with(&a, &Knowledge::default(), Encoding::Utf8, &opts)
        };
        let all = shown(None);
        assert!(all.len() > 5, "{all:?}");
        assert_eq!(shown(Some(5)), all);
    }

    /// The `deprecated-term` diagnostics of `uses` (a document of `syntax`)
    /// as (severity, text covered, message), with `vocab.ttl` beside it.
    fn own_deprecated(
        vocab: &str,
        uses: &str,
        syntax: Syntax,
    ) -> Vec<(DiagnosticSeverity, String, String)> {
        use crate::knowledge::Source;
        let header = "@prefix ex: <http://ex.org/> .\n@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
                      @prefix xsd: <http://www.w3.org/2001/XMLSchema#> .\n\
                      @prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n";
        let v = Analysis::new(
            "file:///w/vocab.ttl".into(),
            &format!("{header}{vocab}"),
            Arc::default(),
            false,
        );
        let a = Arc::new(Analysis::with_syntax(
            "file:///w/uses".into(),
            uses,
            Arc::default(),
            false,
            syntax,
        ));
        let mut k = Knowledge::default();
        k.set("file:///w/vocab.ttl", Source::Disk, Arc::new(v));
        k.set("file:///w/uses", Source::Open, a.clone());
        diagnostics(&a, &k, Encoding::Utf8)
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String("deprecated-term".into())))
            .map(|d| {
                assert_eq!(d.tags, Some(vec![DiagnosticTag::DEPRECATED]));
                let start = a.offset(Encoding::Utf8, d.range.start).unwrap() as usize;
                let end = a.offset(Encoding::Utf8, d.range.end).unwrap() as usize;
                (d.severity.unwrap(), uses[start..end].to_string(), d.message)
            })
            .collect()
    }

    #[test]
    fn a_use_of_an_own_deprecated_term_is_a_warning() {
        use DiagnosticSeverity as S;
        let uses = "@prefix ex: <http://ex.org/> .\n\
                    ex:old ex:p ex:x .\nex:a ex:old <http://ex.org/old> .\n";
        let got = own_deprecated("ex:old owl:deprecated true .\n", uses, Syntax::Turtle);
        // Not at the subject: that describes the term.
        assert_eq!(
            got,
            [
                (
                    S::WARNING,
                    "ex:old".into(),
                    "'ex:old' is deprecated: the workspace marks it owl:deprecated".into()
                ),
                (
                    S::WARNING,
                    "<http://ex.org/old>".into(),
                    "'<http://ex.org/old>' is deprecated: the workspace marks it owl:deprecated"
                        .into()
                ),
            ]
        );
        // The typed form counts; `false` anywhere clears it.
        let typed = "ex:old owl:deprecated \"true\"^^xsd:boolean .\n";
        assert_eq!(own_deprecated(typed, uses, Syntax::Turtle).len(), 2);
        let undone = "ex:old owl:deprecated true .\nex:old owl:deprecated false .\n";
        assert_eq!(own_deprecated(undone, uses, Syntax::Turtle), []);
        // The deprecating file's own description is not flagged.
        let v = "ex:old owl:deprecated true .\n";
        assert_eq!(own_deprecated(v, "", Syntax::Turtle), []);
    }

    #[test]
    fn a_vocabulary_deprecation_stays_a_hint_unless_the_workspace_says_so() {
        use DiagnosticSeverity as S;
        let uses = "@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n\
                    <http://ex.org/a> a vcard:Email .\n";
        assert_eq!(
            own_deprecated("", uses, Syntax::Turtle),
            [(
                S::HINT,
                "vcard:Email".into(),
                "'vcard:Email' is deprecated".into()
            )]
        );
        // Deprecated by both: reported once, as the workspace's warning.
        let got = own_deprecated("vcard:Email owl:deprecated true .\n", uses, Syntax::Turtle);
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].0, S::WARNING);
        // A subject gets no warning (it describes the term): it keeps the hint.
        let uses = "@prefix vcard: <http://www.w3.org/2006/vcard/ns#> .\n\
                    @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
                    vcard:Email rdfs:comment \"x\" .\n";
        let got = own_deprecated("vcard:Email owl:deprecated true .\n", uses, Syntax::Turtle);
        assert_eq!(
            got,
            [(
                S::HINT,
                "vcard:Email".into(),
                "'vcard:Email' is deprecated".into()
            )]
        );
    }

    #[test]
    fn a_jsonld_use_of_an_own_deprecated_term_is_a_warning() {
        let uses = r#"{
  "@context": {"ex": "http://ex.org/", "old": {"@id": "ex:old", "@type": "@id"}},
  "@id": "ex:a",
  "old": "ex:b",
  "ex:p": {"@id": "ex:old"}
}"#;
        let got = own_deprecated("ex:old owl:deprecated true .\n", uses, Syntax::JsonLd);
        let at: Vec<&str> = got.iter().map(|g| g.1.as_str()).collect();
        assert!(
            got.iter().all(|g| g.0 == DiagnosticSeverity::WARNING),
            "{got:?}"
        );
        // The term at its use, and the compact IRI; not the subject `ex:a`.
        assert_eq!(at, ["old", "ex:old"], "{got:?}");
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
