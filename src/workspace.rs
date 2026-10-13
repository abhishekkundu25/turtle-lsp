//! Open documents, indexed files on disk, and project prefixes.
//!
//! Project prefixes: the nearest `prefixes.ttl` in the file's directory or an
//! ancestor (up to the workspace root) supplies prefixes to every Turtle file
//! below it, plus any `prefixes` given in the client's initializationOptions.

use crate::analysis::{Analysis, Encoding, offset_in};
use crate::syntax::PrefixMap;
use line_index::LineIndex;
use lsp_types::TextDocumentContentChangeEvent;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const PREFIX_FILE: &str = "prefixes.ttl";
const SKIP_DIRS: &[&str] = &[
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
    analysis: Option<Arc<Analysis>>,
}

#[derive(Default)]
pub struct Workspace {
    pub roots: Vec<PathBuf>,
    pub open: FxHashMap<String, Doc>,
    pub disk: FxHashMap<String, Arc<Analysis>>,
    pub settings_prefixes: PrefixMap,
    /// Directory -> prefixes from the nearest prefixes.ttl (None = no file found).
    prefix_cache: FxHashMap<PathBuf, Arc<PrefixMap>>,
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

pub fn is_turtle(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("ttl" | "turtle")
    )
}

impl Workspace {
    pub fn new(roots: Vec<PathBuf>, settings_prefixes: PrefixMap) -> Workspace {
        Workspace {
            roots,
            settings_prefixes,
            ..Default::default()
        }
    }

    // ---- documents ----

    pub fn open_doc(&mut self, uri: String, text: String, version: i32) {
        self.open.insert(
            uri,
            Doc {
                text,
                version,
                analysis: None,
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
        self.open.remove(uri);
        if let Some(path) = uri_to_path(uri) {
            self.reindex_disk_file(&path);
        }
    }

    pub fn analysis(&mut self, uri: &str) -> Option<Arc<Analysis>> {
        let external = match self.open.get(uri) {
            Some(d) if d.analysis.is_some() => return d.analysis.clone(),
            Some(_) => self.external_prefixes_for(uri),
            None => return self.disk.get(uri).cloned(),
        };
        let doc = self.open.get_mut(uri)?;
        let a = Arc::new(Analysis::new(uri.to_string(), &doc.text, external, true));
        doc.analysis = Some(a.clone());
        Some(a)
    }

    /// Every analysis in the workspace: open documents win over their disk copy.
    pub fn all(&mut self) -> Vec<Arc<Analysis>> {
        let uris: Vec<String> = self.open.keys().cloned().collect();
        let mut out: Vec<Arc<Analysis>> = uris.iter().filter_map(|u| self.analysis(u)).collect();
        out.extend(
            self.disk
                .iter()
                .filter(|(u, _)| !self.open.contains_key(*u))
                .map(|(_, a)| a.clone()),
        );
        out
    }

    pub fn text(&self, uri: &str) -> Option<&str> {
        self.open.get(uri).map(|d| d.text.as_str())
    }

    // ---- prefixes ----

    pub fn external_prefixes_for(&mut self, uri: &str) -> Arc<PrefixMap> {
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
                let parsed = crate::syntax::parse(&text, &PrefixMap::default());
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

    /// A prefixes.ttl changed: every analysis may resolve differently now.
    pub fn invalidate_prefixes(&mut self) {
        self.prefix_cache.clear();
        for d in self.open.values_mut() {
            d.analysis = None;
        }
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
        let jobs: Vec<(String, PathBuf, Arc<PrefixMap>)> = paths
            .into_iter()
            .filter_map(|p| {
                let uri = path_to_uri(&p)?;
                let ext = self.external_prefixes_for(&uri);
                Some((uri, p, ext))
            })
            .collect();
        let results: Vec<(String, Analysis)> = jobs
            .into_par_iter()
            .filter_map(|(uri, path, ext)| {
                if std::fs::metadata(&path).ok()?.len() > MAX_FILE_BYTES {
                    return None;
                }
                let text = std::fs::read_to_string(&path).ok()?;
                Some((uri.clone(), Analysis::new(uri, &text, ext, false)))
            })
            .collect();
        for (uri, a) in results {
            self.disk.insert(uri, Arc::new(a));
        }
    }

    pub fn reindex_disk_file(&mut self, path: &Path) {
        let Some(uri) = path_to_uri(path) else { return };
        if path.is_file() {
            self.index_paths(vec![path.to_path_buf()]);
        } else {
            self.disk.remove(&uri);
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
