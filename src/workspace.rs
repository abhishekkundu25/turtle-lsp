//! Open documents, indexed files on disk, and project prefixes.
//!
//! Project prefixes: the nearest `prefixes.ttl` in the file's directory or an
//! ancestor (up to the workspace root) supplies prefixes to every Turtle and
//! TriG file below it, plus any `prefixes` given in the client's
//! initializationOptions. N-Triples files have no prefixes.

use crate::analysis::{Analysis, Encoding, offset_in};
use crate::knowledge::{DocFacts, Knowledge, Source};
use crate::syntax::{PrefixMap, Syntax};
use line_index::LineIndex;
use lsp_types::TextDocumentContentChangeEvent;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const PREFIX_FILE: &str = "prefixes.ttl";
pub const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];
pub const MAX_FILES: usize = 20_000;
/// Files larger than this are not indexed (an open file is still analyzed).
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

pub struct Doc {
    pub text: String,
    pub version: i32,
    pub syntax: Syntax,
    analysis: Option<Arc<Analysis>>,
    /// A cached copy of an import: the import already contributes its facts.
    cached_import: bool,
}

#[derive(Default)]
pub struct Workspace {
    pub roots: Vec<PathBuf>,
    pub open: FxHashMap<String, Doc>,
    pub disk: FxHashMap<String, Arc<Analysis>>,
    pub settings_prefixes: PrefixMap,
    /// Facts from every analysis: the open document's when it is open, else
    /// the disk copy's. Kept current by every method that changes either.
    knowledge: Knowledge,
    /// Directory -> prefixes from the nearest prefixes.ttl (None = no file found).
    prefix_cache: FxHashMap<PathBuf, Arc<PrefixMap>>,
    /// Where fetched imports are cached (canonical), if there is a cache.
    import_cache: Option<PathBuf>,
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = url::Url::parse(uri).ok()?.to_file_path().ok()?;
    Some(normalize_drive(path))
}

/// Windows: `c:\x` and `C:\x` are the same file; use the upper-case drive letter.
fn normalize_drive(path: PathBuf) -> PathBuf {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if cfg!(windows) && b.len() >= 2 && b[1] == b':' && b[0].is_ascii_lowercase() {
        let mut fixed = s.into_owned();
        fixed[..1].make_ascii_uppercase();
        return PathBuf::from(fixed);
    }
    path
}

/// One spelling per file. Clients differ (`file:///c%3A/x` from VS Code on
/// Windows, `file:///C:/x` elsewhere); every document is keyed by this form.
pub fn normalize_uri(uri: &str) -> String {
    if !uri.starts_with("file:") {
        return uri.to_string();
    }
    uri_to_path(uri)
        .and_then(|p| path_to_uri(&p))
        .unwrap_or_else(|| uri.to_string())
}

pub fn path_to_uri(path: &Path) -> Option<String> {
    url::Url::from_file_path(path).ok().map(|u| u.to_string())
}

/// `path` with symlinks resolved, as far as it exists (`/var` vs `/private/var`).
fn canonical(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    match (path.parent(), path.file_name()) {
        (Some(dir), Some(name)) => canonical(dir).join(name),
        _ => path.to_path_buf(),
    }
}

/// A file this server reads: `.ttl`, `.turtle`, `.trig` or `.nt`.
pub fn is_turtle(path: &Path) -> bool {
    Syntax::from_extension(&path.to_string_lossy()).is_some()
}

impl Workspace {
    pub fn new(roots: Vec<PathBuf>, settings_prefixes: PrefixMap) -> Workspace {
        Workspace {
            roots,
            settings_prefixes,
            import_cache: crate::imports::cache_dir().map(|d| canonical(&d.join("imports"))),
            ..Default::default()
        }
    }

    /// An open document that is a cached copy of an import: analyzed, but
    /// neither registered with the knowledge index nor diagnosed.
    pub fn is_cached_import(&self, uri: &str) -> bool {
        self.open.get(uri).is_some_and(|d| d.cached_import)
    }

    // ---- documents ----

    /// The syntax comes from the URI's extension.
    pub fn open_doc(&mut self, uri: String, text: String, version: i32) {
        let syntax = Syntax::detect(None, &uri);
        self.open_doc_as(uri, text, version, syntax);
    }

    pub fn open_doc_as(&mut self, uri: String, text: String, version: i32, syntax: Syntax) {
        let cached_import = self.import_cache.as_ref().is_some_and(|cache| {
            uri_to_path(&uri).is_some_and(|p| canonical(&p).starts_with(cache))
        });
        if !cached_import {
            // The open text supersedes the indexed copy (re-read on close), and
            // a second full analysis of a large file costs hundreds of MB. The
            // index keeps its contribution until the open document is analyzed.
            self.disk.remove(&uri);
        }
        self.open.insert(
            uri,
            Doc {
                text,
                version,
                syntax,
                analysis: None,
                cached_import,
            },
        );
    }

    pub fn change_doc(
        &mut self,
        uri: &str,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
        enc: Encoding,
    ) {
        let Some(doc) = self.open.get_mut(uri) else {
            return;
        };
        for ch in changes {
            match ch.range {
                None => doc.text = ch.text,
                Some(r) => {
                    let lines = LineIndex::new(&doc.text);
                    let (Some(s), Some(e)) = (
                        offset_in(&lines, &doc.text, enc, r.start),
                        offset_in(&lines, &doc.text, enc, r.end),
                    ) else {
                        continue;
                    };
                    let (s, e) = (s.min(e) as usize, e.max(s) as usize);
                    doc.text
                        .replace_range(s.min(doc.text.len())..e.min(doc.text.len()), &ch.text);
                }
            }
        }
        doc.version = version;
        doc.analysis = None;
    }

    /// Closing a file keeps it indexed from disk.
    pub fn close_doc(&mut self, uri: &str) {
        if self.open.remove(uri).is_some_and(|d| d.cached_import) {
            return;
        }
        // Only RDF files belong in the index (a .txt opened as Turtle does not).
        if let Some(path) = uri_to_path(uri).filter(|p| is_turtle(p)) {
            self.reindex_disk_file(&path);
        }
        self.sync(uri);
    }

    /// Register the winning analysis of `uri` with the knowledge index: the
    /// open document's (once analyzed), else the disk copy's.
    fn sync(&mut self, uri: &str) {
        match self.open.get(uri) {
            Some(d) if d.cached_import => {}
            Some(d) => {
                if let Some(a) = &d.analysis {
                    self.knowledge.set(uri, Source::Open, a.clone());
                }
                // Not analyzed yet: the previous contribution stands until it is.
            }
            None => match self.disk.get(uri) {
                Some(a) => self.knowledge.set(uri, Source::Disk, a.clone()),
                None => self.knowledge.remove(uri),
            },
        }
    }

    /// The knowledge index, after analyzing open documents edited since the
    /// last call.
    pub fn knowledge(&mut self) -> &Knowledge {
        let stale: Vec<String> = self
            .open
            .iter()
            .filter(|(_, d)| d.analysis.is_none())
            .map(|(u, _)| u.clone())
            .collect();
        for uri in stale {
            self.analysis(&uri);
        }
        &self.knowledge
    }

    /// Read-only sources (e.g. imported ontologies): they add facts to the
    /// knowledge index but are never diagnosed.
    pub fn knowledge_mut(&mut self) -> &mut Knowledge {
        &mut self.knowledge
    }

    pub fn analysis(&mut self, uri: &str) -> Option<Arc<Analysis>> {
        let syntax = match self.open.get(uri) {
            Some(d) if d.analysis.is_some() => return d.analysis.clone(),
            Some(d) => d.syntax,
            None => return self.disk.get(uri).cloned(),
        };
        let external = self.external_prefixes_as(uri, syntax);
        let doc = self.open.get_mut(uri)?;
        let a = Arc::new(Analysis::with_syntax(
            uri.to_string(),
            &doc.text,
            external,
            true,
            syntax,
        ));
        doc.analysis = Some(a.clone());
        if !doc.cached_import {
            self.knowledge.set(uri, Source::Open, a.clone());
        }
        Some(a)
    }

    pub fn text(&self, uri: &str) -> Option<&str> {
        self.open.get(uri).map(|d| d.text.as_str())
    }

    // ---- prefixes ----

    pub fn external_prefixes_for(&mut self, uri: &str) -> Arc<PrefixMap> {
        self.external_prefixes_as(uri, Syntax::detect(None, uri))
    }

    fn external_prefixes_as(&mut self, uri: &str, syntax: Syntax) -> Arc<PrefixMap> {
        if !syntax.has_prefixes() {
            return Arc::new(PrefixMap::default());
        }
        let Some(path) = uri_to_path(uri) else {
            return Arc::new(self.settings_prefixes.clone());
        };
        if path.file_name().is_some_and(|n| n == PREFIX_FILE) {
            return Arc::new(self.settings_prefixes.clone());
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if let Some(m) = self.prefix_cache.get(&dir) {
            return m.clone();
        }
        let mut map = self.settings_prefixes.clone();
        if let Some(file) = self.find_prefix_file(&dir) {
            let file_uri = path_to_uri(&file).unwrap_or_default();
            let text = self
                .open
                .get(&file_uri)
                .map(|d| d.text.clone())
                .or_else(|| std::fs::read_to_string(&file).ok());
            if let Some(text) = text {
                let parsed = crate::syntax::parse(&text, &PrefixMap::default(), Syntax::Turtle);
                for p in parsed.prefixes {
                    map.insert(p.name, p.iri.to_string());
                }
            }
        }
        let map = Arc::new(map);
        self.prefix_cache.insert(dir, map.clone());
        map
    }

    /// The prefixes.ttl that supplies prefixes to files in `dir`, if any.
    pub fn find_prefix_file(&self, dir: &Path) -> Option<PathBuf> {
        let root = self
            .roots
            .iter()
            .filter(|r| dir.starts_with(r))
            .max_by_key(|r| r.as_os_str().len());
        let mut cur = Some(dir);
        while let Some(d) = cur {
            let candidate = d.join(PREFIX_FILE);
            if candidate.is_file() {
                return Some(candidate);
            }
            if root.is_none_or(|r| d == r.as_path()) {
                break;
            }
            cur = d.parent();
        }
        None
    }

    /// An open prefixes.ttl is being edited: open documents re-resolve.
    pub fn invalidate_open_prefixes(&mut self) {
        self.prefix_cache.clear();
        for d in self.open.values_mut() {
            d.analysis = None;
        }
    }

    /// A prefixes.ttl changed: every analysis may resolve differently now.
    pub fn invalidate_prefixes(&mut self) {
        self.invalidate_open_prefixes();
        let paths: Vec<PathBuf> = self.disk.keys().filter_map(|u| uri_to_path(u)).collect();
        self.index_paths(paths);
    }

    // ---- disk index ----

    pub fn scan_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack: Vec<PathBuf> = roots.to_vec();
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                        stack.push(e.path());
                    }
                } else if ft.is_file()
                    && is_turtle(&e.path())
                    && e.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES)
                {
                    out.push(e.path());
                    if out.len() >= MAX_FILES {
                        return out;
                    }
                }
            }
        }
        out
    }

    /// Parse files in parallel and add them to the disk index.
    pub fn index_paths(&mut self, paths: Vec<PathBuf>) {
        let parsed = self.parse_paths(paths);
        self.add_disk(parsed, true);
    }

    /// Add analyses parsed elsewhere (the initial index, built on another
    /// thread). With `replace` false, files indexed meanwhile are kept.
    pub fn add_disk(&mut self, files: Vec<(String, Analysis)>, replace: bool) {
        let files: Vec<(String, Arc<Analysis>, DocFacts)> = files
            .into_par_iter()
            .filter(|(uri, _)| !self.open.contains_key(uri))
            .filter(|(uri, _)| replace || !self.disk.contains_key(uri))
            .map(|(uri, a)| {
                let facts = DocFacts::new(&a);
                (uri, Arc::new(a), facts)
            })
            .collect();
        for (uri, a, facts) in files {
            self.knowledge
                .set_prepared(&uri, Source::Disk, a.clone(), facts);
            self.disk.insert(uri, a);
        }
    }

    /// Parse files in parallel, without adding them to anything.
    pub fn parse_paths(&mut self, paths: Vec<PathBuf>) -> Vec<(String, Analysis)> {
        let jobs: Vec<(String, PathBuf, Arc<PrefixMap>)> = paths
            .into_iter()
            .filter_map(|p| {
                let uri = path_to_uri(&p)?;
                let ext = self.external_prefixes_for(&uri);
                Some((uri, p, ext))
            })
            .collect();
        jobs.into_par_iter()
            .filter_map(|(uri, path, ext)| {
                if std::fs::metadata(&path).ok()?.len() > MAX_FILE_BYTES {
                    return None;
                }
                let text = std::fs::read_to_string(&path).ok()?;
                Some((uri.clone(), Analysis::new(uri, &text, ext, false)))
            })
            .collect()
    }

    pub fn reindex_disk_file(&mut self, path: &Path) {
        let Some(uri) = path_to_uri(path) else { return };
        if path.is_file() {
            self.index_paths(vec![path.to_path_buf()]);
        } else {
            self.disk.remove(&uri);
            self.sync(&uri);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_uri_is_stable() {
        let dir = std::env::temp_dir().join("turtle lsp");
        let uri = path_to_uri(&dir.join("a.ttl")).unwrap();
        assert_eq!(normalize_uri(&uri), uri);
        assert_eq!(normalize_uri(&normalize_uri(&uri)), uri);
        assert_eq!(normalize_uri("untitled:Untitled-1"), "untitled:Untitled-1");
    }

    #[test]
    fn cached_imports_are_analyzed_but_not_registered() {
        let dir = std::env::temp_dir().join(format!("turtle-lsp-cache-{}", std::process::id()));
        let cache = dir.join("cache").join("imports");
        std::fs::create_dir_all(&cache).unwrap();
        let mut ws = Workspace {
            import_cache: Some(canonical(&cache)),
            ..Default::default()
        };
        let text = "@prefix ex: <http://example.org/> .\nex:A a ex:B .\n";
        let cached = path_to_uri(&cache.join("a.ttl")).unwrap();
        let own = path_to_uri(&dir.join("b.ttl")).unwrap();
        let import = Arc::new(Analysis::new(cached.clone(), text, Arc::default(), false));
        ws.knowledge_mut()
            .add_external("http://example.org/", import);

        ws.open_doc(cached.clone(), text.into(), 1);
        ws.open_doc(own.clone(), text.into(), 1);
        assert!(ws.is_cached_import(&cached));
        assert!(!ws.is_cached_import(&own));
        assert!(ws.analysis(&cached).is_some());
        assert!(ws.knowledge().get(&cached).is_none());
        assert!(ws.knowledge().get(&own).is_some());
        assert_eq!(ws.knowledge().docs().count(), 2);

        ws.close_doc(&cached);
        assert!(!ws.disk.contains_key(&cached));
        assert_eq!(ws.knowledge().docs().count(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn vscode_and_url_spellings_of_a_windows_path_match() {
        assert_eq!(
            normalize_uri("file:///c%3A/Users/x/a.ttl"),
            normalize_uri("file:///C:/Users/x/a.ttl")
        );
        assert_eq!(
            normalize_uri("file:///c:/Users/x/a.ttl"),
            "file:///C:/Users/x/a.ttl"
        );
    }
}
