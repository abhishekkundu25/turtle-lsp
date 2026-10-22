//! A SPARQL document's diagnostics as the server publishes them: the
//! document's own checks ([`crate::sparql::Document::diagnostics`]: syntax,
//! prefixes, unbound projections), then the knowledge-based lints on
//! [`crate::sparql::Structure::refs`], with the Turtle logic and wording
//! (`features::diagnostics` and `features::lint`).

use crate::analysis::{Encoding, SparqlAnalysis};
use crate::features::diagnostics::{Options, SOURCE, TermUse, vocabulary_terms};
use crate::features::lint;
use crate::features::quickfix::{PrefixCandidate, add_prefix_note, prefix_candidates};
use crate::knowledge::Knowledge;
use crate::sparql::{Form, Level};
use crate::syntax::Span;
use lsp_types::*;
use rustc_hash::FxHashMap;

/// Every diagnostic of `q`:
/// - the document's own, with their codes and levels; `unused-prefix` and
///   `duplicate-prefix` tagged unnecessary; `undeclared-prefix` extended as
///   in Turtle with the namespace to add, written `(add PREFIX foaf: <ns>)`,
///   and the candidates in `data` for the quick fixes;
/// - `unknown-term` and `deprecated-term` for vocabulary terms;
/// - `undescribed-term` when `opts.lint_undescribed_terms` is on.
pub fn diagnostics(
    q: &SparqlAnalysis,
    k: &Knowledge,
    enc: Encoding,
    opts: &Options,
) -> Vec<Diagnostic> {
    let text = q.text();
    let mk = |span: Span, severity, code: &str, message: String| Diagnostic {
        range: q.range(enc, span),
        severity: Some(severity),
        code: Some(NumberOrString::String(code.to_string())),
        source: Some(SOURCE.to_string()),
        message,
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut candidates: FxHashMap<&str, Vec<PrefixCandidate>> = FxHashMap::default();
    for d in q.doc.diagnostics() {
        let severity = match d.level {
            Level::Error => DiagnosticSeverity::ERROR,
            Level::Warning => DiagnosticSeverity::WARNING,
            Level::Hint => DiagnosticSeverity::HINT,
        };
        let mut lsp = mk(d.span, severity, d.code, d.message.clone());
        match d.code {
            "unused-prefix" | "duplicate-prefix" => {
                lsp.tags = Some(vec![DiagnosticTag::UNNECESSARY]);
            }
            "undeclared-prefix" => {
                // Reported on the prefixed name: its prefix is up to the colon.
                let written = text
                    .get(d.span.start as usize..d.span.end as usize)
                    .unwrap_or_default();
                let name = written.split_once(':').map_or(written, |(p, _)| p);
                let cands = candidates
                    .entry(name)
                    .or_insert_with(|| prefix_candidates(name, k));
                add_prefix_note(&mut lsp, name, cands, "PREFIX");
            }
            _ => {}
        }
        out.push(lsp);
    }

    let refs = &q.doc.structure().refs;
    let uses = || {
        refs.iter().filter_map(|r| {
            let iri = r.iri.as_deref()?;
            let prefix = match r.form {
                Form::A => return None,
                Form::PName { prefix_len } => {
                    text.get(r.span.start as usize..(r.span.start + prefix_len) as usize)
                }
                Form::Iri => None,
            };
            Some(TermUse {
                span: r.span,
                iri,
                prefix,
                bare: None,
            })
        })
    };
    vocabulary_terms(uses(), k, &mk, &mut out);

    if opts.lint_undescribed_terms {
        let st = q.doc.structure();
        // The last PREFIX line for the name wins, else the external binding.
        let namespace = |name: &str| {
            st.prefixes
                .iter()
                .rev()
                .find(|p| p.name == name)
                .map(|p| p.resolved.clone().unwrap_or_else(|| p.iri.clone()))
                .or_else(|| q.external.get(name).cloned())
        };
        out.extend(lint::undescribed(
            uses(),
            text,
            k,
            |_| false,
            namespace,
            |span| q.range(enc, span),
        ));
    }
    out
}
