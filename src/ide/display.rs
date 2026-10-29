//! What the editor shows of a document: hover, document and workspace
//! symbols, folding ranges, the type hierarchy, semantic tokens, inlay hints,
//! code lenses and the workspace graph (`turtle/graph`). Two of them follow the client's settings: inlay hints
//! `inlayHints.labels`, and an RDF document's reference lenses
//! `codeLens.references`.
//!
//! A request on a URI the workspace has no document for answers `None`. The
//! type hierarchy is RDF only: a SPARQL document has no items.

use super::{Doc, Session};
use crate::analysis::{Analysis, Encoding};
use crate::features::graph::{self, GraphParams, GraphResult, InvalidParams};
use crate::features::{hierarchy, hover, inlay, lens, semantic, symbols};
use crate::knowledge::{Knowledge, Source};
use crate::sparql::features as sparql_features;
use crate::workspace::normalize_uri;
use lsp_types::{
    CodeLens, DocumentSymbol, FoldingRange, Hover, InlayHint, Position, Range, SemanticTokens,
    SemanticTokensLegend, SymbolInformation, TypeHierarchyItem,
};
use std::sync::Arc;

impl Session<'_> {
    /// `textDocument/hover`.
    pub(crate) fn hover(&mut self, uri: &str, pos: Position) -> Option<Hover> {
        let doc = self.doc(uri)?;
        let k = self.ws.knowledge();
        match doc {
            Doc::Rdf(a) => hover::hover(&a, k, self.enc, pos),
            Doc::Sparql(q) => sparql_features::hover(&q, k, self.enc, pos),
        }
    }

    /// `textDocument/documentSymbol`, nested.
    pub(crate) fn document_symbols(&mut self, uri: &str) -> Option<Vec<DocumentSymbol>> {
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => symbols::document_symbols(&a, self.enc),
            Doc::Sparql(q) => sparql_features::document_symbols(&q, self.enc),
        })
    }

    /// `workspace/symbol`: the workspace's terms that match `query`.
    pub(crate) fn workspace_symbols(&mut self, query: &str) -> Vec<SymbolInformation> {
        symbols::workspace_symbols(self.ws.knowledge(), self.enc, query)
    }

    /// `textDocument/foldingRange`.
    pub(crate) fn folding_ranges(&mut self, uri: &str) -> Option<Vec<FoldingRange>> {
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => symbols::folding_ranges(&a),
            Doc::Sparql(q) => sparql_features::folding_ranges(&q),
        })
    }

    /// `textDocument/prepareTypeHierarchy`: the class or property at `pos`
    /// of an RDF document.
    pub(crate) fn prepare_type_hierarchy(
        &mut self,
        uri: &str,
        pos: Position,
    ) -> Option<Vec<TypeHierarchyItem>> {
        let Doc::Rdf(a) = self.doc(uri)? else {
            return None;
        };
        hierarchy::prepare(&a, self.ws.knowledge(), self.enc, pos)
    }

    /// `typeHierarchy/supertypes`, named with the prefixes of the document
    /// the item was asked from (its `data.uri`).
    pub(crate) fn supertypes(&mut self, item: &TypeHierarchyItem) -> Vec<TypeHierarchyItem> {
        let asking = self.asking(item);
        hierarchy::supertypes(item, self.ws.knowledge(), self.enc, asking.as_deref())
    }

    /// `typeHierarchy/subtypes`, named as [`Session::supertypes`] names them.
    pub(crate) fn subtypes(&mut self, item: &TypeHierarchyItem) -> Vec<TypeHierarchyItem> {
        let asking = self.asking(item);
        hierarchy::subtypes(item, self.ws.knowledge(), self.enc, asking.as_deref())
    }

    /// The RDF analysis of the document `item` was asked from.
    fn asking(&mut self, item: &TypeHierarchyItem) -> Option<Arc<Analysis>> {
        hierarchy::asking_uri(item).and_then(|u| self.ws.analysis(&normalize_uri(u)))
    }

    /// The legend of the tokens [`Session::semantic_tokens_full`] and
    /// [`Session::semantic_tokens_range`] give, for the server's
    /// capabilities.
    pub(crate) fn semantic_tokens_legend() -> SemanticTokensLegend {
        semantic::legend()
    }

    /// `textDocument/semanticTokens/full`.
    pub(crate) fn semantic_tokens_full(&mut self, uri: &str) -> Option<SemanticTokens> {
        let doc = self.doc(uri)?;
        let k = self.ws.knowledge();
        Some(match doc {
            Doc::Rdf(a) => semantic::full(&a, k, self.enc),
            Doc::Sparql(q) => sparql_features::semantic_tokens_full(&q, k, self.enc),
        })
    }

    /// `textDocument/semanticTokens/range`.
    pub(crate) fn semantic_tokens_range(
        &mut self,
        uri: &str,
        range: Range,
    ) -> Option<SemanticTokens> {
        let doc = self.doc(uri)?;
        let k = self.ws.knowledge();
        Some(match doc {
            Doc::Rdf(a) => semantic::range(&a, k, self.enc, range),
            Doc::Sparql(q) => sparql_features::semantic_tokens_range(&q, k, self.enc, range),
        })
    }

    /// `textDocument/inlayHint`: label hints in `range`, in the
    /// `inlayHints.labels` mode (none when it is `off`).
    pub(crate) fn inlay_hints(&mut self, uri: &str, range: Range) -> Option<Vec<InlayHint>> {
        let mode = self.settings.inlay_hint_labels;
        let doc = self.doc(uri)?;
        let k = self.ws.knowledge();
        Some(match doc {
            Doc::Rdf(a) => inlay::inlay_hints(&a, k, self.enc, range, mode),
            Doc::Sparql(q) => sparql_features::inlay_hints(&q, k, self.enc, range, mode),
        })
    }

    /// `textDocument/codeLens`: an RDF document's reference lenses,
    /// unresolved (none with `codeLens.references` off); a SPARQL query's
    /// "▶ Run query" lens, whatever that setting.
    pub(crate) fn code_lenses(&mut self, uri: &str) -> Option<Vec<CodeLens>> {
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => lens::code_lenses(&a, self.enc, self.settings.code_lens_references),
            Doc::Sparql(q) => lens::query_lenses(&q, self.enc),
        })
    }

    /// `turtle/graph`: the workspace graph `params` asks for, laid out.
    pub(crate) fn graph(&mut self, params: GraphParams) -> Result<GraphResult, InvalidParams> {
        graph::graph(self.ws.knowledge(), self.enc, params)
    }

    /// [`Session::graph`] to run elsewhere: the index as it is now, sources
    /// in the order the index holds them.
    pub(crate) fn graph_job(&mut self, params: GraphParams) -> GraphJob {
        let sources = self
            .ws
            .knowledge()
            .sources()
            .map(|(key, s, a)| (key.to_string(), s, a.clone()))
            .collect();
        GraphJob {
            sources,
            enc: self.enc,
            params,
        }
    }

    /// `codeLens/resolve`: a reference lens with its count; any other lens
    /// as it is.
    pub(crate) fn resolve_code_lens(&mut self, lens: CodeLens) -> CodeLens {
        lens::resolve(lens, self.ws.knowledge(), self.enc)
    }
}

/// `turtle/graph` with a copy of what the knowledge index is built from, to
/// compute away from the session (on a thread of its own): the analyses are
/// shared, the index is rebuilt from them. A rebuild equals the index it
/// copies (`Knowledge`'s `incremental_updates_match_a_rebuild`).
pub(crate) struct GraphJob {
    sources: Vec<(String, Source, Arc<Analysis>)>,
    enc: Encoding,
    params: GraphParams,
}

impl GraphJob {
    /// The answer [`Session::graph`] gives for the same workspace.
    pub(crate) fn run(self) -> Result<GraphResult, InvalidParams> {
        let mut k = Knowledge::default();
        for (key, source, a) in self.sources {
            if source == Source::External {
                k.add_external(&key, a);
            } else {
                k.set(&key, source, a);
            }
        }
        graph::graph(&k, self.enc, self.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::Encoding;
    use crate::settings::{InlayLabels, Settings};
    use crate::workspace::{Workspace, path_to_uri};
    use std::path::PathBuf;

    const PREFIXES: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n";

    /// A fresh folder below the temp dir (canonical), removed when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir().join(format!(
                "turtle-lsp-ide-display-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(crate::workspace::real(&d))
        }

        /// Write `name` with `text`; its URI.
        fn write(&self, name: &str, text: &str) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            path_to_uri(&path).unwrap()
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A workspace over `dir` with `docs` (name, text) written and open;
    /// their URIs.
    fn workspace(dir: &Dir, docs: &[(&str, &str)]) -> (Workspace, Vec<String>) {
        let mut ws = Workspace::new(vec![dir.0.clone()], Default::default());
        ws.set_context_loader(Arc::new(crate::jsonld::loader::Bundled));
        let uris = docs
            .iter()
            .map(|(name, text)| {
                let uri = dir.write(name, text);
                ws.open_doc(uri.clone(), text.to_string(), 1);
                uri
            })
            .collect();
        (ws, uris)
    }

    /// The position `delta` characters into the last `needle` of `text`
    /// (ASCII: UTF-16 and UTF-8 agree).
    fn at(text: &str, needle: &str, delta: usize) -> Position {
        let i = text.rfind(needle).unwrap() + delta;
        let line = text[..i].matches('\n').count();
        let col = i - text[..i].rfind('\n').map_or(0, |n| n + 1);
        Position::new(line as u32, col as u32)
    }

    fn markdown(h: &Hover) -> &str {
        match &h.contents {
            lsp_types::HoverContents::Markup(m) => &m.value,
            other => panic!("not markdown: {other:?}"),
        }
    }

    fn everything() -> Range {
        Range::new(Position::new(0, 0), Position::new(100, 0))
    }

    #[test]
    fn hover_routes_to_the_rdf_or_the_sparql_implementation_by_document_kind() {
        let dir = Dir::new("hover");
        let ttl = format!("{PREFIXES}ex:a rdfs:label \"Alpha\" .\n");
        let rq = "PREFIX ex: <http://example.org/>\nSELECT ?s WHERE { ?s ex:p ex:a }\n";
        let (mut ws, uris) = workspace(&dir, &[("data.ttl", &ttl), ("q.rq", rq)]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);

        let rdf = s.hover(&uris[0], at(&ttl, "ex:a", 3)).unwrap();
        assert!(markdown(&rdf).contains("Alpha"), "{rdf:?}");
        assert_eq!(rdf.range.unwrap().start, at(&ttl, "ex:a", 0));

        // The SPARQL parse answers in the query's text, from what the
        // Turtle document says of the term.
        let sparql = s.hover(&uris[1], at(rq, "ex:a", 3)).unwrap();
        assert!(markdown(&sparql).contains("Alpha"), "{sparql:?}");
        assert_eq!(sparql.range.unwrap().start, at(rq, "ex:a", 0));
        // A `PREFIX` name shows its namespace.
        let prefix = s.hover(&uris[1], Position::new(0, 8)).unwrap();
        assert_eq!(markdown(&prefix), "**ex:** → `<http://example.org/>`");

        assert!(
            s.hover("file:///nowhere.ttl", Position::new(0, 0))
                .is_none()
        );
    }

    #[test]
    fn inlay_hints_follow_the_labels_setting() {
        let dir = Dir::new("inlay");
        let ttl = format!("{PREFIXES}ex:P1234 rdfs:label \"pump pressure\" .\nex:a ex:P1234 1 .\n");
        let (mut ws, uris) = workspace(&dir, &[("data.ttl", &ttl)]);

        let settings = Settings::default();
        assert_eq!(settings.inlay_hint_labels, InlayLabels::Opaque);
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let hints = s.inlay_hints(&uris[0], everything()).unwrap();
        assert!(!hints.is_empty());
        assert!(
            hints.iter().all(|h| h.position.line >= 2),
            "only after the opaque name: {hints:?}"
        );

        let off = Settings {
            inlay_hint_labels: InlayLabels::Off,
            ..Settings::default()
        };
        let mut s = Session::new(&mut ws, Encoding::Utf16, &off, false);
        assert_eq!(
            s.inlay_hints(&uris[0], everything()).map(|h| h.len()),
            Some(0)
        );
    }

    #[test]
    fn code_lenses_count_references_in_turtle_and_run_a_query_file() {
        let dir = Dir::new("lens");
        let ttl = format!("{PREFIXES}ex:a rdfs:label \"A\" .\nex:b rdfs:seeAlso ex:a .\n");
        let rq = "PREFIX ex: <http://example.org/>\nSELECT ?s WHERE { ?s ex:p ex:a }\n";
        let (mut ws, uris) = workspace(&dir, &[("data.ttl", &ttl), ("q.rq", rq)]);

        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let lenses = s.code_lenses(&uris[0]).unwrap();
        assert_eq!(lenses.len(), 2, "one per subject: {lenses:?}");
        assert!(
            lenses
                .iter()
                .all(|l| l.command.is_none() && l.data.is_some())
        );
        let first = s.resolve_code_lens(lenses[0].clone());
        assert_eq!(first.command.unwrap().title, "1 reference");

        let run = s.code_lenses(&uris[1]).unwrap();
        assert_eq!(run.len(), 1, "{run:?}");
        let title = &run[0].command.as_ref().unwrap().title;
        assert_eq!(title, lens::RUN_QUERY_TITLE);
        assert_eq!(run[0].range.start, at(rq, "SELECT", 0));
        // A resolved lens comes back as it is.
        assert_eq!(s.resolve_code_lens(run[0].clone()), run[0]);

        let off = Settings {
            code_lens_references: false,
            ..Settings::default()
        };
        let mut s = Session::new(&mut ws, Encoding::Utf16, &off, false);
        assert_eq!(s.code_lenses(&uris[0]), Some(vec![]));
        assert_eq!(s.code_lenses(&uris[1]).unwrap().len(), 1);
    }

    #[test]
    fn supertypes_are_named_with_the_prefixes_of_the_asking_document() {
        let dir = Dir::new("supertypes");
        let a = "@prefix my: <http://example.org/> .\n\
            @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
            my:B rdfs:subClassOf my:A .\n";
        let b = "@prefix ex: <http://example.org/> .\nex:x a ex:B .\n";
        let (mut ws, uris) = workspace(&dir, &[("a.ttl", a), ("b.ttl", b)]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);

        let names = |items: Vec<TypeHierarchyItem>| -> Vec<String> {
            items.into_iter().map(|i| i.name).collect()
        };
        let mut item = s
            .prepare_type_hierarchy(&uris[1], at(b, "ex:B", 3))
            .unwrap()
            .remove(0);
        assert_eq!(item.name, "ex:B");
        assert_eq!(names(s.supertypes(&item)), ["ex:A"]);

        item.data.as_mut().unwrap()["uri"] = uris[0].clone().into();
        assert_eq!(names(s.supertypes(&item)), ["my:A"]);
        assert_eq!(names(s.subtypes(&item)), Vec::<String>::new());

        item.data.as_mut().unwrap()["uri"] = serde_json::Value::Null;
        assert_eq!(names(s.supertypes(&item)), ["<http://example.org/A>"]);
    }

    #[test]
    fn the_type_hierarchy_has_no_items_in_a_query() {
        let dir = Dir::new("hierarchy-rq");
        let rq = "PREFIX rdfs: <http://www.w3.org/2000/01/rdf-schema#>\n\
            SELECT ?c WHERE { ?c a rdfs:Class }\n";
        let (mut ws, uris) = workspace(&dir, &[("q.rq", rq)]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        assert!(
            s.prepare_type_hierarchy(&uris[0], at(rq, "rdfs:Class", 6))
                .is_none()
        );
    }

    #[test]
    fn display_requests_answer_none_for_an_unknown_document() {
        let dir = Dir::new("unknown");
        let (mut ws, _) = workspace(&dir, &[]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let uri = "file:///nowhere.ttl";
        assert!(s.document_symbols(uri).is_none());
        assert!(s.folding_ranges(uri).is_none());
        assert!(s.semantic_tokens_full(uri).is_none());
        assert!(s.semantic_tokens_range(uri, everything()).is_none());
        assert!(s.inlay_hints(uri, everything()).is_none());
        assert!(s.code_lenses(uri).is_none());
    }

    #[test]
    fn a_graph_job_answers_what_the_session_answers() {
        let dir = Dir::new("graph-job");
        let a = format!(
            "{PREFIXES}ex:Pump a rdfs:Class ; rdfs:label \"Pump\" .\n\
             ex:p1 a ex:Pump ; ex:feeds ex:p2 ; ex:part [ ex:size 3 ] .\n"
        );
        let b = format!("{PREFIXES}ex:p2 a ex:Pump ; rdfs:subClassOf ex:Device .\n");
        let (mut ws, _) = workspace(&dir, &[("a.ttl", &a)]);
        // One document indexed from disk, not open.
        dir.write("b.ttl", &b);
        ws.index_paths(vec![dir.0.join("b.ttl")]);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let params = || -> GraphParams {
            serde_json::from_value(serde_json::json!({
                "types": true,
                "literals": true,
                "layout": { "iterations": 40 }
            }))
            .unwrap()
        };
        let here = s.graph(params()).unwrap();
        assert!(here.nodes.len() >= 5, "{:?}", here.nodes);
        assert!(here.nodes.iter().any(|n| n.uri.ends_with("/b.ttl")));
        assert_eq!(s.graph_job(params()).run().unwrap(), here);
        // Invalid parameters fail the same way.
        let bad: GraphParams = serde_json::from_value(serde_json::json!({ "focus": "" })).unwrap();
        assert_eq!(s.graph_job(bad.clone()).run(), s.graph(bad));
    }
}
