//! A SPARQL document's diagnostics as the server publishes them: the
//! document's own checks ([`crate::sparql::Document::diagnostics`]: syntax,
//! prefixes, unbound projections), then the knowledge-based lints on
//! [`crate::sparql::Structure::refs`], with the Turtle logic and wording
//! (`features::diagnostics` and `features::lint`).

use crate::analysis::{Encoding, SparqlAnalysis};
use crate::features::diagnostics::{Options, SOURCE, TermUse, own_deprecations, vocabulary_terms};
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
/// - `unknown-term` and `deprecated-term` for vocabulary terms, and
///   `deprecated-term` for the workspace's own deprecated terms;
/// - `undescribed-term` when `opts.lint_undescribed_terms` is on.
pub(crate) fn diagnostics(
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
    // A query only uses terms: none of its refs describes one.
    let mut own = Vec::new();
    let warned = own_deprecations(uses(), text, k, &mk, &mut own);
    vocabulary_terms(uses(), k, &warned, &mk, &mut out);
    out.append(&mut own);

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Analysis;
    use crate::knowledge::Source;
    use std::sync::Arc;

    #[test]
    fn a_query_use_of_an_own_deprecated_term_is_a_warning() {
        let v = Analysis::new(
            "file:///w/vocab.ttl".into(),
            "@prefix ex: <http://ex.org/> .\n\
             ex:old <http://www.w3.org/2002/07/owl#deprecated> true .\n",
            Arc::default(),
            false,
        );
        let mut k = Knowledge::default();
        k.set("file:///w/vocab.ttl", Source::Disk, Arc::new(v));
        let text = "PREFIX ex: <http://ex.org/>\nSELECT * WHERE { ex:old ex:p ?o }\n";
        let q = SparqlAnalysis::new("file:///w/q.rq".into(), text, Arc::default());
        let got: Vec<_> = diagnostics(&q, &k, Encoding::Utf8, &Options::default())
            .into_iter()
            .filter(|d| d.code == Some(NumberOrString::String("deprecated-term".into())))
            .collect();
        // Subject position too: a query describes nothing.
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].severity, Some(DiagnosticSeverity::WARNING));
        assert_eq!(got[0].tags, Some(vec![DiagnosticTag::DEPRECATED]));
        assert_eq!(
            got[0].message,
            "'ex:old' is deprecated: the workspace marks it owl:deprecated"
        );
        assert_eq!(got[0].range.start, Position::new(1, 17));
    }
}
