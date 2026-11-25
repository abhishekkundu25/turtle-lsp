//! The SPARQL files under the roots: read on the first ask, kept current
//! from the watcher's events, and the open SPARQL documents beside them.

use super::{
    DocKind, MAX_FILE_BYTES, MAX_FILES, Stamp, Workspace, is_sparql_path, path_to_uri, read_text,
    scan_roots_matching, stamp_of,
};
use crate::analysis::SparqlAnalysis;
use crate::syntax::{PrefixMap, Syntax};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// A SPARQL file on disk, as read.
pub(super) struct SparqlFile {
    uri: String,
    /// The file's stamp when it was read.
    stamp: Option<Stamp>,
    analysis: Arc<SparqlAnalysis>,
}

impl Workspace {
    /// Whether `uri` is an open SPARQL document.
    pub(crate) fn is_sparql(&self, uri: &str) -> bool {
        self.open
            .get(uri)
            .is_some_and(|d| d.kind == DocKind::Sparql)
    }

    /// The parse of the open SPARQL document `uri` (one parse per version),
    /// or `None` when `uri` is no open SPARQL document.
    pub(crate) fn sparql(&mut self, uri: &str) -> Option<Arc<SparqlAnalysis>> {
        match self.open.get(uri) {
            Some(d) if d.kind != DocKind::Sparql => return None,
            Some(d) if d.query.is_some() => return d.query.clone(),
            Some(_) => {}
            None => return None,
        }
        let external = self.external_prefixes_as(uri, Syntax::Turtle);
        let doc = self.open.get_mut(uri)?;
        let q = Arc::new(SparqlAnalysis::new(uri.to_string(), &doc.text, external));
        doc.query = Some(q.clone());
        Some(q)
    }

    /// Every SPARQL document of the workspace, read on demand (never at
    /// startup, never per keystroke): the open SPARQL documents from memory,
    /// then the `.rq`, `.sparql` and `.ru` files under the roots that are not
    /// open (the indexing limits apply: the skipped directories,
    /// [`MAX_FILES`] files, [`MAX_FILE_BYTES`] per file), parsed with their
    /// folder's prefixes. A file open as another kind is left out.
    ///
    /// The files are found and parsed on the first call and kept. They are
    /// kept current by [`Workspace::sparql_files_changed`] and
    /// [`Workspace::remove_under_any`] (the watcher's events), and a file
    /// written since (its stamp differs) is read again here, so an answer
    /// reflects the files' content as it is now. With file watching, a later
    /// call reads no directory, and a file created without the watcher
    /// saying so is not found until the prefixes change; without it
    /// ([`Workspace::rescan_sparql`]), each call lists the folders again to
    /// find new files, and parses only those and the ones written since.
    pub(crate) fn sparql_documents(&mut self) -> Vec<Arc<SparqlAnalysis>> {
        let open: Vec<String> = self
            .open
            .iter()
            .filter(|(_, d)| d.kind == DocKind::Sparql)
            .map(|(u, _)| u.clone())
            .collect();
        let mut out: Vec<Arc<SparqlAnalysis>> =
            open.iter().filter_map(|u| self.sparql(u)).collect();
        self.refresh_sparql_files();
        if let Some(files) = &self.sparql_files {
            out.extend(
                files
                    .values()
                    .filter(|f| !self.open.contains_key(&f.uri))
                    .map(|f| f.analysis.clone()),
            );
        }
        out
    }

    /// Read the SPARQL files if that was not done yet; else read again those
    /// written since (and, with [`Workspace::rescan_sparql`], those created
    /// since).
    fn refresh_sparql_files(&mut self) {
        match &self.sparql_files {
            None => {
                let paths = scan_roots_matching(&self.roots, is_sparql_path).paths;
                let files = self.read_sparql_files(paths).into_iter().collect();
                self.sparql_files = Some(files);
            }
            Some(files) => {
                let mut changed: Vec<PathBuf> = files
                    .iter()
                    .filter(|(path, f)| stamp_of(path) != f.stamp)
                    .map(|(path, _)| path.clone())
                    .collect();
                if self.rescan_sparql {
                    // Deleted files have no stamp: they are among the changed.
                    let listed = scan_roots_matching(&self.roots, is_sparql_path).paths;
                    changed.extend(listed.into_iter().filter(|p| !files.contains_key(p)));
                }
                self.sparql_files_changed(&changed);
            }
        }
    }

    /// Parse SPARQL files in parallel, with their folders' prefixes. A file
    /// that cannot be read or is over [`MAX_FILE_BYTES`] is left out.
    fn read_sparql_files(&mut self, paths: Vec<PathBuf>) -> Vec<(PathBuf, SparqlFile)> {
        let jobs: Vec<(String, PathBuf, Arc<PrefixMap>)> = paths
            .into_iter()
            .filter_map(|p| {
                let uri = path_to_uri(&p)?;
                let ext = self.external_prefixes_as(&uri, Syntax::Turtle);
                Some((uri, p, ext))
            })
            .collect();
        jobs.into_par_iter()
            .filter_map(|(uri, path, ext)| {
                let stamp = stamp_of(&path);
                if stamp.is_some_and(|s| s.len > MAX_FILE_BYTES) {
                    return None;
                }
                let text = read_text(&path)?;
                let analysis = Arc::new(SparqlAnalysis::new(uri.clone(), &text, ext));
                Some((
                    path,
                    SparqlFile {
                        uri,
                        stamp,
                        analysis,
                    },
                ))
            })
            .collect()
    }

    /// The SPARQL files at `paths` were created, changed or deleted: read the
    /// ones that are SPARQL files the scan would take, drop the others. All
    /// in one parallel pass. Nothing is read before the first
    /// [`Workspace::sparql_documents`].
    pub(crate) fn sparql_files_changed(&mut self, paths: &[PathBuf]) {
        let Some(mut files) = self.sparql_files.take() else {
            return;
        };
        let mut seen = rustc_hash::FxHashSet::default();
        let mut room = MAX_FILES.saturating_sub(files.len());
        let mut read = Vec::new();
        for path in paths {
            if !seen.insert(path) {
                continue;
            }
            let takes = is_sparql_path(path) && self.scan_reaches(path);
            if takes && (files.contains_key(path) || room > 0) {
                if !files.contains_key(path) {
                    room -= 1;
                }
                read.push(path.clone());
            } else {
                files.remove(path);
            }
        }
        let read_paths: Vec<PathBuf> = read.clone();
        let mut fresh: FxHashMap<PathBuf, SparqlFile> =
            self.read_sparql_files(read).into_iter().collect();
        for path in read_paths {
            match fresh.remove(&path) {
                Some(f) => {
                    files.insert(path, f);
                }
                // Unreadable now.
                None => {
                    files.remove(&path);
                }
            }
        }
        self.sparql_files = Some(files);
    }
}
