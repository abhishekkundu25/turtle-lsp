//! The disk index: scanning the workspace roots, parsing the files found,
//! and keeping the index current as files change on disk.

#[cfg(test)]
use super::INDEX_PASSES;
use super::contexts::DocContexts;
use super::kinds::is_watched_file;
use super::{
    Change, Doc, Effects, MAX_FILE_BYTES, MAX_FILES, PREFIX_FILE, SKIP_DIRS, Workspace, analyze,
    is_catalog, is_rdf_file, is_sparql_path, path_to_uri, prepared, uri_to_path,
};
use crate::analysis::Analysis;
use crate::jsonld::ContextLoader;
use crate::knowledge::{DocFacts, Source};
use crate::syntax::{PrefixMap, Syntax};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A file's modification time and size: equal stamps mean it was not written
/// since.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) struct Stamp {
    modified: Option<std::time::SystemTime>,
    pub(super) len: u64,
}

/// The text of a file on disk as an editor holds it: UTF-8, without a
/// leading byte order mark, which VS Code and Neovim take off when they
/// open it. Positions in a closed file are then the ones the client sees
/// once it opens it, and a Turtle parser need not read the mark.
pub(crate) fn read_text(path: &Path) -> Option<String> {
    let mut text = std::fs::read_to_string(path).ok()?;
    if text.starts_with('\u{feff}') {
        text.drain(..'\u{feff}'.len_utf8());
    }
    Some(text)
}

pub(super) fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// The files and folders that changed on disk while the initial index was
/// being built: what that index read may be out of date for them.
#[derive(Default)]
pub(super) struct Touched {
    uris: rustc_hash::FxHashSet<String>,
    dirs: Vec<PathBuf>,
}

impl Workspace {
    // ---- disk index ----

    /// The RDF files under `roots` (see [`scan_roots_matching`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn scan_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
        scan_roots_matching(roots, is_rdf_file).paths
    }

    /// Whether the file at `path` belongs in the disk index, as the scan
    /// reads the roots: an RDF file below a root, in no hidden or skipped
    /// ([`SKIP_DIRS`]) directory, of at most [`MAX_FILE_BYTES`]. The caller
    /// keeps the index within [`MAX_FILES`] files. The scan, a file that
    /// changed on disk and a document that was closed all ask this.
    pub(super) fn fits(&self, path: &Path) -> bool {
        is_rdf_file(path) && self.scan_reaches(path)
    }

    /// Whether the scan of the roots reaches the file at `path`: below a
    /// root, in no hidden or skipped directory, through no symbolic link
    /// (the scan follows none, so a link in the workspace to a file or
    /// folder outside it is not read), and a file of at most
    /// [`MAX_FILE_BYTES`].
    pub(super) fn scan_reaches(&self, path: &Path) -> bool {
        self.roots.iter().any(|root| {
            let Ok(rel) = path.strip_prefix(root) else {
                return false;
            };
            let mut at = root.clone();
            Self::below(rel, false)
                && rel.components().all(|c| {
                    at.push(c);
                    std::fs::symlink_metadata(&at).is_ok_and(|m| !m.file_type().is_symlink())
                })
        }) && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES)
    }

    /// Whether `path` is somewhere the index looks: below a root, in no
    /// hidden or skipped directory. Changes elsewhere (`node_modules`, a
    /// file outside the workspace) are no concern of the server's.
    pub(crate) fn watched(&self, path: &Path) -> bool {
        self.in_area(path, false)
    }

    /// [`Workspace::watched`] for a directory: the directory itself counts
    /// too.
    pub(crate) fn watched_dir(&self, path: &Path) -> bool {
        self.in_area(path, true)
    }

    fn in_area(&self, path: &Path, include_last: bool) -> bool {
        self.roots.iter().any(|root| {
            path.strip_prefix(root)
                .is_ok_and(|rel| Self::below(rel, include_last))
        })
    }

    /// Whether `rel`, a path below a root, passes through no hidden or
    /// skipped directory and does not climb out (`..`). With
    /// `include_last`, its last part is a directory and counts too.
    fn below(rel: &Path, include_last: bool) -> bool {
        use std::path::Component;
        let parts: Vec<Component> = rel.components().collect();
        let dirs = if include_last {
            &parts[..]
        } else {
            &parts[..parts.len().saturating_sub(1)]
        };
        dirs.iter().all(|c| match c {
            Component::Normal(name) => !skipped_dir(&name.to_string_lossy()),
            Component::CurDir => true,
            _ => false,
        })
    }

    /// Parse files in parallel and add them to the disk index.
    pub(crate) fn index_paths(&mut self, paths: Vec<PathBuf>) {
        #[cfg(test)]
        INDEX_PASSES.with(|c| c.set(c.get() + 1));
        let parsed = self.parse_paths(paths);
        self.add_disk(parsed, true);
    }

    /// Add analyses parsed elsewhere (the initial index, built on another
    /// thread). With `replace` false, files indexed meanwhile are kept.
    pub(crate) fn add_disk(&mut self, files: Vec<Parsed>, replace: bool) {
        let files: Vec<(String, Arc<Analysis>, DocFacts, Vec<String>)> = files
            .into_par_iter()
            .filter(|f| !self.open.contains_key(&f.uri))
            .filter(|f| replace || !self.disk.contains_key(&f.uri))
            .map(|f| {
                let (a, facts) = prepared(f.analysis);
                (f.uri, Arc::new(a), facts, f.asked)
            })
            .collect();
        for (uri, a, facts, asked) in files {
            self.record_uses(&uri, asked);
            self.knowledge
                .set_prepared(&uri, Source::Disk, a.clone(), facts);
            self.disk.insert(uri, a);
        }
    }

    /// The initial index is being built elsewhere: remember what changes on
    /// disk meanwhile, so that [`Workspace::merge_initial_index`] does not
    /// undo it with what the index read before.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn expect_initial_index(&mut self) {
        self.touched = Some(Touched::default());
    }

    /// Add the initial index, built elsewhere, without the files and
    /// folders that changed on disk since [`Workspace::expect_initial_index`]
    /// (those are as the changes left them), and files indexed meanwhile.
    pub(crate) fn merge_initial_index(&mut self, files: Vec<Parsed>) {
        let touched = self.touched.take().unwrap_or_default();
        let files = files
            .into_iter()
            .filter(|Parsed { uri, .. }| {
                !touched.uris.contains(uri)
                    && !uri_to_path(uri)
                        .is_some_and(|p| touched.dirs.iter().any(|d| p.starts_with(d)))
            })
            .collect();
        self.add_disk(files, false);
    }

    fn touch(&mut self, uri: &str) {
        if let Some(t) = &mut self.touched {
            t.uris.insert(uri.to_string());
        }
    }

    /// The prefixes to parse each of `paths` with.
    pub(crate) fn prefix_jobs(&mut self, paths: Vec<PathBuf>) -> Vec<ParseJob> {
        paths
            .into_iter()
            .filter_map(|path| {
                let uri = path_to_uri(&path)?;
                let external = self.external_prefixes_for(&uri);
                let was = self.disk.get(&uri).cloned();
                Some(ParseJob {
                    uri,
                    path,
                    external,
                    was,
                    changes: self.context_changes,
                })
            })
            .collect()
    }

    /// What parsing files needs of the workspace, to do it on another thread.
    pub(crate) fn reader(&self) -> Reader {
        Reader {
            roots: self.roots.clone(),
            contexts: self.contexts(),
        }
    }

    /// Parse files in parallel, without adding them to anything.
    pub(crate) fn parse_paths(&mut self, paths: Vec<PathBuf>) -> Vec<Parsed> {
        let jobs = self.prefix_jobs(paths);
        let contexts = self.contexts();
        parse_files(&self.open, &self.roots, &*contexts, jobs)
            .into_iter()
            .map(|r| r.file)
            .collect()
    }

    /// Replace indexed files with their analyses parsed again elsewhere
    /// ([`Reader::parse`]), but only those still closed, still indexed as
    /// they were when the parse began, and parsed with the prefixes they
    /// have now. A file that asked for contexts while one changed (a fetch
    /// arrived, a context file was saved) is parsed again here: the change
    /// found no record of its new uses. How many were replaced.
    pub(crate) fn add_reparsed(&mut self, files: Vec<Reparsed>) -> usize {
        let mut current = Vec::new();
        let mut again = Vec::new();
        for Reparsed { file, was, changes } in files {
            let uri = &file.uri;
            let unchanged = match (self.disk.get(uri), &was) {
                (Some(now), Some(was)) => Arc::ptr_eq(now, was),
                _ => false,
            };
            if self.open.contains_key(uri)
                || !unchanged
                || *self.external_prefixes_as(uri, file.analysis.parsed.syntax)
                    != *file.analysis.external
            {
                continue;
            }
            if changes != self.context_changes && !file.asked.is_empty() {
                again.extend(uri_to_path(uri));
            } else {
                current.push(file);
            }
        }
        let n = current.len() + again.len();
        self.add_disk(current, true);
        if !again.is_empty() {
            self.index_paths(again);
        }
        n
    }

    /// Bring the index up to date with the file at `path`, which changed on
    /// disk (it is not open): parse it again, or drop it when it is gone or
    /// no longer belongs in it (`Workspace::fits`). Whether the index
    /// changed.
    pub(crate) fn reindex_disk_file(&mut self, path: &Path) -> bool {
        self.reindex_disk_files(&[path.to_path_buf()])
    }

    /// [`Workspace::reindex_disk_file`] for the files of one change: those
    /// that are indexable are parsed together, in one parallel pass, and
    /// the others dropped. What is read is the files as they are now, so a
    /// path listed twice is one file, and the limit of [`MAX_FILES`] counts the
    /// files this adds. Whether the index changed.
    pub(crate) fn reindex_disk_files(&mut self, paths: &[PathBuf]) -> bool {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut room = MAX_FILES.saturating_sub(self.disk.len());
        let mut read = Vec::new();
        let mut gone = Vec::new();
        for path in paths {
            let Some(uri) = path_to_uri(path) else {
                continue;
            };
            if !seen.insert(uri.clone()) {
                continue;
            }
            self.touch(&uri);
            let new = !self.disk.contains_key(&uri);
            if self.fits(path) && (!new || room > 0) {
                room -= usize::from(new);
                read.push(path.clone());
            } else {
                gone.push(uri);
            }
        }
        let mut changed = !read.is_empty();
        if changed {
            self.index_paths(read);
        }
        for uri in gone {
            changed |= self.forget_disk(&uri);
        }
        changed
    }

    /// Drop the indexed copy of `uri`. Whether there was one.
    fn forget_disk(&mut self, uri: &str) -> bool {
        let had = self.disk.remove(uri).is_some();
        if !self.open.contains_key(uri) {
            self.record_uses(uri, Vec::new());
        }
        self.sync(uri);
        had
    }

    /// The directories `dirs` were deleted or renamed: drop the files indexed
    /// below them, in one pass over the index. Whether there were any.
    pub(crate) fn remove_under_any(&mut self, dirs: &[PathBuf]) -> bool {
        if dirs.is_empty() {
            return false;
        }
        if let Some(t) = &mut self.touched {
            t.dirs.extend(dirs.iter().cloned());
        }
        let dirs: rustc_hash::FxHashSet<&Path> = dirs.iter().map(PathBuf::as_path).collect();
        if let Some(files) = &mut self.sparql_files {
            files.retain(|p, _| !p.ancestors().skip(1).any(|a| dirs.contains(a)));
        }
        let gone: Vec<String> = self
            .disk
            .keys()
            .filter(|uri| {
                uri_to_path(uri).is_some_and(|p| p.ancestors().skip(1).any(|a| dirs.contains(a)))
            })
            .cloned()
            .collect();
        for uri in &gone {
            self.forget_disk(uri);
        }
        !gone.is_empty()
    }

    /// Files changed on disk, as the client's watchers report. JSON-LD
    /// context files count wherever they are (a context may name a skipped
    /// or hidden folder); for the index, only what the scan would have
    /// looked at is of concern: nothing in a skipped or hidden directory,
    /// nothing outside the roots. Catalogs are the caller's (imports), and
    /// so is the settings file; an `.owl` or `.rdf` file is indexed here and
    /// is the caller's too, as a catalog target.
    ///
    /// A changed prefixes.ttl is taken first, so the files of the same
    /// change are parsed once, with the new prefixes; the indexed files it
    /// serves and that did not change are left to parse again.
    pub(crate) fn files_changed(&mut self, changes: &[(PathBuf, Change)]) -> Effects {
        let mut fx = Effects::default();
        let mut prefix_files: Vec<PathBuf> = Vec::new();
        let mut gone_dirs: Vec<PathBuf> = Vec::new();
        let mut disk_changed = false;
        let mut sparql_paths: Vec<PathBuf> = Vec::new();
        let mut reindex: Vec<PathBuf> = Vec::new();
        for (path, change) in changes {
            // A deleted path that no watched kind of file names may be a
            // folder, and the files indexed below it are gone with it.
            if *change == Change::Deleted && !is_watched_file(path) {
                if self.watched_dir(path) {
                    gone_dirs.push(path.clone());
                }
                continue;
            }
            if is_catalog(path) {
                continue;
            }
            let key = path_to_uri(path).unwrap_or_default();
            let open = self.open.contains_key(&key);
            // A JSON-LD context file on disk (the open copy wins).
            if !open && self.context_changed(&key, true) {
                disk_changed = true;
            }
            if !self.watched(path) {
                continue;
            }
            if is_sparql_path(path) {
                // Read on demand, never indexed; but a closed query is diagnosed.
                sparql_paths.push(path.clone());
                continue;
            }
            if !is_rdf_file(path) || open {
                // An open prefixes.ttl was handled on save.
                continue;
            }
            if path.file_name().is_some_and(|f| f == PREFIX_FILE) {
                prefix_files.push(path.clone());
            }
            fx.on_disk.push(path.clone());
            reindex.push(path.clone());
        }
        // The new prefixes first: the files changed with them are parsed
        // below with those, and only the others are left to parse again.
        let mut stale = Vec::new();
        for file in &prefix_files {
            stale.extend(self.prefixes_changed(Some(file)));
        }
        // The files of one change are parsed together.
        disk_changed |= self.reindex_disk_files(&reindex);
        // The files below a deleted folder are no longer shown: knowledge
        // changed, and with it every closed file.
        disk_changed |= self.remove_under_any(&gone_dirs);
        if !sparql_paths.is_empty() {
            self.sparql_files_changed(&sparql_paths);
            fx.closed_files = true;
        }
        let parsed: rustc_hash::FxHashSet<&PathBuf> = reindex.iter().collect();
        stale.retain(|p| !parsed.contains(p));
        stale.sort();
        stale.dedup();
        fx.reparse = stale;
        if !prefix_files.is_empty() || disk_changed {
            fx.add(Effects {
                data: true,
                ..Effects::knowledge()
            });
        }
        fx
    }
}

/// A file to parse, with the prefixes to parse it with, the indexed copy it
/// replaces, if any, and [`Workspace::context_changes`] when it was made.
pub(crate) struct ParseJob {
    uri: String,
    path: PathBuf,
    external: Arc<PrefixMap>,
    was: Option<Arc<Analysis>>,
    changes: u64,
}

/// A file parsed, and the context URLs its analysis asked for: recorded by
/// the workspace that keeps the analysis ([`Workspace::add_disk`]).
pub(crate) struct Parsed {
    uri: String,
    analysis: Analysis,
    asked: Vec<String>,
}

/// A file parsed again, and what its job knew: the indexed copy it was
/// meant to replace, and [`Workspace::context_changes`] then.
pub(crate) struct Reparsed {
    file: Parsed,
    was: Option<Arc<Analysis>>,
    changes: u64,
}

/// What parsing files needs of a workspace, apart from the workspace: it can
/// be handed to another thread ([`Workspace::reader`]).
pub(crate) struct Reader {
    roots: Vec<PathBuf>,
    contexts: Arc<dyn ContextLoader>,
}

impl Reader {
    /// Parse files in parallel, without adding them to anything. Open
    /// documents are not seen: JSON-LD contexts come from the files.
    pub(crate) fn parse(&self, jobs: Vec<ParseJob>) -> Vec<Reparsed> {
        parse_files(&FxHashMap::default(), &self.roots, &*self.contexts, jobs)
    }
}

fn parse_files(
    open: &FxHashMap<String, Doc>,
    roots: &[PathBuf],
    contexts: &dyn ContextLoader,
    jobs: Vec<ParseJob>,
) -> Vec<Reparsed> {
    jobs.into_par_iter()
        .filter_map(
            |ParseJob {
                 uri,
                 path,
                 external,
                 was,
                 changes,
             }| {
                if std::fs::metadata(&path).ok()?.len() > MAX_FILE_BYTES {
                    return None;
                }
                let text = read_text(&path)?;
                let syntax = Syntax::of(None, &uri, &text);
                // The job's prefixes are by name: a text in a syntax without
                // prefixes gets none, as when it is open.
                let external = if syntax.has_prefixes() {
                    external
                } else {
                    Arc::new(PrefixMap::default())
                };
                let loader = DocContexts::new(open, roots, contexts);
                let analysis = analyze(uri.clone(), &text, external, false, syntax, &loader);
                let file = Parsed {
                    uri,
                    analysis,
                    asked: loader.asked(syntax),
                };
                Some(Reparsed { file, was, changes })
            },
        )
        .collect()
}

/// A directory the index does not look into: hidden, or one of [`SKIP_DIRS`].
fn skipped_dir(name: &str) -> bool {
    name.starts_with('.') || SKIP_DIRS.contains(&name)
}

/// What a scan of the roots found.
#[derive(Debug, Default)]
pub(crate) struct Scan {
    /// The files `wanted` accepted, in path order.
    pub(crate) paths: Vec<PathBuf>,
    /// More matching files exist than the cap allows; the walk stopped at the
    /// cap, so `paths` is the first part of the path order.
    pub(crate) truncated: bool,
    /// Matching files over [`MAX_FILE_BYTES`], left out. Not complete when
    /// the scan was `truncated`.
    pub(crate) too_large: Vec<PathBuf>,
}

impl IntoIterator for Scan {
    type Item = PathBuf;
    type IntoIter = std::vec::IntoIter<PathBuf>;

    fn into_iter(self) -> Self::IntoIter {
        self.paths.into_iter()
    }
}

/// The files under `roots` that `wanted` accepts, skipping hidden and
/// [`SKIP_DIRS`] directories and files over [`MAX_FILE_BYTES`], at most
/// [`MAX_FILES`] of them.
pub(crate) fn scan_roots_matching(roots: &[PathBuf], wanted: fn(&Path) -> bool) -> Scan {
    scan_roots_capped(roots, wanted, MAX_FILES)
}

/// [`scan_roots_matching`] with a cap of `max_files`. One walk, in path order
/// (the entries of each folder sorted by name, a folder visited in place), so
/// what a cap leaves out does not depend on the file system's order.
pub(crate) fn scan_roots_capped(
    roots: &[PathBuf],
    wanted: fn(&Path) -> bool,
    max_files: usize,
) -> Scan {
    /// The files and folders in `dir` by name: (name, path, is a folder).
    /// Links and what cannot be read are left out.
    fn entries(dir: &Path) -> std::vec::IntoIter<(std::ffi::OsString, PathBuf, bool)> {
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let ft = e.file_type().ok()?;
                (ft.is_dir() || ft.is_file()).then(|| (e.file_name(), e.path(), ft.is_dir()))
            })
            .collect();
        out.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        out.into_iter()
    }

    let mut scan = Scan::default();
    let mut roots = roots.to_vec();
    roots.sort();
    roots.dedup();
    let mut seen: rustc_hash::FxHashSet<PathBuf> = rustc_hash::FxHashSet::default();
    'roots: for root in roots {
        let mut stack = vec![entries(&root)];
        while let Some(top) = stack.last_mut() {
            let Some((name, path, is_dir)) = top.next() else {
                stack.pop();
                continue;
            };
            if is_dir {
                if !skipped_dir(&name.to_string_lossy()) {
                    stack.push(entries(&path));
                }
            } else if wanted(&path) {
                let Ok(meta) = std::fs::metadata(&path) else {
                    continue;
                };
                if meta.len() > MAX_FILE_BYTES {
                    scan.too_large.push(path);
                } else if seen.contains(&path) {
                    // Found under another root too.
                } else if scan.paths.len() >= max_files {
                    scan.truncated = true;
                    break 'roots;
                } else {
                    seen.insert(path.clone());
                    scan.paths.push(path);
                }
            }
        }
    }
    scan.paths.sort();
    scan.too_large.sort();
    scan.too_large.dedup();
    scan
}
