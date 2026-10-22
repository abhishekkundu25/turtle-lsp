//! What the editor changes in a document, and what it reports there:
//! completion, code actions, formatting and diagnostics. Formatting follows
//! the client's `format.lineWidth`; lazily resolved code actions follow
//! [`Session::resolve_edits`].
//!
//! A request on a URI the workspace has no document for answers `None`.
//! The two resolve requests carry no document: a completion item is routed
//! by `data.sparql`, a code action by the document its `data.uri` names.

use super::{Doc, Session};
use crate::diagnose::{self, DiagnosticInputs};
use crate::features::{completion, embedded, format, quickfix, refactor};
use crate::layout;
use crate::sparql::features as sparql_features;
use lsp_types::{
    CodeAction, CodeActionOrCommand, CodeActionParams, CompletionItem, CompletionParams,
    CompletionResponse, Diagnostic, FormattingOptions, TextEdit,
};

impl Session<'_> {
    /// `textDocument/completion`. In an RDF document, `?` and `$` start
    /// variables, which it has only inside the SPARQL queries of SHACL
    /// shapes: typed anywhere else they offer nothing (`None`).
    pub(crate) fn completion(&mut self, p: &CompletionParams) -> Option<CompletionResponse> {
        let at = &p.text_document_position;
        let doc = self.doc(at.text_document.uri.as_str())?;
        let k = self.ws.knowledge();
        let a = match doc {
            Doc::Rdf(a) => a,
            Doc::Sparql(q) => return sparql_features::completion(&q, k, self.enc, p),
        };
        let trigger = p
            .context
            .as_ref()
            .and_then(|c| c.trigger_character.as_deref());
        if matches!(trigger, Some("?" | "$"))
            && !a
                .offset(self.enc, at.position)
                .is_some_and(|o| embedded::in_query(&a, o))
        {
            return None;
        }
        Some(completion::complete(&a, k, self.enc, at.position))
    }

    /// `completionItem/resolve`: by the SPARQL resolver when the item's data
    /// says `"sparql": true`, else by the RDF one.
    pub(crate) fn resolve_completion(&mut self, item: CompletionItem) -> CompletionItem {
        let sparql = item.data.as_ref().is_some_and(|d| d["sparql"] == true);
        let k = self.ws.knowledge();
        if sparql {
            sparql_features::resolve_completion(item, k)
        } else {
            completion::resolve(item, k)
        }
    }

    /// `textDocument/codeAction`. An RDF document's actions are its quick
    /// fixes of the kinds `context.only` asks for, then its refactorings,
    /// then "Fetch and cache" for its unresolved web imports.
    pub(crate) fn code_actions(
        &mut self,
        p: &CodeActionParams,
    ) -> Option<Vec<CodeActionOrCommand>> {
        let doc = self.doc(p.text_document.uri.as_str())?;
        let (enc, lazy) = (self.enc, self.resolve_edits);
        let k = self.ws.knowledge();
        let a = match doc {
            Doc::Rdf(a) => a,
            Doc::Sparql(q) => return Some(sparql_features::code_actions(&q, k, enc, p, lazy)),
        };
        let only = p.context.only.as_deref();
        let mut actions: Vec<_> = quickfix::code_actions(&a, k, enc, p)
            .into_iter()
            .filter(|c| match c {
                CodeActionOrCommand::CodeAction(c) => {
                    c.kind.as_ref().is_none_or(|k| refactor::wanted(only, k))
                }
                CodeActionOrCommand::Command(_) => true,
            })
            .collect();
        actions.extend(refactor::code_actions(&a, enc, p, lazy));
        actions.extend(crate::imports::fetch_actions(&a, enc, p));
        Some(actions)
    }

    /// `codeAction/resolve`: the edit of an action that came without one,
    /// computed in the document its `data.uri` names. An action with no such
    /// document comes back as it is.
    pub(crate) fn resolve_code_action(&mut self, action: CodeAction) -> CodeAction {
        let Some(uri) = action.data.as_ref().and_then(|d| d["uri"].as_str()) else {
            return action;
        };
        match self.doc(uri) {
            Some(Doc::Rdf(a)) => refactor::resolve(&a, self.enc, action),
            Some(Doc::Sparql(q)) => sparql_features::resolve_code_action(&q, self.enc, action),
            None => action,
        }
    }

    /// `textDocument/formatting`, with the client's `format.lineWidth`.
    pub(crate) fn formatting(
        &mut self,
        uri: &str,
        options: &FormattingOptions,
    ) -> Option<Vec<TextEdit>> {
        let width = self.settings.format_line_width as usize;
        let opts = layout::Options::from_lsp(options, width);
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => format::formatting(&a, self.enc, &opts),
            Doc::Sparql(q) => sparql_features::formatting(&q, self.enc, &opts),
        })
    }

    /// The diagnostics the document at `uri` shows, from what `inputs` says
    /// besides the document and the knowledge ([`diagnose::document`]).
    pub(crate) fn diagnostics(
        &mut self,
        uri: &str,
        inputs: &DiagnosticInputs,
    ) -> Option<Vec<Diagnostic>> {
        let doc = self.doc(uri)?;
        let k = self.ws.knowledge();
        Some(match doc {
            Doc::Rdf(a) => diagnose::document(diagnose::Doc::Rdf(&a), k, self.enc, inputs),
            Doc::Sparql(q) => diagnose::document(diagnose::Doc::Sparql(&q), k, self.enc, inputs),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Encoding;
    use crate::imports::Status;
    use crate::settings::Settings;
    use crate::workspace::Workspace;
    use lsp_types::{
        CodeActionContext, CodeActionKind, CompletionContext, CompletionTriggerKind, Position,
        Range, TextDocumentIdentifier, TextDocumentPositionParams,
    };

    const TTL: &str = "file:///work/a.ttl";
    const RQ: &str = "file:///work/q.rq";

    /// A workspace with `docs` (URI, text) open, nothing on disk.
    fn workspace(docs: &[(&str, &str)]) -> Workspace {
        let mut ws = Workspace::new(Vec::new(), Default::default());
        for (uri, text) in docs {
            ws.open_doc(uri.to_string(), text.to_string(), 1);
        }
        ws
    }

    /// The position `delta` characters into the first `needle` of `text`
    /// (ASCII: UTF-16 and UTF-8 agree).
    fn at(text: &str, needle: &str, delta: usize) -> Position {
        let i = text.find(needle).unwrap() + delta;
        let line = text[..i].matches('\n').count();
        let col = i - text[..i].rfind('\n').map_or(0, |n| n + 1);
        Position::new(line as u32, col as u32)
    }

    fn completion_params(uri: &str, pos: Position, trigger: &str) -> CompletionParams {
        CompletionParams {
            text_document_position: TextDocumentPositionParams {
                text_document: TextDocumentIdentifier {
                    uri: uri.parse().unwrap(),
                },
                position: pos,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
            context: Some(CompletionContext {
                trigger_kind: CompletionTriggerKind::TRIGGER_CHARACTER,
                trigger_character: Some(trigger.into()),
            }),
        }
    }

    fn action_params(
        uri: &str,
        range: Range,
        diagnostics: Vec<Diagnostic>,
        only: Option<Vec<CodeActionKind>>,
    ) -> CodeActionParams {
        CodeActionParams {
            text_document: TextDocumentIdentifier {
                uri: uri.parse().unwrap(),
            },
            range,
            context: CodeActionContext {
                diagnostics,
                only,
                trigger_kind: None,
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        }
    }

    fn actions(all: Vec<CodeActionOrCommand>) -> Vec<CodeAction> {
        all.into_iter()
            .map(|c| match c {
                CodeActionOrCommand::CodeAction(c) => c,
                other => panic!("not an action: {other:?}"),
            })
            .collect()
    }

    fn kind(c: &CodeAction) -> &str {
        c.kind.as_ref().map_or("", CodeActionKind::as_str)
    }

    #[test]
    fn a_variable_trigger_completes_only_inside_a_shacl_query() {
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n\
            @prefix ex: <http://example.org/> .\n\
            ex:S sh:sparql [ sh:select \"SELECT ?this WHERE { ?this ex:p ?o }\" ] .\n\
            ex:a ex:p ? .\n";
        let mut ws = workspace(&[(TTL, text)]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);

        let outside = at(text, "ex:p ? .", 6);
        for trigger in ["?", "$"] {
            assert_eq!(
                s.completion(&completion_params(TTL, outside, trigger)),
                None
            );
        }
        // Invoked there, completion answers.
        let mut invoked = completion_params(TTL, outside, "?");
        invoked.context = None;
        assert!(s.completion(&invoked).is_some());

        let inside = at(text, "?o }", 1);
        let got = s.completion(&completion_params(TTL, inside, "?")).unwrap();
        let Some(Doc::Rdf(a)) = s.doc(TTL) else {
            panic!("no analysis")
        };
        let want = completion::complete(&a, s.ws.knowledge(), Encoding::Utf16, inside);
        assert_eq!(got, want);
        let items = match &got {
            CompletionResponse::Array(items) => items,
            CompletionResponse::List(list) => &list.items,
        };
        assert!(items.iter().any(|i| i.label == "?this"), "{items:?}");
    }

    #[test]
    fn only_filters_the_quick_fixes_and_no_only_offers_all_three_sources() {
        let text = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
            @prefix ex: <http://example.org/> .\n\
            <http://example.org/o> a owl:Ontology ; owl:imports <http://example.org/missing> .\n\
            ex:a ex:p foaf:x .\n";
        let mut ws = workspace(&[(TTL, text)]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let status = Status::Unresolved("not loaded".into());
        let lookup = |iri: &str| (iri == "http://example.org/missing").then_some(&status);
        let inputs = DiagnosticInputs {
            import_status: &lookup,
            ..DiagnosticInputs::new(&settings)
        };
        let diagnostics = s.diagnostics(TTL, &inputs).unwrap();
        let on_name = Range::new(at(text, "ex:a ", 1), at(text, "ex:a ", 1));

        let p = action_params(TTL, on_name, diagnostics.clone(), None);
        let all = actions(s.code_actions(&p).unwrap());
        let titles: Vec<&str> = all.iter().map(|c| c.title.as_str()).collect();
        assert!(
            titles.iter().any(|t| t.starts_with("Add @prefix foaf:")),
            "{titles:?}"
        );
        assert!(titles.contains(&"Expand to full IRI"), "{titles:?}");
        assert!(
            titles.contains(&"Fetch http://example.org/missing and cache it"),
            "{titles:?}"
        );

        let quickfix = Some(vec![CodeActionKind::QUICKFIX]);
        let p = action_params(TTL, on_name, diagnostics, quickfix);
        let fixes = actions(s.code_actions(&p).unwrap());
        assert!(fixes.iter().all(|c| kind(c) == "quickfix"), "{fixes:?}");
        assert_eq!(
            fixes,
            all.into_iter()
                .filter(|c| kind(c) == "quickfix")
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn an_action_on_a_query_resolves_with_the_sparql_resolver() {
        let text = "PREFIX ex: <http://example.org/>\n\
            SELECT * WHERE { <http://example.org/a> ?p ?o }\n";
        let mut ws = workspace(&[(RQ, text)]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, true);
        let p = action_params(RQ, Range::default(), vec![], None);
        let lazy = actions(s.code_actions(&p).unwrap())
            .into_iter()
            .find(|c| c.title == "Shorten all IRIs in file")
            .unwrap();
        assert!(lazy.edit.is_none());

        let resolved = s.resolve_code_action(lazy.clone());
        let Some(Doc::Sparql(q)) = s.doc(RQ) else {
            panic!("no query")
        };
        assert_eq!(
            resolved,
            sparql_features::resolve_code_action(&q, Encoding::Utf16, lazy)
        );
        let changes = resolved.edit.unwrap().changes.unwrap();
        let edits: Vec<&str> = changes
            .values()
            .flatten()
            .map(|e| e.new_text.as_str())
            .collect();
        assert_eq!(edits, ["ex:a"]);
    }

    #[test]
    fn formatting_follows_the_line_width_setting() {
        let text = "@prefix ex: <http://example.org/> .\n\
            ex:a ex:p ex:b , ex:c , ex:d , ex:e , ex:f , ex:g , ex:h .\n";
        let mut ws = workspace(&[(TTL, text)]);
        let options = FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        };
        let width = |n: u32| Settings {
            format_line_width: n,
            ..Settings::default()
        };
        let mut lines = |settings: &Settings| {
            let mut s = Session::new(&mut ws, Encoding::Utf16, settings, false);
            let edits = s.formatting(TTL, &options).unwrap();
            edits
                .first()
                .map_or(text, |e| e.new_text.as_str())
                .lines()
                .count()
        };
        let wide = lines(&width(200));
        let narrow = lines(&width(30));
        assert!(narrow > wide, "{narrow} lines at 30, {wide} at 200");

        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        assert!(s.formatting("file:///nowhere.ttl", &options).is_none());
    }

    #[test]
    fn diagnostics_are_the_composers_for_the_open_document() {
        let ttl = "@prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
            @prefix unused: <http://u/> .\n\
            <http://x/a> rdfs:labl \"a\" ; foaf:name \"b\" .\n";
        let rq = "PREFIX unused: <http://u/>\nSELECT * WHERE { ?s foaf:name ?o }\n";
        let mut ws = workspace(&[(TTL, ttl), (RQ, rq)]);
        let settings = Settings::default();
        let inputs = DiagnosticInputs::new(&settings);
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);

        let got = s.diagnostics(TTL, &inputs).unwrap();
        assert!(!got.is_empty());
        let Some(Doc::Rdf(a)) = s.doc(TTL) else {
            panic!("no analysis")
        };
        let k = s.ws.knowledge();
        let want = diagnose::document(diagnose::Doc::Rdf(&a), k, Encoding::Utf16, &inputs);
        assert_eq!(got, want);

        let got = s.diagnostics(RQ, &inputs).unwrap();
        assert!(!got.is_empty());
        let Some(Doc::Sparql(q)) = s.doc(RQ) else {
            panic!("no query")
        };
        let k = s.ws.knowledge();
        let want = diagnose::document(diagnose::Doc::Sparql(&q), k, Encoding::Utf16, &inputs);
        assert_eq!(got, want);

        assert!(s.diagnostics("file:///nowhere.ttl", &inputs).is_none());
    }
}
