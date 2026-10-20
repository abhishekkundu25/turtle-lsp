//! Workspace lint: references to terms in the workspace's own namespaces that
//! nothing describes (code `undescribed-term`), usually typos.
//!
//! A ref to IRI *x* is flagged when *x*'s namespace is an own namespace (see
//! [`Knowledge::is_own_namespace`]) and no source describes *x*. Refs in
//! subject position are descriptions themselves and never flagged. With a
//! described term in the same namespace within a small edit distance the
//! diagnostic is a warning with a replace fix; otherwise information.

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::{SOURCE, TermUse};
use crate::knowledge::{Knowledge, namespace_of};
use crate::syntax::{Form, Iri, IriKey, Role, Span};
use crate::vocab::levenshtein;
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::json;

pub const CODE: &str = "undescribed-term";

/// Whether `candidate` could be what the author meant by `local`: same case
/// of the first letter (`ex:Signal` the class and `ex:signal` the property are
/// different terms by convention), and not just different digits
/// (`ex:pump-12` is not a typo of `ex:pump-13`).
fn plausible(local: &str, candidate: &str) -> bool {
    let first_upper = |s: &str| s.chars().next().is_some_and(char::is_uppercase);
    let letters = |s: &str| -> String { s.chars().filter(|c| !c.is_ascii_digit()).collect() };
    first_upper(local) == first_upper(candidate) && letters(local) != letters(candidate)
}

/// The closest described term in `ns` to `local`: edit distance at most 2,
/// at most 1 for local names of 3 to 5 characters, and none for shorter ones
/// (`ex:p` is not a typo of `ex:s`); ties to the shorter, then smaller, name.
fn suggest(k: &Knowledge, ns: &str, local: &str) -> Option<Iri> {
    let max = match local.chars().count() {
        0..=2 => return None,
        3..=5 => 1,
        _ => 2,
    };
    let lower = local.to_lowercase();
    k.terms_in(ns)
        .into_iter()
        .filter(|t| k.is_described(t))
        .filter_map(|t| {
            let l = t.get(ns.len()..)?;
            if l.is_empty() || !plausible(local, l) {
                return None;
            }
            let d = levenshtein(&lower, &l.to_lowercase());
            (d <= max).then_some((d, l.len(), t))
        })
        .min_by(|x, y| (x.0, x.1, x.2.as_str()).cmp(&(y.0, y.1, y.2.as_str())))
        .map(|(_, _, t)| t)
}

/// A local name that can be written after `prefix:` without escapes.
fn plain_local(l: &str) -> bool {
    !l.is_empty()
        && !l.ends_with('.')
        && !l.starts_with(['-', '.'])
        && l.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// Predicates whose objects name whole ontologies (or versions of them),
/// which are resolved as imports, not described term by term.
const ONTOLOGY_LINKS: &[&str] = &[
    "imports",
    "versionIRI",
    "priorVersion",
    "backwardCompatibleWith",
    "incompatibleWith",
];

fn ontology_objects(a: &Analysis) -> FxHashSet<u32> {
    a.parsed
        .triples
        .iter()
        .filter(|t| {
            t.p.strip_prefix(OWL)
                .is_some_and(|l| ONTOLOGY_LINKS.contains(&l))
        })
        .map(|t| t.o_span.start)
        .collect()
}

const OWL: &str = "http://www.w3.org/2002/07/owl#";

pub fn undescribed_terms(a: &Analysis, k: &Knowledge, enc: Encoding) -> Vec<Diagnostic> {
    let mut ontologies: Option<FxHashSet<u32>> = None;
    let mut prefixes = None;
    let uses = a
        .parsed
        .refs
        .iter()
        .filter(|r| r.role != Role::Subject)
        .filter_map(|r| match r.form {
            Form::A => None,
            Form::PName { prefix_len } => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: a
                    .text
                    .get(r.span.start as usize..(r.span.start + prefix_len) as usize),
                bare: None,
            }),
            Form::Iri => Some(TermUse {
                span: r.span,
                iri: r.iri,
                prefix: None,
                bare: None,
            }),
        });
    undescribed(
        uses,
        &a.text,
        k,
        |span| {
            ontologies
                .get_or_insert_with(|| ontology_objects(a))
                .contains(&span.start)
        },
        |p| {
            prefixes
                .get_or_insert_with(|| a.prefix_map())
                .get(p)
                .cloned()
        },
        |span| a.range(enc, span),
    )
}

/// The `undescribed-term` diagnostics for `uses` (the document decides which
/// of its IRIs count: RDF documents leave out subjects, which describe).
/// `skip` leaves out a flagged use by its span, `namespace` resolves a
/// written prefix name, `range` maps a span. Shared by RDF and SPARQL
/// documents.
pub fn undescribed<'a, K>(
    uses: impl Iterator<Item = TermUse<'a, K>>,
    text: &str,
    k: &Knowledge,
    mut skip: impl FnMut(Span) -> bool,
    mut namespace: impl FnMut(&str) -> Option<String>,
    range: impl Fn(Span) -> Range,
) -> Vec<Diagnostic>
where
    K: std::ops::Deref<Target = str> + IriKey + Copy + Eq + std::hash::Hash,
{
    let mut out = Vec::new();
    // Per IRI: None = fine, Some(suggestion) = flagged.
    let mut verdicts: FxHashMap<K, Option<Option<Iri>>> = FxHashMap::default();
    for u in uses {
        let verdict = *verdicts.entry(u.iri).or_insert_with(|| {
            let ns = namespace_of(&u.iri);
            let flagged =
                ns.len() < u.iri.len() && !k.is_described(&u.iri) && k.is_own_namespace(ns);
            flagged.then(|| suggest(k, ns, &u.iri[ns.len()..]))
        });
        let Some(suggestion) = verdict else { continue };
        if skip(u.span) {
            continue;
        }
        let written = text
            .get(u.span.start as usize..u.span.end as usize)
            .unwrap_or_default();
        let mut d = Diagnostic {
            range: range(u.span),
            severity: Some(DiagnosticSeverity::INFORMATION),
            code: Some(NumberOrString::String(CODE.to_string())),
            source: Some(SOURCE.to_string()),
            message: format!("'{written}' is not described anywhere in the workspace"),
            ..Default::default()
        };
        if let Some(s) = suggestion {
            let pname = u.prefix.and_then(|prefix| {
                namespace(prefix)
                    .and_then(|ns| s.strip_prefix(ns.as_str()).map(str::to_string))
                    .filter(|l| plain_local(l))
                    .map(|l| format!("{prefix}:{l}"))
            });
            let replacement = pname.unwrap_or_else(|| format!("<{s}>"));
            d.severity = Some(DiagnosticSeverity::WARNING);
            d.message
                .push_str(&format!(". Did you mean '{replacement}'?"));
            d.data = Some(json!({ "replace": replacement }));
        }
        out.push(d);
    }
    out
}
