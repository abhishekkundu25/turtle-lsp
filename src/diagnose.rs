//! The diagnostics composer: what an editor shows for one document, built in
//! one place. The server for an open document, workspace diagnostics for a
//! closed file, and [`crate::batch`] (the command line's `check` and the
//! browser) all call [`document`], so they cannot disagree about which
//! diagnostics a document has or in which order they come: the document's
//! own, then the unresolved imports, then the SHACL results.

use crate::analysis::{Analysis, Encoding, SparqlAnalysis};
use crate::features::diagnostics::{self, needs_index};
use crate::imports::{self, Status};
use crate::knowledge::Knowledge;
use crate::settings::Settings;
use crate::sparql::features as sparql_features;
use lsp_types::Diagnostic;

/// A document to diagnose.
#[derive(Clone, Copy)]
pub(crate) enum Doc<'a> {
    Rdf(&'a Analysis),
    Sparql(&'a SparqlAnalysis),
}

/// What the diagnostics of a document depend on besides the document and the
/// knowledge.
pub(crate) struct DiagnosticInputs<'a> {
    pub(crate) settings: &'a Settings,
    /// The status of each `owl:imports` target by IRI; an import without one
    /// is not reported.
    pub(crate) import_status: &'a dyn Fn(&str) -> Option<&'a Status>,
    /// The SHACL results for the document, which come last.
    pub(crate) shacl: &'a [Diagnostic],
    /// Whether the workspace is indexed. Until it is, findings that say a
    /// term is known nowhere are left out ([`needs_index`]).
    pub(crate) indexed: bool,
    /// The document is a cached copy of an import: read-only, nothing to fix
    /// there, so it has no diagnostics.
    pub(crate) cached_import: bool,
    /// How many syntax errors of a Turtle-family document to show
    /// ([`diagnostics::Options::max_syntax_errors`]): the editor's
    /// [`diagnostics::MAX_SYNTAX_ERRORS`], or None (all) for the command line.
    pub(crate) max_syntax_errors: Option<usize>,
}

impl<'a> DiagnosticInputs<'a> {
    /// A workspace that is indexed, with no import status and no SHACL
    /// results.
    pub(crate) fn new(settings: &'a Settings) -> DiagnosticInputs<'a> {
        DiagnosticInputs {
            settings,
            import_status: &no_status,
            shacl: &[],
            indexed: true,
            cached_import: false,
            max_syntax_errors: Some(diagnostics::MAX_SYNTAX_ERRORS),
        }
    }
}

fn no_status<'s>(_: &str) -> Option<&'s Status> {
    None
}

/// The diagnostics of `doc` as the editor shows them, in `enc` columns.
pub(crate) fn document(
    doc: Doc,
    k: &Knowledge,
    enc: Encoding,
    inputs: &DiagnosticInputs,
) -> Vec<Diagnostic> {
    if inputs.cached_import {
        return Vec::new();
    }
    let opts = diagnostics::Options {
        max_syntax_errors: inputs.max_syntax_errors,
        ..diagnostics::Options::from_settings(inputs.settings)
    };
    let mut out = match doc {
        Doc::Rdf(a) => diagnostics::diagnostics_with(a, k, enc, &opts),
        Doc::Sparql(q) => sparql_features::diagnostics(q, k, enc, &opts),
    };
    // "Known nowhere" findings wait for the initial index.
    if !inputs.indexed {
        out.retain(|d| !needs_index(d));
    }
    if let Doc::Rdf(a) = doc {
        out.extend(imports::diagnostics_with(a, enc, |iri| {
            (inputs.import_status)(iri)
        }));
    }
    out.extend(inputs.shacl.iter().cloned());
    // `lint.ignore`, and the off-by-default checks not in `lint.enable`.
    out.retain(|d| match &d.code {
        Some(lsp_types::NumberOrString::String(c)) => inputs.settings.lint_on(c),
        _ => true,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch::{self, BatchFile, Options};
    use crate::workspace::Workspace;
    use lsp_types::{DiagnosticSeverity, NumberOrString};
    use std::sync::Arc;

    fn code(d: &Diagnostic) -> &str {
        match &d.code {
            Some(NumberOrString::String(s)) => s,
            _ => "",
        }
    }

    /// A workspace of one document.
    fn analysed(text: &str) -> (Workspace, Arc<Analysis>) {
        let mut ws = Workspace::new(Vec::new(), Default::default());
        ws.open_doc("file:///work/a.ttl".into(), text.into(), 1);
        let a = ws.analysis("file:///work/a.ttl").unwrap();
        (ws, a)
    }

    const TEXT: &str = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
        @prefix unused: <http://u/> .\n\
        <http://x/o> a owl:Ontology ; owl:imports <http://x/missing> .\n\
        <http://x/a> rdfs:labl \"a\" ; foaf:name \"b\" .\n";

    #[test]
    fn the_order_is_own_then_imports_then_shacl() {
        let (mut ws, a) = analysed(TEXT);
        let k = &*ws.knowledge_mut();
        let status = Status::Unresolved("not loaded".into());
        let shacl = [Diagnostic {
            code: Some(NumberOrString::String("shacl-violation".into())),
            severity: Some(DiagnosticSeverity::ERROR),
            ..Default::default()
        }];
        let settings = Settings::default();
        let lookup = |iri: &str| (iri == "http://x/missing").then_some(&status);
        let inputs = DiagnosticInputs {
            import_status: &lookup,
            shacl: &shacl,
            ..DiagnosticInputs::new(&settings)
        };
        let d = document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs);
        let codes: Vec<&str> = d.iter().map(code).collect();
        let at = |c: &str| {
            codes
                .iter()
                .position(|x| *x == c)
                .unwrap_or_else(|| panic!("{c} in {codes:?}"))
        };
        assert!(at("unknown-term") < at("unresolved-import"));
        assert_eq!(at("unresolved-import") + 1, at("shacl-violation"));
        assert_eq!(codes.last(), Some(&"shacl-violation"));
    }

    #[test]
    fn findings_that_need_the_index_wait_for_it() {
        let text = "@prefix ex: <http://e.org/> .\n\
            @prefix sh: <http://www.w3.org/ns/shacl#> .\n\
            ex:P a sh:PropertyShape .\n";
        let (mut ws, a) = analysed(text);
        let k = &*ws.knowledge_mut();
        let settings = Settings::default();
        let mut inputs = DiagnosticInputs::new(&settings);
        let all = document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs);
        assert!(
            all.iter().any(needs_index),
            "{:?}",
            all.iter().map(code).collect::<Vec<_>>()
        );
        inputs.indexed = false;
        let early = document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs);
        assert!(early.iter().all(|d| !needs_index(d)));
        assert!(early.len() < all.len());
    }

    #[test]
    fn missing_checks_wait_for_the_index() {
        let text = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            @prefix ex: <http://e.org/> .\n\
            ex:onto a owl:Ontology .\n\
            ex:A a owl:Class .\n";
        let (mut ws, a) = analysed(text);
        let k = &*ws.knowledge_mut();
        let v = serde_json::json!({ "lint": { "enable": crate::features::curation::OPT_IN } });
        let settings = Settings::from_layers([(crate::settings::Layer::File, &v)]);
        let mut inputs = DiagnosticInputs::new(&settings);
        let mut codes: Vec<String> = document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs)
            .iter()
            .map(|d| code(d).to_string())
            .filter(|c| c.starts_with("missing-"))
            .collect();
        codes.sort();
        assert_eq!(
            codes,
            [
                "missing-definition",
                "missing-label",
                "missing-ontology-description",
                "missing-ontology-license",
                "missing-ontology-title",
            ]
        );
        inputs.indexed = false;
        let early = document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs);
        assert!(early.iter().all(|d| !code(d).starts_with("missing-")));
    }

    #[test]
    fn a_cached_import_has_no_diagnostics() {
        let (mut ws, a) = analysed(TEXT);
        let k = &*ws.knowledge_mut();
        let settings = Settings::default();
        let mut inputs = DiagnosticInputs::new(&settings);
        assert!(!document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs).is_empty());
        inputs.cached_import = true;
        assert!(document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs).is_empty());
    }

    #[test]
    fn batch_gives_what_the_composer_gives_for_the_same_input() {
        let (mut ws, a) = analysed(TEXT);
        let k = &*ws.knowledge_mut();
        let settings = Settings::default();
        let status = Status::Unresolved(imports::unloaded_note("http://x/missing"));
        let lookup = |iri: &str| (iri == "http://x/missing").then_some(&status);
        let inputs = DiagnosticInputs {
            import_status: &lookup,
            ..DiagnosticInputs::new(&settings)
        };
        let direct = document(Doc::Rdf(&a), k, Encoding::Utf16, &inputs);
        let files = [BatchFile::named("a.ttl", TEXT.to_string(), false)];
        let report = batch::analyse(&files, &settings, &Options::default());
        let from_batch: Vec<Diagnostic> = report.files[0]
            .problems
            .iter()
            .map(|p| p.diagnostic.clone())
            .collect();
        assert!(from_batch.iter().any(|d| code(d) == "unresolved-import"));
        assert_eq!(direct, from_batch);
    }
}
