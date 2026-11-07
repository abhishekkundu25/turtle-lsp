//! What a run shows in the documents: TurbOWL's findings (contradictions,
//! ill-typed literals) as diagnostics, the note that a run was skipped, and
//! the error of a run that failed.
//!
//! A finding is shown in each workspace statement that describes one of its
//! terms, on the statement's subject (a member of two disjoint classes: on
//! the member and on the classes); a finding about terms no workspace
//! statement describes (a literal) on the triples whose object is one of
//! them. A finding only about imports is not shown. Its code is
//! `turbowl/<rule>`, and its message is the finding's proof, as an indented
//! tree, or its sentence when the proof is not searched for.

use super::Base;
use crate::analysis::Analysis;
use crate::features::diagnostics::SOURCE;
use crate::knowledge::Source;
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};
use rustc_hash::{FxHashMap, FxHashSet};
use spargebra::term::{NamedOrBlankNode, Term};
use std::sync::Arc;

/// Diagnostics per document URI.
pub(crate) type ByUri = FxHashMap<String, Vec<Diagnostic>>;

/// The note on the open documents that a run was skipped for its size.
pub(crate) const SKIPPED_CODE: &str = "reasoning-skipped";
/// The code of a finding is this followed by its W3C rule.
pub(crate) const CODE_PREFIX: &str = "turbowl/";
/// Findings whose proofs are searched for; the rest show their sentence.
const MAX_EXPLAINED: usize = 100;
/// Findings shown per document; the rest are summarized in one diagnostic.
const MAX_PER_DOC: usize = 1000;

/// The diagnostics of `r`'s last run over `base`, and the explainer's
/// cache their proofs filled.
pub(super) fn of(r: &turbowl::Reasoner, base: &Base) -> (ByUri, turbowl::ExplainCache) {
    let mut out = ByUri::default();
    let found = r.diagnostics();
    if found.is_empty() {
        return (out, turbowl::ExplainCache::default());
    }
    let mentions = Mentions::of(base, found);
    let explainer = r.explainer().ok();
    let mut explained = 0;
    for d in found {
        let places = mentions.places(base, d);
        if places.is_empty() {
            continue;
        }
        let proof = explainer
            .as_ref()
            .filter(|_| explained < MAX_EXPLAINED)
            .and_then(|e| e.explain_diagnostic(d));
        explained += 1;
        let message = proof.map_or_else(|| d.message.clone(), |p| p.to_string().trim_end().into());
        let severity = match d.severity {
            turbowl::Severity::Error => DiagnosticSeverity::ERROR,
            turbowl::Severity::Warning => DiagnosticSeverity::WARNING,
        };
        for (uri, range) in places {
            out.entry(uri.to_string()).or_default().push(Diagnostic {
                range,
                severity: Some(severity),
                code: Some(NumberOrString::String(format!("{CODE_PREFIX}{}", d.code))),
                source: Some(SOURCE.into()),
                message: message.clone(),
                ..Default::default()
            });
        }
    }
    for list in out.values_mut() {
        if list.len() > MAX_PER_DOC {
            let hidden = list.len() - MAX_PER_DOC;
            list.truncate(MAX_PER_DOC);
            list.push(Diagnostic {
                severity: Some(DiagnosticSeverity::INFORMATION),
                code: Some(NumberOrString::String(format!("{CODE_PREFIX}more"))),
                source: Some(SOURCE.into()),
                message: format!("{hidden} more reasoning findings in this file are not shown"),
                ..Default::default()
            });
        }
    }
    let cache = explainer.map(|e| e.into_cache()).unwrap_or_default();
    (out, cache)
}

/// Where the workspace writes the findings' terms: the base triples with
/// each as subject and as object, by (document, index), in order.
struct Mentions {
    subjects: FxHashMap<NamedOrBlankNode, Vec<(u32, u32)>>,
    objects: FxHashMap<Term, Vec<(u32, u32)>>,
}

fn as_subject(t: &Term) -> Option<NamedOrBlankNode> {
    match t {
        Term::NamedNode(n) => Some(n.clone().into()),
        Term::BlankNode(b) => Some(b.clone().into()),
        _ => None,
    }
}

impl Mentions {
    /// One pass over the workspace's triples.
    fn of(base: &Base, found: &[turbowl::Diagnostic]) -> Mentions {
        let mut subjects = FxHashMap::default();
        let mut objects = FxHashMap::default();
        for t in found.iter().flat_map(|d| &d.terms) {
            if let Some(s) = as_subject(t) {
                subjects.insert(s, Vec::new());
            }
            objects.insert(t.clone(), Vec::new());
        }
        for (t, place) in base.in_workspace() {
            if let Some(v) = subjects.get_mut(&t.subject) {
                v.push(place);
            }
            if let Some(v) = objects.get_mut(&t.object) {
                v.push(place);
            }
        }
        for v in subjects.values_mut().chain(objects.values_mut()) {
            v.sort_unstable();
        }
        Mentions { subjects, objects }
    }

    /// The (document URI, range) where `d` is shown, each once, in order.
    fn places<'b>(&self, base: &'b Base, d: &turbowl::Diagnostic) -> Vec<(&'b str, Range)> {
        let enc = base.encoding();
        let mut out = Vec::new();
        let mut statements = FxHashSet::default();
        for place in d
            .terms
            .iter()
            .filter_map(as_subject)
            .filter_map(|s| self.subjects.get(&s))
            .flatten()
        {
            let (a, t) = base.written(*place);
            if !statements.insert((place.0, t.statement)) {
                continue;
            }
            let range = match a.parsed.statements[t.statement as usize].subject {
                Some((_, span)) => a.range(enc, span),
                // A blank node subject: the triple.
                None => base.place(*place).range,
            };
            out.push((a.uri.as_str(), range));
        }
        if out.is_empty() {
            for place in d.terms.iter().filter_map(|t| self.objects.get(t)).flatten() {
                let (a, t) = base.written(*place);
                out.push((a.uri.as_str(), a.range(enc, t.o_span)));
            }
        }
        let mut seen = FxHashSet::default();
        out.retain(|p| seen.insert(*p));
        out
    }
}

/// The note on each open document that the run was skipped: the sources
/// have `total` triples, more than `max`.
pub(super) fn skipped(docs: &[(Source, Arc<Analysis>)], total: u64, max: u64) -> ByUri {
    on_open(docs, || Diagnostic {
        severity: Some(DiagnosticSeverity::INFORMATION),
        code: Some(NumberOrString::String(SKIPPED_CODE.into())),
        source: Some(SOURCE.into()),
        message: format!(
            "Reasoning skipped: the workspace has {total} triples, more than the limit of {max} (setting turtle.reasoning.maxTriples)"
        ),
        ..Default::default()
    })
}

/// The error on each open document that a run panicked.
pub(super) fn crashed(
    docs: &[(Source, Arc<Analysis>)],
    panic: &(dyn std::any::Any + Send),
) -> ByUri {
    let detail = panic
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_default();
    on_open(docs, || Diagnostic {
        severity: Some(DiagnosticSeverity::ERROR),
        code: Some(NumberOrString::String(format!("{CODE_PREFIX}internal"))),
        source: Some(SOURCE.into()),
        message: format!(
            "turtle-lsp internal error in reasoning: {detail}. Results are not shown; please report this at https://github.com/abhishekkundu25/turtle-lsp/issues"
        ),
        ..Default::default()
    })
}

/// `d` at the start of each open document.
fn on_open(docs: &[(Source, Arc<Analysis>)], d: impl Fn() -> Diagnostic) -> ByUri {
    docs.iter()
        .filter(|(s, _)| *s == Source::Open)
        .map(|(_, a)| (a.uri.clone(), vec![d()]))
        .collect()
}
