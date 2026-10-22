//! Navigation and rename: definition, type definition, implementations,
//! references, document highlights, prepare rename and rename. Two requests
//! cross from RDF documents into SPARQL ones: references on an RDF term add
//! its uses in the workspace's SPARQL documents, and renaming an RDF term
//! renames it there too. A SPARQL document reads the others only when the
//! cursor is on a term; variables and prefixes stay within the document.
//!
//! Prepare rename has one convention for both kinds: `Err` refuses with its
//! message, `Ok(None)` is nothing to rename.

use super::{Doc, Session};
use crate::analysis::{Analysis, Encoding, SparqlAnalysis};
use crate::features::navigation;
use crate::sparql::features as sparql_features;
use crate::workspace::{find_prefix_file, path_to_uri, uri_to_path};
use lsp_types::{DocumentHighlight, Location, Position, PrepareRenameResponse, WorkspaceEdit};
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use std::sync::Arc;

impl Session<'_> {
    /// `textDocument/definition`; `None` when there is no document at `uri`.
    pub(crate) fn definition(&mut self, uri: &str, pos: Position) -> Option<Vec<Location>> {
        let (u, doc) = self.find(uri)?;
        let prefix_file = self.prefix_file_for(&u);
        let k = self.ws.knowledge();
        Some(match doc {
            Doc::Rdf(a) => navigation::definition(&a, k, self.enc, pos, prefix_file),
            Doc::Sparql(q) => sparql_features::definition(&q, k, self.enc, pos, prefix_file),
        })
    }

    /// `textDocument/typeDefinition`: none in a SPARQL document.
    pub(crate) fn type_definition(&mut self, uri: &str, pos: Position) -> Option<Vec<Location>> {
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => navigation::type_definition(&a, self.ws.knowledge(), self.enc, pos),
            Doc::Sparql(_) => Vec::new(),
        })
    }

    /// `textDocument/implementation`: none in a SPARQL document.
    pub(crate) fn implementations(&mut self, uri: &str, pos: Position) -> Option<Vec<Location>> {
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => navigation::implementations(&a, self.ws.knowledge(), self.enc, pos),
            Doc::Sparql(_) => Vec::new(),
        })
    }

    /// `textDocument/references`. An RDF term's references include its uses
    /// in the workspace's SPARQL documents, open or not.
    pub(crate) fn references(
        &mut self,
        uri: &str,
        pos: Position,
        include_declaration: bool,
    ) -> Option<Vec<Location>> {
        let enc = self.enc;
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => {
                let k = self.ws.knowledge();
                let mut locs = navigation::references(&a, k, enc, pos, include_declaration);
                if let Some((_, iri)) = rdf_term_at(&a, enc, pos) {
                    let docs = self.ws.sparql_documents();
                    locs.extend(sparql_features::term_references(
                        &iri,
                        include_declaration,
                        &docs,
                        enc,
                    ));
                }
                locs
            }
            Doc::Sparql(q) => {
                let others = self.sparql_others(&q, pos);
                let k = self.ws.knowledge();
                sparql_features::references(&q, k, enc, pos, include_declaration, &others)
            }
        })
    }

    /// `textDocument/documentHighlight`.
    pub(crate) fn highlights(
        &mut self,
        uri: &str,
        pos: Position,
    ) -> Option<Vec<DocumentHighlight>> {
        Some(match self.doc(uri)? {
            Doc::Rdf(a) => navigation::highlights(&a, self.enc, pos),
            Doc::Sparql(q) => sparql_features::highlights(&q, self.enc, pos),
        })
    }

    /// `textDocument/prepareRename`: `Err` refuses with its message (a
    /// JSON-LD term of a bundled or remote context), `Ok(None)` is nothing
    /// to rename at `pos` or no document at `uri`.
    pub(crate) fn prepare_rename(
        &mut self,
        uri: &str,
        pos: Position,
    ) -> Result<Option<PrepareRenameResponse>, String> {
        match self.doc(uri) {
            Some(Doc::Rdf(a)) => navigation::prepare_rename(&a, self.enc, pos),
            Some(Doc::Sparql(q)) => Ok(sparql_features::prepare_rename(&q, self.enc, pos)),
            None => Ok(None),
        }
    }

    /// `textDocument/rename`: `Err` refuses with its message, `Ok(None)` is
    /// no document at `uri`. Renaming an RDF term renames it in the
    /// workspace's SPARQL documents too; a JSON-LD term is renamed as a
    /// term, not as its IRI.
    pub(crate) fn rename(
        &mut self,
        uri: &str,
        pos: Position,
        new_name: &str,
    ) -> Result<Option<WorkspaceEdit>, String> {
        let Some((u, doc)) = self.find(uri) else {
            return Ok(None);
        };
        match doc {
            Doc::Rdf(a) => self.rdf_rename(&u, &a, pos, new_name).map(Some),
            Doc::Sparql(q) => {
                let prefix_file = self.prefix_file_for(&u);
                let others = self.sparql_others(&q, pos);
                let k = self.ws.knowledge();
                sparql_features::rename(&q, k, self.enc, pos, new_name, prefix_file, &others)
                    .map(Some)
            }
        }
    }

    /// The rename of RDF document `u`: its prefixes may come from a
    /// prefixes.ttl, here or for the other documents the rename reaches.
    fn rdf_rename(
        &mut self,
        u: &str,
        a: &Analysis,
        pos: Position,
        new_name: &str,
    ) -> Result<WorkspaceEdit, String> {
        let enc = self.enc;
        let term = rdf_term_at(a, enc, pos)
            .filter(|(offset, _)| navigation::jsonld::term_at(a, *offset).is_none())
            .map(|(offset, old)| {
                navigation::renamed_iri_at(a, offset, &old, new_name).map(|new| (old, new))
            })
            .transpose();
        let file = self.prefix_file_for(u);
        let roots = self.ws.roots().to_vec();
        let mut dirs: FxHashMap<PathBuf, Option<PathBuf>> = FxHashMap::default();
        let mut file_of = |uri: &str| {
            let dir = uri_to_path(uri)?.parent()?.to_path_buf();
            dirs.entry(dir)
                .or_insert_with_key(|d| find_prefix_file(&roots, d))
                .clone()
        };
        let scope = navigation::PrefixScope {
            file,
            file_of: &mut file_of,
        };
        let mut edit = navigation::rename(a, self.ws.knowledge(), enc, pos, new_name, scope)?;
        if let Some((old, new_iri)) = term? {
            let docs = self.ws.sparql_documents();
            sparql_features::rdf_term_rename(&old, &new_iri, &docs, enc, &mut edit)?;
        }
        Ok(edit)
    }

    /// The workspace's SPARQL documents for a query's references or rename,
    /// read only when the cursor is on a term (an IRI, prefixed name or `a`):
    /// variables and prefixes stay within the document.
    fn sparql_others(&mut self, q: &SparqlAnalysis, pos: Position) -> Vec<Arc<SparqlAnalysis>> {
        let on_term = q
            .offset(self.enc, pos)
            .is_some_and(|off| q.doc.structure().refs.iter().any(|r| r.span.contains(off)));
        if on_term {
            self.ws.sparql_documents()
        } else {
            Vec::new()
        }
    }

    /// The prefixes.ttl that supplies prefixes to `uri`'s folder: the open
    /// document's analysis, else the file read from disk.
    fn prefix_file_for(&mut self, uri: &str) -> Option<Arc<Analysis>> {
        let dir = uri_to_path(uri)?.parent()?.to_path_buf();
        let file = self.ws.find_prefix_file(&dir)?;
        let file_uri = path_to_uri(&file)?;
        if let Some(a) = self.ws.analysis(&file_uri) {
            return Some(a);
        }
        let text = std::fs::read_to_string(&file).ok()?;
        Some(Arc::new(Analysis::new(
            file_uri,
            &text,
            Arc::new(Default::default()),
            false,
        )))
    }
}

/// The offset and IRI of the term at `pos` in an RDF document, if the
/// cursor is on one.
fn rdf_term_at(a: &Analysis, enc: Encoding, pos: Position) -> Option<(u32, String)> {
    let offset = a.offset(enc, pos)?;
    match navigation::target_at(a, offset)? {
        (navigation::Target::Iri(iri), _) => Some((offset, iri.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;
    use crate::workspace::Workspace;
    use lsp_types::TextEdit;
    use std::path::Path;

    const TTL: &str = "@prefix ex: <http://example.org/> .\nex:a ex:p ex:o .\n";
    const QUERY: &str = "PREFIX ex: <http://example.org/>\nSELECT ?s WHERE { ?s ex:p ?o }\n";

    /// A fresh folder below the temp dir (canonical), removed when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let d =
                std::env::temp_dir().join(format!("turtle-lsp-ide-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d.canonicalize().unwrap())
        }

        /// Write `name` with `text`; its URI.
        fn write(&self, name: &str, text: &str) -> String {
            let path = self.0.join(name);
            std::fs::write(&path, text).unwrap();
            uri_of(&path)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn uri_of(path: &Path) -> String {
        path_to_uri(path).unwrap()
    }

    /// A workspace over `dir` whose JSON-LD contexts are the bundled ones.
    fn workspace(dir: &Dir) -> Workspace {
        let mut ws = Workspace::new(vec![dir.0.clone()], Default::default());
        ws.set_context_loader(Arc::new(crate::jsonld::loader::Bundled));
        ws
    }

    /// The position `delta` characters into the first `needle` of `text`
    /// (ASCII: UTF-16 and UTF-8 agree), or of the last with `last`.
    fn at(text: &str, needle: &str, delta: usize, last: bool) -> Position {
        let i = if last {
            text.rfind(needle)
        } else {
            text.find(needle)
        };
        let i = i.unwrap() + delta;
        let line = text[..i].matches('\n').count();
        let col = i - text[..i].rfind('\n').map_or(0, |n| n + 1);
        Position::new(line as u32, col as u32)
    }

    /// `text` with single-line `edits` applied.
    fn apply(text: &str, edits: &[TextEdit]) -> String {
        let starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let offset = |p: Position| starts[p.line as usize] + p.character as usize;
        let mut edits = edits.to_vec();
        edits.sort_by_key(|e| std::cmp::Reverse(offset(e.range.start)));
        let mut out = text.to_string();
        for e in edits {
            out.replace_range(offset(e.range.start)..offset(e.range.end), &e.new_text);
        }
        out
    }

    fn uris(locs: &[Location]) -> Vec<&str> {
        locs.iter().map(|l| l.uri.as_str()).collect()
    }

    fn changed(edit: &WorkspaceEdit) -> Vec<&str> {
        let mut out: Vec<&str> = edit
            .changes
            .as_ref()
            .unwrap()
            .keys()
            .map(|u| u.as_str())
            .collect();
        out.sort();
        out
    }

    fn edits_of<'e>(edit: &'e WorkspaceEdit, uri: &str) -> &'e [TextEdit] {
        let changes = edit.changes.as_ref().unwrap();
        changes.iter().find(|(u, _)| u.as_str() == uri).unwrap().1
    }

    #[test]
    fn references_on_an_rdf_term_include_its_uses_in_a_closed_query_file() {
        let dir = Dir::new("refs-rdf");
        let ttl = dir.write("data.ttl", TTL);
        let rq = dir.write("q.rq", QUERY);
        let mut ws = workspace(&dir);
        ws.open_doc(ttl.clone(), TTL.into(), 1);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let locs = s.references(&ttl, at(TTL, "ex:p", 3, false), true).unwrap();
        assert_eq!(uris(&locs), [ttl.as_str(), rq.as_str()], "{locs:?}");
    }

    /// Whether the other SPARQL files were read shows in a file written
    /// after the first request: without file watching, a file created after
    /// the folders were read is not found until the prefixes change.
    #[test]
    fn references_on_a_sparql_variable_leave_the_other_query_files_unread() {
        let dir = Dir::new("refs-var");
        let a = dir.write("a.rq", QUERY);
        let settings = Settings::default();

        let mut ws = workspace(&dir);
        ws.open_doc(a.clone(), QUERY.into(), 1);
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let vars = s.references(&a, at(QUERY, "?s", 1, true), true).unwrap();
        assert_eq!(vars.len(), 2, "{vars:?}");
        assert!(uris(&vars).iter().all(|u| *u == a), "{vars:?}");
        let b = dir.write("b.rq", QUERY);
        let terms = s.references(&a, at(QUERY, "ex:p", 3, false), true).unwrap();
        assert!(uris(&terms).contains(&b.as_str()), "{terms:?}");

        // The probe tells: once a term's references read the folders, a file
        // written after them is not found.
        let mut ws = workspace(&dir);
        ws.open_doc(a.clone(), QUERY.into(), 1);
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        s.references(&a, at(QUERY, "ex:p", 3, false), true);
        let c = dir.write("c.rq", QUERY);
        let terms = s.references(&a, at(QUERY, "ex:p", 3, false), true).unwrap();
        assert!(!uris(&terms).contains(&c.as_str()), "{terms:?}");
    }

    #[test]
    fn renaming_an_rdf_term_rewrites_its_uses_in_a_query_file() {
        let dir = Dir::new("rename-rdf");
        let ttl = dir.write("data.ttl", TTL);
        let rq = dir.write("q.rq", QUERY);
        let mut ws = workspace(&dir);
        ws.open_doc(ttl.clone(), TTL.into(), 1);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let edit = s
            .rename(&ttl, at(TTL, "ex:p", 3, false), "ex:q")
            .unwrap()
            .unwrap();
        assert_eq!(changed(&edit), [ttl.as_str(), rq.as_str()]);
        assert_eq!(
            apply(TTL, edits_of(&edit, &ttl)),
            TTL.replace("ex:p", "ex:q")
        );
        assert_eq!(
            apply(QUERY, edits_of(&edit, &rq)),
            QUERY.replace("ex:p", "ex:q")
        );
    }

    #[test]
    fn renaming_a_prefix_in_a_query_names_the_closed_prefixes_ttl_declaring_it() {
        let dir = Dir::new("rename-prefix");
        dir.write("prefixes.ttl", "@prefix ex: <http://example.org/> .\n");
        let text = "SELECT ?s WHERE { ?s ex:p ?o }\n";
        let rq = dir.write("q.rq", text);
        let mut ws = workspace(&dir);
        ws.open_doc(rq.clone(), text.into(), 1);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let err = s.rename(&rq, at(text, "ex:p", 1, false), "fx").unwrap_err();
        let file = dir.0.join("prefixes.ttl");
        assert_eq!(
            err,
            format!(
                "prefix 'ex:' is declared in {}; rename it there",
                file.display()
            )
        );
    }

    #[test]
    fn renaming_a_json_ld_term_renames_the_term_not_its_iri() {
        let dir = Dir::new("rename-jsonld");
        let text = r#"{ "@context": { "name": "http://schema.org/name" }, "@id": "http://example.org/ann", "name": "Ann" }"#;
        let json = dir.write("ann.jsonld", text);
        dir.write(
            "q.rq",
            "SELECT ?n WHERE { ?s <http://schema.org/name> ?n }\n",
        );
        let mut ws = workspace(&dir);
        ws.open_doc(json.clone(), text.into(), 1);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        let edit = s
            .rename(&json, at(text, "\"name\"", 2, true), "fullName")
            .unwrap()
            .unwrap();
        assert_eq!(changed(&edit), [json.as_str()]);
        assert_eq!(
            apply(text, edits_of(&edit, &json)),
            text.replace("\"name\":", "\"fullName\":")
        );
    }

    #[test]
    fn prepare_rename_has_nothing_in_a_query_and_refuses_a_bundled_json_ld_term() {
        let dir = Dir::new("prepare");
        let rq = dir.write("q.rq", QUERY);
        let text = r#"{ "@context": "https://schema.org/", "@type": "Person", "name": "Ann" }"#;
        let json = dir.write("ann.jsonld", text);
        let mut ws = workspace(&dir);
        ws.open_doc(rq.clone(), QUERY.into(), 1);
        ws.open_doc(json.clone(), text.into(), 1);
        let settings = Settings::default();
        let mut s = Session::new(&mut ws, Encoding::Utf16, &settings, false);
        assert_eq!(
            s.prepare_rename(&rq, at(QUERY, "SELECT", 2, false)),
            Ok(None)
        );
        let err = s
            .prepare_rename(&json, at(text, "\"name\"", 2, false))
            .unwrap_err();
        assert!(err.contains("https://schema.org/"), "{err}");
    }
}
