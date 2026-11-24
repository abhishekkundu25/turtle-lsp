//! The prefixes each document gets from outside its text: the settings'
//! prefixes and the nearest prefixes.ttl, read from disk or from its open
//! document, and kept current as those change.

use super::{PREFIX_FILE, Stamp, Workspace, file_uri_name, path_to_uri, stamp_of, uri_to_path};
use crate::analysis::Analysis;
use crate::syntax::{PrefixMap, Syntax};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Whether `uri` names a prefixes.ttl.
pub(super) fn is_prefix_uri(uri: &str) -> bool {
    uri.ends_with(&format!("/{PREFIX_FILE}"))
}

/// A prefixes.ttl's prefixes, as read.
pub(super) struct PrefixEntry {
    stamp: Option<Stamp>,
    /// Read from the open document, not from the file.
    from_open: bool,
    map: Arc<PrefixMap>,
}

impl Workspace {
    // ---- prefixes ----

    /// The prefixes a document at `uri` gets, by its name alone: a name whose
    /// syntax its content gives counts as Turtle here, and a parse whose
    /// text turns out otherwise drops them (`parse_files` in `disk`).
    pub(crate) fn external_prefixes_for(&mut self, uri: &str) -> Arc<PrefixMap> {
        let syntax = if Syntax::by_content(uri) {
            Syntax::Turtle
        } else {
            Syntax::of(None, uri, "")
        };
        self.external_prefixes_as(uri, syntax)
    }

    /// The project's prefixes where `uri` is, whatever its syntax: those a
    /// Turtle document there may use undeclared. Reasoning names IRIs with
    /// them in N-Triples, N-Quads and JSON-LD documents too, which parse
    /// with none.
    pub(crate) fn project_prefixes_for(&mut self, uri: &str) -> Arc<PrefixMap> {
        self.external_prefixes_as(uri, Syntax::Turtle)
    }

    pub(super) fn external_prefixes_as(&mut self, uri: &str, syntax: Syntax) -> Arc<PrefixMap> {
        if !syntax.has_prefixes() {
            return Arc::new(PrefixMap::default());
        }
        let Some(path) = uri_to_path(uri) else {
            return self.open_prefixes_by_uri(uri);
        };
        if path.file_name().is_some_and(|n| n == PREFIX_FILE) {
            return self.settings_only();
        }
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let file = match self.prefix_dirs.get(&dir) {
            Some(file) => file.clone(),
            None => {
                let file = self.find_prefix_file(&dir);
                self.prefix_dirs.insert(dir, file.clone());
                file
            }
        };
        match file {
            Some(file) => self.prefix_file_map(&file),
            None => self.settings_only(),
        }
    }

    /// The settings prefixes alone.
    fn settings_only(&mut self) -> Arc<PrefixMap> {
        let settings = &self.settings_prefixes;
        self.settings_map
            .get_or_insert_with(|| Arc::new(settings.clone()))
            .clone()
    }

    /// The prefixes `file` (a prefixes.ttl) declares over the settings
    /// prefixes, read once: the open document's text when it is open, else
    /// the file's.
    fn prefix_file_map(&mut self, file: &Path) -> Arc<PrefixMap> {
        if let Some(e) = self.prefix_files.get(file) {
            return e.map.clone();
        }
        let file_uri = path_to_uri(file).unwrap_or_default();
        let from_open = self.open.contains_key(&file_uri);
        let text = self
            .open
            .get(&file_uri)
            .map(|d| d.text.clone())
            .or_else(|| std::fs::read_to_string(file).ok());
        let mut map = self.settings_prefixes.clone();
        if let Some(text) = text {
            let parsed = crate::syntax::parse(&text, &PrefixMap::default(), Syntax::Turtle);
            for p in parsed.prefixes {
                map.insert(p.name, p.iri.to_string());
            }
        }
        let map = Arc::new(map);
        self.prefix_files.insert(
            file.to_path_buf(),
            PrefixEntry {
                stamp: stamp_of(file),
                from_open,
                map: map.clone(),
            },
        );
        map
    }

    /// Prefixes for a document without a file path (the browser build): the
    /// nearest open prefixes.ttl in the URI's directory or an ancestor, over
    /// the settings prefixes. The native build always has the file path.
    fn open_prefixes_by_uri(&mut self, uri: &str) -> Arc<PrefixMap> {
        let url = url::Url::parse(uri)
            .ok()
            .filter(|u| cfg!(target_arch = "wasm32") && u.scheme() == "file");
        let Some(dir) = url.as_ref().and_then(|u| u.join("./").ok()) else {
            return Arc::new(self.settings_prefixes.clone());
        };
        if file_uri_name(uri).is_some_and(|n| n == PREFIX_FILE) {
            return Arc::new(self.settings_prefixes.clone());
        }
        let key = dir.to_string();
        if let Some(m) = self.uri_prefix_cache.get(&key) {
            return m.clone();
        }
        let mut map = self.settings_prefixes.clone();
        let mut cur = dir;
        loop {
            let file = cur.join(PREFIX_FILE).map(|u| u.to_string());
            if let Some(d) = file.ok().and_then(|f| self.open.get(&f)) {
                let parsed = crate::syntax::parse(&d.text, &PrefixMap::default(), Syntax::Turtle);
                for p in parsed.prefixes {
                    map.insert(p.name, p.iri.to_string());
                }
                break;
            }
            match cur.join("../") {
                Ok(up) if up != cur => cur = up,
                _ => break,
            }
        }
        let map = Arc::new(map);
        self.uri_prefix_cache.insert(key, map.clone());
        map
    }

    /// The prefixes.ttl that supplies prefixes to files in `dir`, if any.
    pub(crate) fn find_prefix_file(&self, dir: &Path) -> Option<PathBuf> {
        find_prefix_file(&self.roots, dir)
    }

    /// The open prefixes.ttl at `file` is being edited: the open documents
    /// below it re-resolve. Indexed files follow when it is saved (see
    /// [`Workspace::prefixes_changed`]).
    pub(super) fn invalidate_open_prefixes(&mut self, file: &Path) {
        self.sparql_files = None;
        self.prefix_files.remove(file);
        self.uri_prefix_cache.clear();
        self.reset_open_below(file.parent());
    }

    /// Analyze the open documents below `dir` again (all of them for `None`,
    /// and those that have no file path).
    fn reset_open_below(&mut self, dir: Option<&Path>) {
        for (uri, d) in self.open.iter_mut() {
            let below = match (dir, uri_to_path(uri)) {
                (Some(dir), Some(p)) => p.starts_with(dir),
                _ => true,
            };
            if below {
                d.analyzed.outdate();
                d.query = None;
            }
        }
    }

    /// A prefixes.ttl at `file` was created, changed, deleted or closed with
    /// unsaved edits, or (`None`) the settings prefixes changed: the open
    /// documents it serves are analyzed again, and the paths of the indexed
    /// files whose prefixes differ now are returned, for the caller to parse
    /// again ([`Workspace::prefix_jobs`], off the main loop if it can). A
    /// file the change leaves with the same prefixes is not among them.
    pub(super) fn prefixes_changed(&mut self, file: Option<&Path>) -> Vec<PathBuf> {
        let scope = file.and_then(Path::parent).map(Path::to_path_buf);
        match file {
            Some(file) => {
                let unchanged = self.prefix_files.get(file).is_some_and(|e| {
                    e.stamp.is_some()
                        && e.stamp == stamp_of(file)
                        && !e.from_open
                        && !self.is_open_path(file)
                });
                if unchanged {
                    return Vec::new();
                }
                self.prefix_files.remove(file);
            }
            None => {
                self.prefix_files.clear();
                self.settings_map = None;
            }
        }
        // Which file serves a directory may have changed with it, and the
        // queries are parsed with the prefixes.
        self.sparql_files = None;
        self.prefix_dirs.clear();
        self.uri_prefix_cache.clear();
        self.reset_open_below(scope.as_deref());
        let indexed: Vec<(String, Arc<Analysis>)> = self
            .disk
            .iter()
            .filter(|(uri, _)| {
                scope
                    .as_ref()
                    .is_none_or(|dir| uri_to_path(uri).is_some_and(|p| p.starts_with(dir)))
            })
            .map(|(uri, a)| (uri.clone(), a.clone()))
            .collect();
        let mut stale = Vec::new();
        for (uri, a) in indexed {
            let now = self.external_prefixes_as(&uri, a.parsed.syntax);
            if !Arc::ptr_eq(&now, &a.external)
                && *now != *a.external
                && let Some(path) = uri_to_path(&uri)
            {
                stale.push(path);
            }
        }
        stale
    }

    /// The prefixes.ttl `uri` was saved or closed with unsaved edits: the
    /// indexed files it serves whose prefixes differ now (all of them when it
    /// has no file path).
    pub(super) fn prefix_file_changed(&mut self, uri: &str) -> Vec<PathBuf> {
        match uri_to_path(uri) {
            Some(file) => self.prefixes_changed(Some(&file)),
            None => self.prefixes_changed(None),
        }
    }

    fn is_open_path(&self, path: &Path) -> bool {
        path_to_uri(path).is_some_and(|u| self.open.contains_key(&u))
    }
}

/// The prefixes.ttl that supplies prefixes to files in `dir` (searching up to
/// the innermost workspace root), if any.
pub(crate) fn find_prefix_file(roots: &[PathBuf], dir: &Path) -> Option<PathBuf> {
    let root = roots
        .iter()
        .filter(|r| dir.starts_with(r))
        .max_by_key(|r| r.as_os_str().len());
    // A document on a network share is looked up there only if the workspace is.
    if crate::imports::is_network_path(dir)
        && !roots.iter().any(|r| crate::imports::is_network_path(r))
    {
        return None;
    }
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let candidate = d.join(PREFIX_FILE);
        // Inside the workspace the file must really be there: a link to a
        // file outside is not read.
        if candidate.is_file()
            && (root.is_none() || crate::imports::fs_policy_path(&candidate, roots).is_ok())
        {
            return Some(candidate);
        }
        if root.is_none_or(|r| d == r.as_path()) {
            break;
        }
        cur = d.parent();
    }
    None
}
