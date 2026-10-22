//! Open documents, indexed files on disk, and project prefixes.
//!
//! Project prefixes: the nearest `prefixes.ttl` in the file's directory or an
//! ancestor (up to the workspace root) supplies prefixes to every Turtle and
//! TriG file below it, plus any `prefixes` given in the client's
//! initializationOptions. N-Triples and N-Quads files have no prefixes.
//! SPARQL documents get the same prefixes as Turtle files.
//!
//! Document kinds ([`DocKind`]): an open document is RDF (with a [`Syntax`])
//! or SPARQL. RDF documents feed the knowledge index; SPARQL documents never
//! do, are never validated against SHACL shapes, and closing one leaves the
//! index as it was. SPARQL files on disk are not read at startup: features
//! that look across the workspace's queries (references and rename of a
//! term) ask for them with [`Workspace::sparql_documents`], which reads them
//! on the first ask and keeps them current from the watcher's events.
//!
//! JSON-LD contexts: a `file:` context URL is served from the open document
//! at that URL, else read from disk, but only inside the workspace roots (as
//! local imports are); other URLs go to the context loader (the imports
//! cache, the bundled contexts, the network when allowed). Which contexts
//! each JSON-LD analysis loaded is recorded, so a change to one (an edit, a
//! save, a fetch that arrived) analyzes the documents that use it again
//! ([`Workspace::context_changed`]).
//!
//! File kinds: which paths are RDF, SPARQL, imports sources or JSON is one
//! table, in [`kinds`].
//!
//! Effects: every method that changes what the workspace holds from outside
//! (a document opened, edited, saved or closed, files changed on disk, the
//! settings prefixes, a context fetched) returns [`Effects`], what the change
//! means beyond itself: which open documents to diagnose again, whether
//! knowledge outside any one document changed, whether SHACL is due, which
//! indexed files to parse again. The workspace decides that; the caller (the
//! server) only acts on it.

use crate::analysis::{Analysis, Encoding, SparqlAnalysis, offset_in};
use crate::jsonld::loader::{self, Bundled};
use crate::jsonld::{ContextLoader, LoadError, RemoteDocument};
use crate::knowledge::{DocFacts, Knowledge, Source};
use crate::server::worker::catch;
use crate::syntax::{PrefixMap, Severity, Span, Syntax, SyntaxError};
use line_index::LineIndex;
use lsp_types::TextDocumentContentChangeEvent;
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub mod kinds;

use kinds::is_watched_file;
pub use kinds::{is_import_source, is_rdf_file, is_sparql_name, is_sparql_path};

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

/// What an open document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocKind {
    /// Turtle, TriG, N-Triples, N-Quads or JSON-LD.
    Rdf(Syntax),
    /// A SPARQL query or update.
    Sparql,
}

impl From<Syntax> for DocKind {
    fn from(s: Syntax) -> DocKind {
        DocKind::Rdf(s)
    }
}

impl DocKind {
    /// SPARQL when the client says `sparql`, or when it sends no language id,
    /// one this server does not know, or the generic `turtle`, and the name
    /// is a SPARQL file's ([`is_sparql_name`]). Otherwise RDF, as
    /// [`Syntax::detect`] decides.
    pub fn detect(language_id: Option<&str>, uri: &str) -> DocKind {
        if language_id.is_some_and(|id| id.eq_ignore_ascii_case("sparql")) {
            return DocKind::Sparql;
        }
        let generic = language_id
            .and_then(Syntax::from_language_id)
            .is_none_or(|s| s == Syntax::Turtle);
        if generic && is_sparql_name(uri) {
            return DocKind::Sparql;
        }
        DocKind::Rdf(Syntax::detect(language_id, uri))
    }
}

/// Which open documents a change makes due for diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Dirty {
    #[default]
    None,
    /// This document (the one edited).
    One(String),
    /// Every open document.
    AllOpen,
}

/// How a file on disk changed, as the client's watchers report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Created,
    Changed,
    Deleted,
}

/// What a change to the workspace means beyond itself, for the caller to act
/// on (see the module doc). A plain value: a caller with nothing to act on
/// (`batch`, tests) drops it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Effects {
    /// The open documents whose diagnostics are due again.
    pub dirty: Dirty,
    /// Knowledge outside any one document changed (prefixes, the disk index,
    /// a context): every open document is due again, whatever `dirty` says,
    /// and so is every closed file shown by workspace diagnostics.
    pub knowledge: bool,
    /// The data SHACL validates changed: validate again.
    pub shacl: bool,
    /// Indexed files whose prefixes changed, to parse again (off the main
    /// loop if the caller can) with [`Workspace::prefix_jobs`].
    pub reparse: Vec<PathBuf>,
    /// RDF files that changed on disk and are not open, in the order
    /// reported: workspace diagnostics take them first, or forget them when
    /// they are gone.
    pub on_disk: Vec<PathBuf>,
    /// SPARQL files changed on disk: the closed files' diagnostics are due
    /// again, though knowledge did not change.
    pub closed_files: bool,
}

impl Effects {
    /// Knowledge outside any one document changed: every open document is due.
    pub fn knowledge() -> Effects {
        Effects {
            dirty: Dirty::AllOpen,
            knowledge: true,
            ..Effects::default()
        }
    }

    fn one(uri: &str) -> Effects {
        Effects {
            dirty: Dirty::One(uri.to_string()),
            ..Effects::default()
        }
    }

    /// Add what `other` says. Two different documents are every open one.
    pub fn add(&mut self, other: Effects) {
        self.dirty = match (std::mem::take(&mut self.dirty), other.dirty) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (Dirty::One(a), Dirty::One(b)) if a == b => Dirty::One(a),
            _ => Dirty::AllOpen,
        };
        self.knowledge |= other.knowledge;
        self.shacl |= other.shacl;
        self.reparse.extend(other.reparse);
        self.on_disk.extend(other.on_disk);
        self.closed_files |= other.closed_files;
    }
}

/// Whether `uri` names a prefixes.ttl.
fn is_prefix_uri(uri: &str) -> bool {
    uri.ends_with(&format!("/{PREFIX_FILE}"))
}

pub struct Doc {
    pub text: String,
    pub version: i32,
    pub kind: DocKind,
    analysis: Option<Arc<Analysis>>,
    /// For a SPARQL document: its parse, until the next edit.
    query: Option<Arc<SparqlAnalysis>>,
    /// A cached copy of an import: the import already contributes its facts.
    cached_import: bool,
}

#[derive(Default)]
pub struct Workspace {
    roots: Vec<PathBuf>,
    open: FxHashMap<String, Doc>,
    disk: FxHashMap<String, Arc<Analysis>>,
    settings_prefixes: PrefixMap,
    /// Facts from every analysis: the open document's when it is open, else
    /// the disk copy's. Kept current by every method that changes either.
    knowledge: Knowledge,
    /// Directory -> the prefixes.ttl that serves it (None = no file found).
    prefix_dirs: FxHashMap<PathBuf, Option<PathBuf>>,
    /// A prefixes.ttl -> its prefixes over the settings prefixes, with the
    /// file's stamp when they were read. Every directory the file serves
    /// shares the one map.
    prefix_files: FxHashMap<PathBuf, PrefixEntry>,
    /// The settings prefixes alone: the map of a file no prefixes.ttl serves.
    settings_map: Option<Arc<PrefixMap>>,
    /// The same for documents without a file path (the browser build): URI
    /// directory -> prefixes from the nearest open prefixes.ttl.
    uri_prefix_cache: FxHashMap<String, Arc<PrefixMap>>,
    /// Where fetched imports are cached (canonical), if there is a cache.
    import_cache: Option<PathBuf>,
    /// Where JSON-LD documents' remote contexts come from (None: the bundled
    /// contexts only).
    contexts: Option<Arc<dyn ContextLoader>>,
    /// Document URI -> the context URLs its last JSON-LD analysis asked
    /// for. Shared with the thread that builds the initial index.
    context_uses: ContextUses,
    /// What changed on disk since the workspace was created, kept until the
    /// initial index is merged (see [`Workspace::expect_initial_index`]).
    touched: Option<Touched>,
    /// The SPARQL files under the roots, read on first use and kept current
    /// (see [`Workspace::sparql_documents`]). `None` until then, and after
    /// anything that changes how they are parsed.
    sparql_files: Option<std::collections::BTreeMap<PathBuf, SparqlFile>>,
    /// No one reports file changes (the client registered no file watchers):
    /// [`Workspace::sparql_documents`] lists the folders again on each call.
    rescan_sparql: bool,
}

/// A SPARQL file on disk, as read.
struct SparqlFile {
    uri: String,
    /// The file's stamp when it was read.
    stamp: Option<Stamp>,
    analysis: Arc<SparqlAnalysis>,
}

/// A prefixes.ttl's prefixes, as read.
struct PrefixEntry {
    stamp: Option<Stamp>,
    /// Read from the open document, not from the file.
    from_open: bool,
    map: Arc<PrefixMap>,
}

/// A file's modification time and size: equal stamps mean it was not written
/// since.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    modified: Option<std::time::SystemTime>,
    len: u64,
}

fn stamp_of(path: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(path).ok()?;
    Some(Stamp {
        modified: m.modified().ok(),
        len: m.len(),
    })
}

/// The files and folders that changed on disk while the initial index was
/// being built: what that index read may be out of date for them.
#[derive(Default)]
struct Touched {
    uris: rustc_hash::FxHashSet<String>,
    dirs: Vec<PathBuf>,
}

type ContextUses = Arc<Mutex<FxHashMap<String, Vec<String>>>>;

/// Record the contexts `uri`'s analysis asked for.
fn record_contexts(uses: &ContextUses, uri: &str, asked: Vec<String>) {
    let Ok(mut uses) = uses.lock() else { return };
    if asked.is_empty() {
        uses.remove(uri);
    } else {
        uses.insert(uri.to_string(), asked);
    }
}

/// What a workspace hands to another one parsing files for it (the initial
/// index, built on another thread): its context loader, and the record of
/// which documents use which contexts.
#[derive(Clone)]
pub struct SharedContexts {
    loader: Option<Arc<dyn ContextLoader>>,
    uses: ContextUses,
}

/// The contexts of one JSON-LD analysis: a `file:` URL is the open
/// document's text, else the file's, inside the workspace roots only; other
/// URLs go to `web`. Every URL asked for is recorded.
struct DocContexts<'a> {
    open: &'a FxHashMap<String, Doc>,
    roots: &'a [PathBuf],
    web: &'a dyn ContextLoader,
    asked: Mutex<Vec<String>>,
}

impl<'a> DocContexts<'a> {
    fn new(
        open: &'a FxHashMap<String, Doc>,
        roots: &'a [PathBuf],
        web: &'a dyn ContextLoader,
    ) -> DocContexts<'a> {
        DocContexts {
            open,
            roots,
            web,
            asked: Mutex::default(),
        }
    }

    /// Record in `uses` what `uri`'s analysis asked for, when it is JSON-LD.
    fn record(self, uses: &ContextUses, uri: &str, syntax: Syntax) {
        if syntax == Syntax::JsonLd {
            record_contexts(uses, uri, self.asked.into_inner().unwrap_or_default());
        }
    }

    fn file(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        let key = normalize_uri(url);
        if let Some(d) = self.open.get(&key) {
            return Ok(RemoteDocument {
                url: url.to_string(),
                text: Arc::from(d.text.as_str()),
            });
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // Nothing asks the file system about the file (a host, a share,
            // a path outside the roots) before the roots have said yes.
            let path = crate::imports::fs_policy(url, self.roots).map_err(LoadError::Failed)?;
            loader::read_context_file(&path).map(|text| RemoteDocument {
                url: url.to_string(),
                text: text.into(),
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = self.roots;
            Err(LoadError::NotFound)
        }
    }
}

impl ContextLoader for DocContexts<'_> {
    fn load(&self, url: &str) -> Result<RemoteDocument, LoadError> {
        if let Ok(mut asked) = self.asked.lock()
            && !asked.iter().any(|u| u == url)
        {
            asked.push(url.to_string());
        }
        if url.starts_with("file:") {
            self.file(url)
        } else {
            self.web.load(url)
        }
    }
}

/// The file a `file:` URL names. None in the browser build, which has no files.
pub fn file_path_of(u: &url::Url) -> Option<PathBuf> {
    #[cfg(not(target_arch = "wasm32"))]
    return u.to_file_path().ok();
    #[cfg(target_arch = "wasm32")]
    {
        let _ = u;
        None
    }
}

/// The `file:` URL of `path` (of the directory `path` when `dir`). None in
/// the browser build.
pub fn file_url_of(path: &Path, dir: bool) -> Option<url::Url> {
    #[cfg(not(target_arch = "wasm32"))]
    return if dir {
        url::Url::from_directory_path(path).ok()
    } else {
        url::Url::from_file_path(path).ok()
    };
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (path, dir);
        None
    }
}

/// The file name a `file:` URI ends in, decoded, without reading any file
/// system (the browser build has none).
pub fn file_uri_name(uri: &str) -> Option<String> {
    let u = url::Url::parse(uri).ok().filter(|u| u.scheme() == "file")?;
    let last = u.path_segments()?.next_back().filter(|s| !s.is_empty())?;
    let bytes = last.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok())
            .and_then(|h| u8::from_str_radix(h, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).ok()
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let path = file_path_of(&url::Url::parse(uri).ok()?)?;
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
    file_url_of(path, false).map(|u| u.to_string())
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

#[cfg(test)]
thread_local! {
    /// How many times this thread has parsed files into the disk index
    /// ([`Workspace::index_paths`]), to tell a batch from one by one.
    pub(crate) static INDEX_PASSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Text of a document that makes analysis panic, in tests (see
/// [`analyze`]).
#[cfg(test)]
pub(crate) const PANIC_IN_ANALYSIS: &str = "turtle-lsp-test: panic in analysis";
/// Text of a document whose facts cannot be computed, in tests (see
/// [`prepared`]).
#[cfg(test)]
pub(crate) const PANIC_IN_FACTS: &str = "turtle-lsp-test: panic in facts";

/// [`Analysis::with_loader`], but a panic in it is the analysis of a document
/// that failed ([`failed_analysis`]) rather than the end of the server: one
/// document's bug must not take every other document down.
fn analyze(
    uri: String,
    text: &str,
    external: Arc<PrefixMap>,
    keep_tokens: bool,
    syntax: Syntax,
    loader: &dyn ContextLoader,
) -> Analysis {
    let attempt = catch("analysis", || {
        #[cfg(test)]
        if text.contains(PANIC_IN_ANALYSIS) {
            panic!("injected analysis panic");
        }
        Analysis::with_loader(
            uri.clone(),
            text,
            external.clone(),
            keep_tokens,
            syntax,
            loader,
        )
    });
    attempt.unwrap_or_else(|msg| failed_analysis(uri, syntax, external, &msg))
}

/// What a document whose analysis panicked is analyzed as: empty, with the
/// failure as its one error, so it shows as a diagnostic.
fn failed_analysis(uri: String, syntax: Syntax, external: Arc<PrefixMap>, msg: &str) -> Analysis {
    let mut a = Analysis::with_loader(uri, "", external, true, syntax, &Bundled);
    a.parsed.errors.push(SyntaxError::new(
        Span::default(),
        format!(
            "{msg}. The rest of the server keeps working; please report this file at https://github.com/abhishekkundu25/turtle-lsp/issues"
        ),
        Severity::Error,
        "internal-error",
    ));
    a
}

/// `a` with its facts for the knowledge index. When the facts cannot be
/// computed, a [`failed_analysis`] stands in for it.
fn prepared(a: Analysis) -> (Analysis, DocFacts) {
    let attempt = catch("indexing", || {
        #[cfg(test)]
        if a.text.contains(PANIC_IN_FACTS) {
            panic!("injected facts panic");
        }
        DocFacts::new(&a)
    });
    match attempt {
        Ok(facts) => (a, facts),
        Err(msg) => {
            let failed = failed_analysis(a.uri.clone(), a.parsed.syntax, a.external.clone(), &msg);
            let facts = DocFacts::new(&failed);
            (failed, facts)
        }
    }
}

impl Workspace {
    pub fn new(roots: Vec<PathBuf>, settings_prefixes: PrefixMap) -> Workspace {
        Workspace {
            roots,
            settings_prefixes,
            import_cache: crate::imports::import_cache_dir().map(|d| canonical(&d)),
            contexts: Some(loader::for_platform(false)),
            ..Default::default()
        }
    }

    /// Settle the one choice the server makes when it starts: with
    /// `rescan`, [`Workspace::sparql_documents`] lists the folders again on
    /// each call, because no one reports file changes.
    pub fn set_rescan_sparql(&mut self, rescan: bool) {
        self.rescan_sparql = rescan;
    }

    // ---- state ----

    /// The workspace folders, in the order the client gave them.
    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The prefixes the settings declare (`turtle.prefixes`).
    pub fn settings_prefixes(&self) -> &PrefixMap {
        &self.settings_prefixes
    }

    /// Replace the settings prefixes: every open document is analyzed again,
    /// and the indexed files whose prefixes differ now are to be parsed
    /// again.
    pub fn set_settings_prefixes(&mut self, prefixes: PrefixMap) -> Effects {
        self.settings_prefixes = prefixes;
        Effects {
            shacl: true,
            reparse: self.prefixes_changed(None),
            ..Effects::knowledge()
        }
    }

    /// Whether `uri` is an open document.
    pub fn is_open(&self, uri: &str) -> bool {
        self.open.contains_key(uri)
    }

    /// The open documents' URIs, in no particular order.
    pub fn open_uris(&self) -> impl Iterator<Item = &str> {
        self.open.keys().map(String::as_str)
    }

    /// The version the client gave the open document `uri`.
    pub fn version(&self, uri: &str) -> Option<i32> {
        self.open.get(uri).map(|d| d.version)
    }

    /// What the open document `uri` is.
    pub fn doc_kind(&self, uri: &str) -> Option<DocKind> {
        self.open.get(uri).map(|d| d.kind)
    }

    /// Whether `uri` has an indexed copy from disk. An open RDF document
    /// that is not a cached import has none: its open text replaces the
    /// copy until it is closed ([`Workspace::open_doc_as`]).
    pub fn is_indexed(&self, uri: &str) -> bool {
        self.disk.contains_key(uri)
    }

    /// The URIs of the indexed copies from disk, in no particular order.
    pub fn indexed_uris(&self) -> impl Iterator<Item = &str> {
        self.disk.keys().map(String::as_str)
    }

    /// How many files have an indexed copy from disk.
    #[cfg(test)]
    pub(crate) fn indexed_count(&self) -> usize {
        self.disk.len()
    }

    /// The indexed copy of `uri` from disk.
    #[cfg(test)]
    pub(crate) fn indexed_analysis(&self, uri: &str) -> Option<&Arc<Analysis>> {
        self.disk.get(uri)
    }

    /// Load JSON-LD contexts with [`loader::for_platform`]: the network is
    /// used only when `network` (`imports.network`, `--network`) is on.
    pub fn set_network(&mut self, network: bool) {
        self.set_context_loader(loader::for_platform(network));
    }

    /// Load JSON-LD contexts with `loader` from now on: open JSON-LD
    /// documents are analyzed again, and indexed ones that use a web
    /// context are parsed again.
    pub fn set_context_loader(&mut self, loader: Arc<dyn ContextLoader>) {
        self.contexts = Some(loader);
        for d in self.open.values_mut() {
            if d.kind == DocKind::Rdf(Syntax::JsonLd) {
                d.analysis = None;
            }
        }
        let on_web: Vec<PathBuf> = self
            .context_users(|url| !url.starts_with("file:"))
            .into_iter()
            .filter(|u| !self.open.contains_key(u) && self.disk.contains_key(u))
            .filter_map(|u| uri_to_path(&u))
            .collect();
        if !on_web.is_empty() {
            self.index_paths(on_web);
        }
    }

    fn contexts(&self) -> Arc<dyn ContextLoader> {
        self.contexts.clone().unwrap_or_else(|| Arc::new(Bundled))
    }

    /// The context loader and the record of context uses, for a workspace
    /// that parses files for this one ([`Workspace::use_shared_contexts`]).
    pub fn shared_contexts(&self) -> SharedContexts {
        SharedContexts {
            loader: self.contexts.clone(),
            uses: self.context_uses.clone(),
        }
    }

    /// Load contexts as the workspace that shared them does, and record
    /// which documents use which in its record.
    pub fn use_shared_contexts(&mut self, shared: SharedContexts) {
        self.contexts = shared.loader;
        self.context_uses = shared.uses;
    }

    /// The documents whose last analysis asked for a context URL `wanted`
    /// accepts.
    fn context_users(&self, wanted: impl Fn(&str) -> bool) -> Vec<String> {
        let Ok(uses) = self.context_uses.lock() else {
            return Vec::new();
        };
        uses.iter()
            .filter(|(_, urls)| urls.iter().any(|u| wanted(u)))
            .map(|(doc, _)| doc.clone())
            .collect()
    }

    /// The JSON-LD context at `url` was fetched in the background (or the
    /// fetch failed): the documents that use it are analyzed or parsed
    /// again.
    pub fn context_fetched(&mut self, url: &str) -> Effects {
        if self.context_changed(url, true) {
            Effects {
                shacl: true,
                ..Effects::knowledge()
            }
        } else {
            Effects::default()
        }
    }

    /// The context at `url` changed (an open context file was edited,
    /// saved or closed, a file changed on disk, a fetch arrived): the open
    /// documents that use it are analyzed again, and, with `disk`, the
    /// indexed ones are parsed again. Whether any document uses it.
    pub fn context_changed(&mut self, url: &str, disk: bool) -> bool {
        let key = normalize_uri(url);
        let users =
            self.context_users(|u| u == url || (u.starts_with("file:") && normalize_uri(u) == key));
        let mut paths = Vec::new();
        for u in &users {
            if let Some(d) = self.open.get_mut(u) {
                d.analysis = None;
            } else if disk && self.disk.contains_key(u) {
                paths.extend(uri_to_path(u));
            }
        }
        if !paths.is_empty() {
            self.index_paths(paths);
        }
        !users.is_empty()
    }

    /// An open document that is a cached copy of an import: analyzed, but
    /// neither registered with the knowledge index nor diagnosed.
    pub fn is_cached_import(&self, uri: &str) -> bool {
        self.open.get(uri).is_some_and(|d| d.cached_import)
    }

    // ---- documents ----

    /// The kind comes from the URI's extension.
    pub fn open_doc(&mut self, uri: String, text: String, version: i32) -> Effects {
        let kind = DocKind::detect(None, &uri);
        self.open_doc_as(uri, text, version, kind)
    }

    /// Open a document. The document is due for diagnostics; opening a
    /// prefixes.ttl or a context file is an edit of it ([`Workspace::change_doc`]).
    pub fn open_doc_as(
        &mut self,
        uri: String,
        text: String,
        version: i32,
        kind: impl Into<DocKind>,
    ) -> Effects {
        let kind = kind.into();
        let cached_import = kind != DocKind::Sparql
            && self.import_cache.as_ref().is_some_and(|cache| {
                uri_to_path(&uri).is_some_and(|p| canonical(&p).starts_with(cache))
            });
        // A SPARQL document never replaces an indexed RDF copy.
        if !cached_import && kind != DocKind::Sparql {
            // The open text supersedes the indexed copy (re-read on close), and
            // a second full analysis of a large file costs hundreds of MB. The
            // index keeps its contribution until the open document is analyzed.
            self.disk.remove(&uri);
        }
        self.open.insert(
            uri.clone(),
            Doc {
                text,
                version,
                kind,
                analysis: None,
                query: None,
                cached_import,
            },
        );
        self.edited(&uri)
    }

    /// Apply the client's edits to the open document `uri`. The document is
    /// due for diagnostics. An edit to a prefixes.ttl analyzes the open
    /// documents below it again and changes knowledge; the indexed files
    /// follow when it is saved or closed (re-reading the workspace per
    /// keystroke stalls). An edit to a context file analyzes the open
    /// documents that use it again.
    pub fn change_doc(
        &mut self,
        uri: &str,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
        enc: Encoding,
    ) -> Effects {
        if let Some(doc) = self.open.get_mut(uri) {
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
            doc.query = None;
        }
        self.edited(uri)
    }

    /// The text of `uri` (open, or not open when the client skipped the
    /// open) is new: what that means beyond the document itself.
    fn edited(&mut self, uri: &str) -> Effects {
        let mut fx = Effects::one(uri);
        if is_prefix_uri(uri) {
            match uri_to_path(uri) {
                Some(file) => self.invalidate_open_prefixes(&file),
                // No file path (the browser build): every open document.
                None => {
                    self.prefixes_changed(None);
                }
            }
            fx.add(Effects::knowledge());
        }
        if self.context_changed(uri, false) {
            fx.add(Effects::knowledge());
        }
        if !self.open.contains_key(uri) {
            // Not among the open documents, but due all the same.
            fx.dirty = Dirty::One(uri.to_string());
        }
        fx
    }

    /// The open document `uri` was saved: a saved prefixes.ttl lists the
    /// indexed files it serves whose prefixes differ now, a saved context
    /// file parses the indexed files that use it again, and SHACL is due.
    pub fn save_doc(&mut self, uri: &str) -> Effects {
        let mut fx = Effects {
            shacl: true,
            ..Effects::default()
        };
        if is_prefix_uri(uri) {
            fx.add(Effects {
                reparse: self.prefix_file_changed(uri),
                ..Effects::knowledge()
            });
        }
        if self.context_changed(uri, true) {
            fx.add(Effects::knowledge());
        }
        fx
    }

    /// Closing a file indexes it from disk again, when the scan would have
    /// ([`Workspace::indexable`]). Closing a SPARQL document leaves the index
    /// untouched. Unsaved edits to a context file or a prefixes.ttl are gone
    /// with the buffer: what they changed changes back.
    pub fn close_doc(&mut self, uri: &str) -> Effects {
        let gone = self.open.remove(uri);
        if !gone.is_some_and(|d| d.cached_import || d.kind == DocKind::Sparql) {
            // Only what the scan would index goes back to the index: an RDF file
            // below a root (a .txt opened as Turtle, or a file elsewhere, does not).
            if let Some(path) = uri_to_path(uri).filter(|p| is_rdf_file(p)) {
                self.reindex_disk_file(&path);
            }
            self.sync(uri);
        }
        let mut fx = Effects::default();
        if self.context_changed(uri, true) {
            fx.add(Effects::knowledge());
        }
        if is_prefix_uri(uri) {
            fx.add(Effects {
                reparse: self.prefix_file_changed(uri),
                ..Effects::knowledge()
            });
        }
        fx
    }

    /// Register the winning analysis of `uri` with the knowledge index: the
    /// open document's (once analyzed), else the disk copy's.
    fn sync(&mut self, uri: &str) {
        match self.open.get(uri) {
            Some(d) if d.cached_import || d.kind == DocKind::Sparql => {}
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
    /// last call. Several are analyzed in parallel.
    pub fn knowledge(&mut self) -> &Knowledge {
        let stale: Vec<(String, Syntax)> = self
            .open
            .iter()
            .filter_map(|(u, d)| match d.kind {
                DocKind::Rdf(s) if d.analysis.is_none() => Some((u.clone(), s)),
                _ => None,
            })
            .collect();
        if stale.len() < 2 {
            for (uri, _) in stale {
                self.analysis(&uri);
            }
            return &self.knowledge;
        }
        let jobs: Vec<(String, Syntax, Arc<PrefixMap>)> = stale
            .into_iter()
            .map(|(uri, s)| {
                let ext = self.external_prefixes_as(&uri, s);
                (uri, s, ext)
            })
            .collect();
        let (open, roots, uses) = (&self.open, &self.roots, &self.context_uses);
        let contexts = self.contexts();
        let done: Vec<(String, Arc<Analysis>, Option<DocFacts>)> = jobs
            .into_par_iter()
            .filter_map(|(uri, syntax, ext)| {
                let doc = open.get(&uri)?;
                let loader = DocContexts::new(open, roots, &*contexts);
                let a = analyze(uri.clone(), &doc.text, ext, true, syntax, &loader);
                loader.record(uses, &uri, syntax);
                if doc.cached_import {
                    return Some((uri, Arc::new(a), None));
                }
                let (a, facts) = prepared(a);
                Some((uri, Arc::new(a), Some(facts)))
            })
            .collect();
        for (uri, a, facts) in done {
            if let Some(f) = facts {
                self.knowledge
                    .set_prepared(&uri, Source::Open, a.clone(), f);
            }
            if let Some(doc) = self.open.get_mut(&uri) {
                doc.analysis = Some(a);
            }
        }
        &self.knowledge
    }

    /// Read-only sources (e.g. imported ontologies): they add facts to the
    /// knowledge index but are never diagnosed.
    pub fn knowledge_mut(&mut self) -> &mut Knowledge {
        &mut self.knowledge
    }

    /// The RDF analysis of `uri`: the open document's, else the indexed
    /// copy's. `None` for a SPARQL document (see [`Workspace::sparql`]).
    pub fn analysis(&mut self, uri: &str) -> Option<Arc<Analysis>> {
        let syntax = match self.open.get(uri) {
            Some(d) if d.analysis.is_some() => return d.analysis.clone(),
            Some(Doc {
                kind: DocKind::Rdf(syntax),
                ..
            }) => *syntax,
            Some(_) => return None,
            None => return self.disk.get(uri).cloned(),
        };
        let external = self.external_prefixes_as(uri, syntax);
        let contexts = self.contexts();
        let loader = DocContexts::new(&self.open, &self.roots, &*contexts);
        let a = analyze(
            uri.to_string(),
            &self.open.get(uri)?.text,
            external,
            true,
            syntax,
            &loader,
        );
        loader.record(&self.context_uses, uri, syntax);
        let doc = self.open.get_mut(uri)?;
        if doc.cached_import {
            let a = Arc::new(a);
            doc.analysis = Some(a.clone());
            return Some(a);
        }
        let (a, facts) = prepared(a);
        let a = Arc::new(a);
        doc.analysis = Some(a.clone());
        self.knowledge
            .set_prepared(uri, Source::Open, a.clone(), facts);
        Some(a)
    }

    /// Whether `uri` is an open SPARQL document.
    pub fn is_sparql(&self, uri: &str) -> bool {
        self.open
            .get(uri)
            .is_some_and(|d| d.kind == DocKind::Sparql)
    }

    /// The parse of the open SPARQL document `uri` (one parse per version),
    /// or `None` when `uri` is no open SPARQL document.
    pub fn sparql(&mut self, uri: &str) -> Option<Arc<SparqlAnalysis>> {
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
    pub fn sparql_documents(&mut self) -> Vec<Arc<SparqlAnalysis>> {
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
                let text = std::fs::read_to_string(&path).ok()?;
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
    pub fn sparql_files_changed(&mut self, paths: &[PathBuf]) {
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
            let takes = is_sparql_path(path)
                && self.in_area(path, false)
                && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES);
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
    pub fn find_prefix_file(&self, dir: &Path) -> Option<PathBuf> {
        find_prefix_file(&self.roots, dir)
    }

    /// The open prefixes.ttl at `file` is being edited: the open documents
    /// below it re-resolve. Indexed files follow when it is saved (see
    /// [`Workspace::prefixes_changed`]).
    fn invalidate_open_prefixes(&mut self, file: &Path) {
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
                d.analysis = None;
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
    fn prefixes_changed(&mut self, file: Option<&Path>) -> Vec<PathBuf> {
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
            let now = self.external_prefixes_for(&uri);
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
    fn prefix_file_changed(&mut self, uri: &str) -> Vec<PathBuf> {
        match uri_to_path(uri) {
            Some(file) => self.prefixes_changed(Some(&file)),
            None => self.prefixes_changed(None),
        }
    }

    fn is_open_path(&self, path: &Path) -> bool {
        path_to_uri(path).is_some_and(|u| self.open.contains_key(&u))
    }

    // ---- disk index ----

    /// The RDF files under `roots` (see [`scan_roots_matching`]).
    pub fn scan_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
        scan_roots_matching(roots, is_rdf_file).paths
    }

    /// Whether the file at `path` belongs in the disk index, as the scan
    /// reads the roots: an RDF file below a root, in no hidden or skipped
    /// ([`SKIP_DIRS`]) directory, of at most [`MAX_FILE_BYTES`], and
    /// within [`MAX_FILES`] of them. The scan, a file that changed on disk
    /// and a document that was closed all ask this.
    pub fn indexable(&self, path: &Path) -> bool {
        self.fits(path)
            && (self.disk.len() < MAX_FILES
                || path_to_uri(path).is_some_and(|u| self.disk.contains_key(&u)))
    }

    /// [`Workspace::indexable`] but for the limit on the number of files.
    fn fits(&self, path: &Path) -> bool {
        is_rdf_file(path)
            && self.in_area(path, false)
            && std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE_BYTES)
    }

    /// Whether `path` is somewhere the index looks: below a root, in no
    /// hidden or skipped directory. Changes elsewhere (`node_modules`, a
    /// file outside the workspace) are no concern of the server's.
    pub fn watched(&self, path: &Path) -> bool {
        self.in_area(path, false)
    }

    /// [`Workspace::watched`] for a directory: the directory itself counts
    /// too.
    pub fn watched_dir(&self, path: &Path) -> bool {
        self.in_area(path, true)
    }

    fn in_area(&self, path: &Path, include_last: bool) -> bool {
        use std::path::Component;
        self.roots.iter().any(|root| {
            let Ok(rel) = path.strip_prefix(root) else {
                return false;
            };
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
        })
    }

    /// Parse files in parallel and add them to the disk index.
    pub fn index_paths(&mut self, paths: Vec<PathBuf>) {
        #[cfg(test)]
        INDEX_PASSES.with(|c| c.set(c.get() + 1));
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
                let (a, facts) = prepared(a);
                (uri, Arc::new(a), facts)
            })
            .collect();
        for (uri, a, facts) in files {
            self.knowledge
                .set_prepared(&uri, Source::Disk, a.clone(), facts);
            self.disk.insert(uri, a);
        }
    }

    /// The initial index is being built elsewhere: remember what changes on
    /// disk meanwhile, so that [`Workspace::merge_initial_index`] does not
    /// undo it with what the index read before.
    pub fn expect_initial_index(&mut self) {
        self.touched = Some(Touched::default());
    }

    /// Add the initial index, built elsewhere, without the files and
    /// folders that changed on disk since [`Workspace::expect_initial_index`]
    /// (those are as the changes left them), and files indexed meanwhile.
    pub fn merge_initial_index(&mut self, files: Vec<(String, Analysis)>) {
        let touched = self.touched.take().unwrap_or_default();
        let files = files
            .into_iter()
            .filter(|(uri, _)| {
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
    pub fn prefix_jobs(&mut self, paths: Vec<PathBuf>) -> Vec<ParseJob> {
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
                })
            })
            .collect()
    }

    /// What parsing files needs of the workspace, to do it on another thread.
    pub fn reader(&self) -> Reader {
        Reader {
            roots: self.roots.clone(),
            contexts: self.contexts(),
            uses: self.context_uses.clone(),
        }
    }

    /// Parse files in parallel, without adding them to anything.
    pub fn parse_paths(&mut self, paths: Vec<PathBuf>) -> Vec<(String, Analysis)> {
        let jobs = self.prefix_jobs(paths);
        let contexts = self.contexts();
        parse_files(
            &self.open,
            &self.roots,
            &*contexts,
            &self.context_uses,
            jobs,
        )
        .into_iter()
        .map(|r| (r.uri, r.analysis))
        .collect()
    }

    /// Replace indexed files with their analyses parsed again elsewhere
    /// ([`Reader::parse`]), but only those still closed, still indexed as
    /// they were when the parse began, and parsed with the prefixes they
    /// have now. How many were replaced.
    pub fn add_reparsed(&mut self, files: Vec<Reparsed>) -> usize {
        let mut current = Vec::new();
        for Reparsed { uri, analysis, was } in files {
            let unchanged = match (self.disk.get(&uri), &was) {
                (Some(now), Some(was)) => Arc::ptr_eq(now, was),
                _ => false,
            };
            if self.open.contains_key(&uri)
                || !unchanged
                || *self.external_prefixes_for(&uri) != *analysis.external
            {
                continue;
            }
            current.push((uri, analysis));
        }
        let n = current.len();
        self.add_disk(current, true);
        n
    }

    /// Bring the index up to date with the file at `path`, which changed on
    /// disk (it is not open): parse it again, or drop it when it is gone or
    /// no longer [`indexable`](Workspace::indexable). Whether the index
    /// changed.
    pub fn reindex_disk_file(&mut self, path: &Path) -> bool {
        self.reindex_disk_files(&[path.to_path_buf()])
    }

    /// [`Workspace::reindex_disk_file`] for the files of one change: those
    /// that are indexable are parsed together, in one parallel pass, and
    /// the others dropped. What is read is the files as they are now, so a
    /// path listed twice is one file, and the limit of [`MAX_FILES`] counts the
    /// files this adds. Whether the index changed.
    pub fn reindex_disk_files(&mut self, paths: &[PathBuf]) -> bool {
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
            record_contexts(&self.context_uses, uri, Vec::new());
        }
        self.sync(uri);
        had
    }

    /// The directory `dir` was deleted or renamed: drop the files indexed
    /// below it. Whether there were any.
    pub fn remove_under(&mut self, dir: &Path) -> bool {
        self.remove_under_any(&[dir.to_path_buf()])
    }

    /// [`Workspace::remove_under`] for several directories, in one pass over
    /// the index.
    pub fn remove_under_any(&mut self, dirs: &[PathBuf]) -> bool {
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
    /// nothing outside the roots. Catalogs and imported RDF/XML files are
    /// the caller's (imports), and so is the settings file.
    ///
    /// A changed prefixes.ttl is taken first, so the files of the same
    /// change are parsed once, with the new prefixes; the indexed files it
    /// serves and that did not change are left to parse again.
    pub fn files_changed(&mut self, changes: &[(PathBuf, Change)]) -> Effects {
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
            if is_import_source(path) {
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
                shacl: true,
                ..Effects::knowledge()
            });
        }
        fx
    }
}

/// A file to parse, with the prefixes to parse it with, and the indexed copy
/// it replaces, if any.
pub struct ParseJob {
    uri: String,
    path: PathBuf,
    external: Arc<PrefixMap>,
    was: Option<Arc<Analysis>>,
}

/// A file parsed again, and the indexed copy it was meant to replace.
pub struct Reparsed {
    uri: String,
    analysis: Analysis,
    was: Option<Arc<Analysis>>,
}

/// What parsing files needs of a workspace, apart from the workspace: it can
/// be handed to another thread ([`Workspace::reader`]).
pub struct Reader {
    roots: Vec<PathBuf>,
    contexts: Arc<dyn ContextLoader>,
    uses: ContextUses,
}

impl Reader {
    /// Parse files in parallel, without adding them to anything. Open
    /// documents are not seen: JSON-LD contexts come from the files.
    pub fn parse(&self, jobs: Vec<ParseJob>) -> Vec<Reparsed> {
        parse_files(
            &FxHashMap::default(),
            &self.roots,
            &*self.contexts,
            &self.uses,
            jobs,
        )
    }
}

fn parse_files(
    open: &FxHashMap<String, Doc>,
    roots: &[PathBuf],
    contexts: &dyn ContextLoader,
    uses: &ContextUses,
    jobs: Vec<ParseJob>,
) -> Vec<Reparsed> {
    jobs.into_par_iter()
        .filter_map(
            |ParseJob {
                 uri,
                 path,
                 external,
                 was,
             }| {
                if std::fs::metadata(&path).ok()?.len() > MAX_FILE_BYTES {
                    return None;
                }
                let text = std::fs::read_to_string(&path).ok()?;
                let syntax = Syntax::from_extension(&uri).unwrap_or_default();
                let loader = DocContexts::new(open, roots, contexts);
                let a = analyze(uri.clone(), &text, external, false, syntax, &loader);
                loader.record(uses, &uri, syntax);
                Some(Reparsed {
                    uri,
                    analysis: a,
                    was,
                })
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
pub struct Scan {
    /// The files `wanted` accepted, in path order.
    pub paths: Vec<PathBuf>,
    /// More matching files exist than the cap allows; the walk stopped at the
    /// cap, so `paths` is the first part of the path order.
    pub truncated: bool,
    /// Matching files over [`MAX_FILE_BYTES`], left out. Not complete when
    /// the scan was `truncated`.
    pub too_large: Vec<PathBuf>,
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
pub fn scan_roots_matching(roots: &[PathBuf], wanted: fn(&Path) -> bool) -> Scan {
    scan_roots_capped(roots, wanted, MAX_FILES)
}

/// [`scan_roots_matching`] with a cap of `max_files`. One walk, in path order
/// (the entries of each folder sorted by name, a folder visited in place), so
/// what a cap leaves out does not depend on the file system's order.
pub fn scan_roots_capped(roots: &[PathBuf], wanted: fn(&Path) -> bool, max_files: usize) -> Scan {
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

/// The prefixes.ttl that supplies prefixes to files in `dir` (searching up to
/// the innermost workspace root), if any.
pub fn find_prefix_file(roots: &[PathBuf], dir: &Path) -> Option<PathBuf> {
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

#[cfg(test)]
pub(crate) mod tests {
    #[test]
    fn file_uri_name_is_the_decoded_last_segment() {
        assert_eq!(
            file_uri_name("file:///work/shapes/my%20file.ttl").as_deref(),
            Some("my file.ttl")
        );
        assert_eq!(
            file_uri_name("file:///work/a.ttl").as_deref(),
            Some("a.ttl")
        );
        assert_eq!(file_uri_name("https://e.org/a.ttl"), None);
        assert_eq!(file_uri_name("file:///work/"), None);
    }

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

    #[test]
    fn jsonld_documents_are_rdf_and_parse_without_turtle_errors() {
        assert_eq!(
            DocKind::detect(Some("json"), "file:///w/a.jsonld"),
            DocKind::Rdf(Syntax::JsonLd)
        );
        assert!(is_rdf_file(Path::new("/w/a.jsonld")) && !is_rdf_file(Path::new("/w/a.json")));
        let mut ws = Workspace::default();
        let uri = "file:///w/a.jsonld".to_string();
        ws.open_doc(uri.clone(), r#"{"@id": "ex:a", "ex:p": 1}"#.into(), 1);
        let a = ws.analysis(&uri).unwrap();
        assert_eq!(a.parsed.syntax, Syntax::JsonLd);
        assert_eq!(a.parsed.document_base.as_deref(), Some(uri.as_str()));
        assert!(a.parsed.errors.is_empty() && a.external.is_empty());
    }

    #[test]
    fn a_new_context_loader_analyzes_open_jsonld_documents_again() {
        let mut ws = Workspace::default();
        let (ld, ttl) = ("file:///w/a.jsonld", "file:///w/a.ttl");
        ws.open_doc(ld.into(), "{}".into(), 1);
        ws.open_doc(ttl.into(), "".into(), 1);
        let (a, b) = (ws.analysis(ld).unwrap(), ws.analysis(ttl).unwrap());
        ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default()));
        assert!(!Arc::ptr_eq(&a, &ws.analysis(ld).unwrap()));
        assert!(Arc::ptr_eq(&b, &ws.analysis(ttl).unwrap()));
    }

    /// A fresh directory (canonical), removed when dropped.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(name: &str) -> TempDir {
            let d =
                std::env::temp_dir().join(format!("turtle-lsp-ws-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            TempDir(canonical(&d))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn context_errors(ws: &mut Workspace, uri: &str) -> Vec<String> {
        ws.analysis(uri)
            .unwrap()
            .parsed
            .errors
            .iter()
            .filter(|e| e.code == "jsonld-context")
            .map(|e| e.message().to_string())
            .collect()
    }

    const USES_CTX: &str = r#"{"@context": "ctx.jsonld", "@id": "http://ex.org/a", "name": "A"}"#;
    const CTX: &str = r#"{"@context": {"name": "http://ex.org/name"}}"#;

    #[test]
    fn a_file_context_comes_from_the_open_document_then_the_disk_inside_the_roots_only() {
        let root = TempDir::new("ctx-root");
        let outside = TempDir::new("ctx-outside");
        for dir in [&root.0, &outside.0] {
            std::fs::write(dir.join("ctx.jsonld"), CTX).unwrap();
        }
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let inside = path_to_uri(&root.0.join("a.jsonld")).unwrap();
        let away = path_to_uri(&outside.0.join("a.jsonld")).unwrap();
        ws.open_doc(inside.clone(), USES_CTX.into(), 1);
        ws.open_doc(away.clone(), USES_CTX.into(), 1);
        assert_eq!(context_errors(&mut ws, &inside), Vec::<String>::new());
        let refused = context_errors(&mut ws, &away);
        assert!(
            refused.len() == 1 && refused[0].contains("outside the workspace"),
            "{refused:?}"
        );
        assert_eq!(ws.analysis(&inside).unwrap().parsed.triples.len(), 1);

        // An open context file wins over the disk, and edits to it reach the
        // documents that use it.
        let ctx = path_to_uri(&root.0.join("ctx.jsonld")).unwrap();
        ws.open_doc(ctx.clone(), r#"{"@context": {"name": null}}"#.into(), 1);
        assert!(ws.context_changed(&ctx, false));
        assert_eq!(ws.analysis(&inside).unwrap().parsed.triples.len(), 0);
        // Outside the roots, an open document is served too (the browser
        // has nothing else).
        let away_ctx = path_to_uri(&outside.0.join("ctx.jsonld")).unwrap();
        ws.open_doc(away_ctx.clone(), CTX.into(), 1);
        assert!(ws.context_changed(&away_ctx, false));
        assert_eq!(context_errors(&mut ws, &away), Vec::<String>::new());
        assert!(!ws.context_changed("file:///nobody/uses.jsonld", true));
    }

    #[test]
    fn indexed_json_ld_files_follow_their_context_file_and_the_network_setting() {
        let root = TempDir::new("ctx-disk");
        std::fs::write(root.0.join("ctx.jsonld"), CTX).unwrap();
        std::fs::write(root.0.join("a.jsonld"), USES_CTX).unwrap();
        let web = r#"{"@context": "https://example.org/unknown-context.jsonld", "@id": "http://ex.org/b"}"#;
        std::fs::write(root.0.join("b.jsonld"), web).unwrap();
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default()));
        ws.index_paths(Workspace::scan_roots(&ws.roots.clone()));
        let a = path_to_uri(&root.0.join("a.jsonld")).unwrap();
        let b = path_to_uri(&root.0.join("b.jsonld")).unwrap();
        let ctx = path_to_uri(&root.0.join("ctx.jsonld")).unwrap();
        assert_eq!(ws.disk[&a].parsed.triples.len(), 1);
        std::fs::write(root.0.join("ctx.jsonld"), r#"{"@context": {"name": null}}"#).unwrap();
        assert!(ws.context_changed(&ctx, true));
        assert_eq!(ws.disk[&a].parsed.triples.len(), 0);
        // A new loader parses again the indexed files that use web contexts.
        let before = ws.disk[&b].clone();
        let a_before = ws.disk[&a].clone();
        ws.set_context_loader(Arc::new(crate::jsonld::NoNetwork::default().with(
            "https://example.org/unknown-context.jsonld",
            r#"{"@context": {}}"#,
        )));
        assert!(!Arc::ptr_eq(&before, &ws.disk[&b]));
        assert!(
            ws.disk[&b].parsed.errors.is_empty(),
            "{:?}",
            ws.disk[&b].parsed.errors
        );
        assert!(Arc::ptr_eq(&a_before, &ws.disk[&a]));
    }

    #[test]
    fn sparql_documents_are_recognised_and_never_feed_the_index() {
        assert_eq!(
            DocKind::detect(Some("sparql"), "file:///a.txt"),
            DocKind::Sparql
        );
        for id in [None, Some("turtle"), Some("plaintext")] {
            for name in ["a.rq", "a.sparql", "a.ru"] {
                let uri = format!("file:///x/{name}");
                assert_eq!(DocKind::detect(id, &uri), DocKind::Sparql, "{id:?} {name}");
            }
        }
        assert_eq!(
            DocKind::detect(Some("trig"), "file:///a.rq"),
            DocKind::Rdf(Syntax::TriG)
        );
        assert_eq!(
            DocKind::detect(None, "file:///a.nq"),
            DocKind::Rdf(Syntax::NQuads)
        );

        let mut ws = Workspace::default();
        let ttl = "file:///w/a.ttl".to_string();
        let rq = "file:///w/q.rq".to_string();
        ws.open_doc(
            ttl.clone(),
            "@prefix ex: <http://e.org/> .\nex:A a ex:B .\n".into(),
            1,
        );
        ws.open_doc(
            rq.clone(),
            "PREFIX ex: <http://e.org/>\nINSERT DATA { ex:G a ex:B }\n".into(),
            1,
        );
        assert!(ws.is_sparql(&rq) && !ws.is_sparql(&ttl));
        assert!(ws.analysis(&rq).is_none());
        let q = ws.sparql(&rq).unwrap();
        assert!(q.doc.is_update());
        assert!(
            Arc::ptr_eq(&q, &ws.sparql(&rq).unwrap()),
            "one parse per version"
        );
        assert!(ws.sparql(&ttl).is_none());
        assert_eq!(ws.knowledge().docs().count(), 1);
        assert!(!ws.knowledge().is_described("http://e.org/G"));
        assert_eq!(ws.sparql_documents().len(), 1);
        ws.close_doc(&rq);
        assert_eq!(ws.knowledge().docs().count(), 1);
        assert!(ws.sparql_documents().is_empty());
    }

    const TTL: &str = "@prefix ex: <http://e.org/> .\nex:A a ex:B .\n";

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    fn uri(path: &Path) -> String {
        path_to_uri(path).unwrap()
    }

    #[test]
    fn indexable_is_what_the_scan_would_index() {
        let root = TempDir::new("indexable");
        let outside = TempDir::new("indexable-outside");
        let ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let ok = write(&root.0, "sub/a.ttl", TTL);
        assert!(ws.indexable(&ok));
        // Not RDF, outside the roots, in hidden and skipped directories.
        assert!(!ws.indexable(&write(&root.0, "sub/a.txt", TTL)));
        assert!(!ws.indexable(&write(&outside.0, "a.ttl", TTL)));
        for dir in [".git", ".hidden/deep", "node_modules", "a/target", "dist/x"] {
            let f = write(&root.0, &format!("{dir}/b.ttl"), TTL);
            assert!(!ws.indexable(&f), "{dir}");
        }
        // A hidden file name is indexed, as the scan does.
        assert!(ws.indexable(&write(&root.0, ".dotfile.ttl", TTL)));
        // Gone, or not a file.
        assert!(!ws.indexable(&root.0.join("missing.ttl")));
        assert!(!ws.indexable(&root.0.join("sub")));
        // Too large.
        let big = write(&root.0, "big.ttl", "");
        std::fs::File::options()
            .write(true)
            .open(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        assert!(!ws.indexable(&big));
        // The scan and the check agree.
        let scanned = Workspace::scan_roots(&ws.roots);
        assert!(scanned.contains(&ok) && scanned.iter().all(|p| ws.indexable(p)));
        // `..` does not climb out of a root.
        assert!(!ws.indexable(&root.0.join("..").join("x.ttl")));
        // Folders: the folder's own name counts.
        assert!(ws.watched_dir(&root.0.join("sub")));
        assert!(!ws.watched_dir(&root.0.join("node_modules")));
        assert!(!ws.watched(&root.0.join("node_modules").join("x.json")));
    }

    #[test]
    fn a_capped_scan_keeps_the_first_files_in_path_order_and_says_it_stopped() {
        let root = TempDir::new("scan-cap");
        // Created in an order that is neither alphabetical nor reversed.
        for name in ["z/1.ttl", "b/2.ttl", "a.ttl", "b/1.ttl", "m.ttl", "a/9.ttl"] {
            write(&root.0, name, TTL);
        }
        write(&root.0, "node_modules/x.ttl", TTL);
        write(&root.0, "notes.txt", "");
        let names = |scan: &Scan| -> Vec<String> {
            scan.paths
                .iter()
                .map(|p| {
                    p.strip_prefix(&root.0)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect()
        };
        let all = scan_roots_capped(std::slice::from_ref(&root.0), is_rdf_file, 6);
        assert!(!all.truncated, "exactly at the cap is not truncated");
        assert_eq!(
            names(&all),
            ["a/9.ttl", "a.ttl", "b/1.ttl", "b/2.ttl", "m.ttl", "z/1.ttl"]
        );
        let roots = [root.0.clone()];
        let cut = scan_roots_capped(&roots, is_rdf_file, 4);
        assert!(cut.truncated);
        assert_eq!(names(&cut), ["a/9.ttl", "a.ttl", "b/1.ttl", "b/2.ttl"]);
        // The same roots twice, or nested, list a file once.
        let twice = scan_roots_capped(&[root.0.clone(), root.0.join("b")], is_rdf_file, 100);
        assert_eq!(twice.paths.len(), 6);
        // The server's scan is the capped scan at MAX_FILES.
        assert_eq!(scan_roots_matching(&roots, is_rdf_file).paths, all.paths);
    }

    #[test]
    fn a_scan_lists_the_files_it_left_out_for_size() {
        let root = TempDir::new("scan-large");
        let ok = write(&root.0, "ok.ttl", TTL);
        let big = write(&root.0, "sub/big.ttl", "");
        std::fs::File::options()
            .write(true)
            .open(&big)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        // Not wanted, so not named: only a wanted file can be too large.
        let other = write(&root.0, "big.txt", "");
        std::fs::File::options()
            .write(true)
            .open(&other)
            .unwrap()
            .set_len(MAX_FILE_BYTES + 1)
            .unwrap();
        let scan = scan_roots_matching(std::slice::from_ref(&root.0), is_rdf_file);
        assert_eq!(scan.paths, [ok]);
        assert_eq!(scan.too_large, [big]);
        assert!(!scan.truncated);
    }

    #[test]
    fn a_file_changed_on_disk_is_indexed_only_where_the_scan_would() {
        let root = TempDir::new("reindex");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let a = write(&root.0, "a.ttl", TTL);
        let skipped = write(&root.0, "node_modules/b.ttl", TTL);
        assert!(ws.reindex_disk_file(&a));
        assert!(!ws.reindex_disk_file(&skipped));
        assert!(ws.disk.contains_key(&uri(&a)) && !ws.disk.contains_key(&uri(&skipped)));
        assert!(ws.knowledge().get(&uri(&a)).is_some());
        // Gone from disk: gone from the index.
        std::fs::remove_file(&a).unwrap();
        assert!(ws.reindex_disk_file(&a));
        assert!(ws.disk.is_empty() && ws.knowledge().get(&uri(&a)).is_none());
    }

    #[test]
    fn removing_a_folder_drops_the_files_below_it_and_only_those() {
        let root = TempDir::new("remove-under");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let files: Vec<PathBuf> = [
            "gone/a.ttl",
            "gone/deep/b.ttl",
            "gone2/c.ttl",
            "kept/d.ttl",
            "gonex.ttl",
        ]
        .iter()
        .map(|f| write(&root.0, f, TTL))
        .collect();
        ws.index_paths(files.clone());
        assert_eq!(ws.disk.len(), 5);
        assert!(ws.remove_under(&root.0.join("gone")));
        let left: Vec<&str> = [
            "gone/a.ttl",
            "gone/deep/b.ttl",
            "gone2/c.ttl",
            "kept/d.ttl",
            "gonex.ttl",
        ]
        .into_iter()
        .filter(|f| ws.disk.contains_key(&uri(&root.0.join(f))))
        .collect();
        assert_eq!(left, ["gone2/c.ttl", "kept/d.ttl", "gonex.ttl"]);
        for gone in &files[..2] {
            assert!(ws.knowledge().get(&uri(gone)).is_none());
        }
        assert!(ws.knowledge().get(&uri(&files[3])).is_some());
        assert!(
            !ws.remove_under(&root.0.join("gone")),
            "nothing is left to remove"
        );
        assert!(!ws.remove_under_any(&[]));
        // Several at once, in one pass.
        assert!(ws.remove_under_any(&[root.0.join("gone2"), root.0.join("kept")]));
        assert_eq!(ws.disk.len(), 1);
    }

    #[test]
    fn closing_a_file_outside_the_roots_leaves_no_entry() {
        let root = TempDir::new("close-root");
        let outside = TempDir::new("close-outside");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let inside = write(&root.0, "in.ttl", TTL);
        let away = write(&outside.0, "out.ttl", TTL);
        for f in [&inside, &away] {
            ws.open_doc(uri(f), TTL.into(), 1);
            assert!(ws.analysis(&uri(f)).is_some());
        }
        ws.close_doc(&uri(&inside));
        ws.close_doc(&uri(&away));
        assert!(ws.disk.contains_key(&uri(&inside)));
        assert!(!ws.disk.contains_key(&uri(&away)));
        assert!(ws.knowledge().get(&uri(&inside)).is_some());
        assert!(ws.knowledge().get(&uri(&away)).is_none());
        // Nor does a file in a skipped directory come back.
        let skipped = write(&root.0, "node_modules/x.ttl", TTL);
        ws.open_doc(uri(&skipped), TTL.into(), 1);
        ws.analysis(&uri(&skipped));
        ws.close_doc(&uri(&skipped));
        assert!(!ws.disk.contains_key(&uri(&skipped)));
        assert!(ws.knowledge().get(&uri(&skipped)).is_none());
    }

    #[test]
    fn the_read_accessors_agree_with_the_documents_and_the_index() {
        let root = TempDir::new("accessors");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        assert_eq!(ws.roots(), std::slice::from_ref(&root.0));
        ws.set_rescan_sparql(true);
        assert!(ws.rescan_sparql);
        let prefixes = PrefixMap::from_iter([("ex".to_string(), "http://e.org/".to_string())]);
        ws.set_settings_prefixes(prefixes.clone());
        assert_eq!(ws.settings_prefixes(), &prefixes);

        let paths = [
            write(&root.0, "a.ttl", TTL),
            write(&root.0, "b.trig", TTL),
            write(&root.0, "q.rq", "SELECT * WHERE { ?s ?p ?o }\n"),
        ];
        let [a, b, q] = paths.each_ref().map(|p| uri(p));
        let agree = |ws: &Workspace| {
            fn sorted(mut v: Vec<&str>) -> Vec<&str> {
                v.sort_unstable();
                v
            }
            assert_eq!(
                sorted(ws.open_uris().collect()),
                sorted(ws.open.keys().map(String::as_str).collect())
            );
            assert_eq!(
                sorted(ws.indexed_uris().collect()),
                sorted(ws.disk.keys().map(String::as_str).collect())
            );
            assert_eq!(ws.indexed_count(), ws.disk.len());
            for u in [&a, &b, &q] {
                assert_eq!(ws.is_open(u), ws.open.contains_key(u));
                assert_eq!(ws.version(u), ws.open.get(u).map(|d| d.version));
                assert_eq!(ws.doc_kind(u), ws.open.get(u).map(|d| d.kind));
                assert_eq!(ws.is_indexed(u), ws.disk.contains_key(u));
                assert_eq!(
                    ws.indexed_analysis(u).map(Arc::as_ptr),
                    ws.disk.get(u).map(Arc::as_ptr)
                );
            }
        };

        let parsed = ws.parse_paths(paths[..2].to_vec());
        ws.add_disk(parsed, true);
        agree(&ws);
        assert!(ws.is_indexed(&a) && ws.is_indexed(&b) && !ws.is_indexed(&q));
        assert_eq!(ws.open_uris().count(), 0);
        assert_eq!(ws.version(&a), None);

        // An open RDF document that is also on disk is open, not indexed.
        ws.open_doc(a.clone(), TTL.into(), 3);
        ws.open_doc(q.clone(), "SELECT * WHERE { ?s ?p ?o }\n".into(), 1);
        agree(&ws);
        assert!(ws.is_open(&a) && !ws.is_indexed(&a));
        assert_eq!(ws.version(&a), Some(3));
        assert_eq!(ws.doc_kind(&a), Some(DocKind::Rdf(Syntax::Turtle)));
        assert_eq!(ws.doc_kind(&q), Some(DocKind::Sparql));
        assert_eq!(ws.doc_kind(&b), None);
        assert_eq!(ws.indexed_uris().collect::<Vec<_>>(), [b.as_str()]);

        // The index does not take back a file while it is open.
        let parsed = ws.parse_paths(vec![paths[0].clone()]);
        ws.add_disk(parsed, true);
        agree(&ws);
        assert!(!ws.is_indexed(&a));

        // Closing it indexes it from disk again.
        ws.close_doc(&a);
        ws.close_doc(&q);
        agree(&ws);
        assert!(!ws.is_open(&a) && ws.is_indexed(&a));
        assert_eq!((ws.version(&a), ws.doc_kind(&a)), (None, None));
        assert_eq!(ws.indexed_count(), 2);
    }

    #[test]
    fn what_changed_before_the_initial_index_landed_is_not_undone_by_it() {
        let root = TempDir::new("pre-merge");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.expect_initial_index();
        let deleted = write(&root.0, "deleted.ttl", TTL);
        let changed = write(&root.0, "changed.ttl", TTL);
        let in_folder = write(&root.0, "folder/f.ttl", TTL);
        let kept = write(&root.0, "kept.ttl", TTL);
        // The index reads the disk as it is now ...
        let mut reader = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let files = reader.parse_paths(Workspace::scan_roots(std::slice::from_ref(&root.0)));
        assert_eq!(files.len(), 4);
        // ... and the disk changes before it lands.
        std::fs::remove_file(&deleted).unwrap();
        ws.reindex_disk_file(&deleted);
        std::fs::write(&changed, "@prefix ex: <http://e.org/> .\nex:New a ex:B .\n").unwrap();
        ws.reindex_disk_file(&changed);
        std::fs::remove_dir_all(root.0.join("folder")).unwrap();
        ws.remove_under(&root.0.join("folder"));
        ws.merge_initial_index(files);
        assert!(
            !ws.disk.contains_key(&uri(&deleted)),
            "a deleted file came back"
        );
        assert!(
            !ws.disk.contains_key(&uri(&in_folder)),
            "a deleted folder's file came back"
        );
        assert!(ws.disk.contains_key(&uri(&kept)));
        assert!(ws.knowledge().is_described("http://e.org/New"));
        assert!(
            !ws.knowledge().is_described("http://e.org/A") || ws.disk.contains_key(&uri(&kept))
        );
        assert!(ws.knowledge().get(&uri(&deleted)).is_none());
        // After the merge nothing is remembered any more.
        assert!(ws.touched.is_none());
    }

    #[test]
    fn a_panic_in_analysis_is_that_document_s_error_not_the_end_of_the_server() {
        let quiet = QuietPanics::new();
        let mut ws = Workspace::default();
        let (bad, good, facts) = (
            "file:///w/bad.ttl",
            "file:///w/good.ttl",
            "file:///w/facts.ttl",
        );
        ws.open_doc(bad.into(), format!("# {PANIC_IN_ANALYSIS}\n{TTL}"), 1);
        ws.open_doc(good.into(), TTL.into(), 1);
        ws.open_doc(facts.into(), format!("# {PANIC_IN_FACTS}\n{TTL}"), 1);
        // The analyses of several documents at once, as the batch does.
        assert_eq!(ws.knowledge().docs().count(), 3);
        for u in [bad, facts] {
            let a = ws.analysis(u).unwrap();
            let e: Vec<_> = a
                .parsed
                .errors
                .iter()
                .filter(|e| e.code == "internal-error")
                .collect();
            assert_eq!(e.len(), 1, "{u}");
            assert!(
                e[0].message.contains("internal error") && e[0].message.contains("report"),
                "{}",
                e[0].message
            );
        }
        assert!(ws.analysis(good).unwrap().parsed.errors.is_empty());
        // One at a time, as a request does.
        ws.change_doc(
            good,
            2,
            vec![TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: format!("# {PANIC_IN_ANALYSIS}\n"),
            }],
            Encoding::Utf16,
        );
        assert!(
            ws.analysis(good)
                .unwrap()
                .parsed
                .errors
                .iter()
                .any(|e| e.code == "internal-error")
        );
        // Indexed files too.
        let root = TempDir::new("panic-disk");
        let f = write(&root.0, "bad.ttl", &format!("# {PANIC_IN_ANALYSIS}\n{TTL}"));
        let g = write(&root.0, "ok.ttl", TTL);
        let mut disk = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        disk.index_paths(vec![f.clone(), g.clone()]);
        drop(quiet);
        assert_eq!(disk.disk.len(), 2);
        assert!(
            disk.disk[&uri(&f)]
                .parsed
                .errors
                .iter()
                .any(|e| e.code == "internal-error")
        );
        assert!(disk.disk[&uri(&g)].parsed.errors.is_empty());
    }

    #[test]
    fn prefixes_are_cached_by_file_and_a_change_reaches_only_the_files_it_serves() {
        let root = TempDir::new("prefix-scope");
        let ns = |n: &str| format!("@prefix {n}: <http://{n}.org/> .\n");
        let pf = write(&root.0, "a/prefixes.ttl", &ns("one"));
        let nested = write(&root.0, "a/sub/prefixes.ttl", &ns("two"));
        let files: Vec<PathBuf> = [
            write(&root.0, "a/x.ttl", "ex:s ex:p ex:o .\n"),
            write(&root.0, "a/deeper/y.ttl", "ex:s ex:p ex:o .\n"),
            write(&root.0, "a/sub/z.ttl", "ex:s ex:p ex:o .\n"),
            write(&root.0, "b/w.ttl", "ex:s ex:p ex:o .\n"),
        ]
        .into();
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(files.clone());
        let ext = |ws: &mut Workspace, f: &Path| ws.external_prefixes_for(&uri(f));
        // One map per prefixes.ttl, shared by every directory it serves.
        assert!(Arc::ptr_eq(
            &ext(&mut ws, &files[0]),
            &ext(&mut ws, &files[1])
        ));
        assert!(!Arc::ptr_eq(
            &ext(&mut ws, &files[0]),
            &ext(&mut ws, &files[2])
        ));
        assert!(ext(&mut ws, &files[0]).contains_key("one"));
        assert!(
            ext(&mut ws, &files[2]).contains_key("two")
                && !ext(&mut ws, &files[2]).contains_key("one")
        );
        assert!(ext(&mut ws, &files[3]).is_empty());

        // Saving a prefixes.ttl without a change to what it declares re-parses nothing.
        assert!(ws.prefixes_changed(Some(&pf)).is_empty());
        // Changing it re-parses the files it serves, not the nested folder's, not b/'s.
        std::fs::write(&pf, format!("{}{}", ns("one"), ns("three"))).unwrap();
        let mut stale = ws.prefixes_changed(Some(&pf));
        stale.sort();
        let mut served = vec![files[0].clone(), files[1].clone()];
        served.sort();
        assert_eq!(stale, served);
        assert!(ext(&mut ws, &files[0]).contains_key("three"));
        // The reparse lands only for files still as they were and with today's prefixes.
        let jobs = ws.prefix_jobs(stale);
        let parsed = ws.reader().parse(jobs);
        assert_eq!(ws.add_reparsed(parsed), 2);
        assert!(ws.disk[&uri(&files[0])].external.contains_key("three"));

        // A prefixes.ttl created later takes over the files below it.
        let made = write(&root.0, "b/prefixes.ttl", &ns("four"));
        assert_eq!(ws.prefixes_changed(Some(&made)), [files[3].clone()]);
        // A deleted one gives its files back to the nearest above (here, none).
        std::fs::remove_file(&nested).unwrap();
        assert_eq!(ws.prefixes_changed(Some(&nested)), [files[2].clone()]);
        assert!(!ext(&mut ws, &files[2]).contains_key("two"));
        // A settings change reaches every file whose prefixes it changes.
        ws.settings_prefixes
            .insert("set".into(), "http://set.org/".into());
        assert_eq!(ws.prefixes_changed(None).len(), 4);
        // A reparse made before another change is dropped.
        let jobs = ws.prefix_jobs(vec![files[0].clone()]);
        let parsed = ws.reader().parse(jobs);
        ws.settings_prefixes
            .insert("later".into(), "http://later.org/".into());
        ws.prefixes_changed(None);
        assert_eq!(ws.add_reparsed(parsed), 0);
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

    const QUERY: &str = "PREFIX ex: <http://e.org/>\nSELECT * WHERE { ?s a ex:B }\n";

    /// Every call used to walk the roots and read every query file again (9.6
    /// ms for a tree of 20,000 entries with no hit). The files are found once
    /// and kept; the watcher's events keep them current.
    #[test]
    fn sparql_files_are_found_once_and_kept_current() {
        let root = TempDir::new("sparql-kept");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let a = write(&root.0, "q/a.rq", QUERY);
        let b = write(&root.0, "b.sparql", QUERY);
        write(&root.0, "node_modules/x.rq", QUERY);
        let uris = |ws: &mut Workspace| {
            let mut v: Vec<String> = ws
                .sparql_documents()
                .iter()
                .map(|q| q.uri.clone())
                .collect();
            v.sort();
            v
        };
        let mut both = vec![uri(&a), uri(&b)];
        both.sort();
        assert_eq!(uris(&mut ws), both);

        // No directory is read again: a file nobody reported is not found...
        let c = write(&root.0, "c.ru", "CLEAR ALL\n");
        assert_eq!(uris(&mut ws), both);
        // ...and one the watcher reported is, with the ones that were deleted
        // and the one that changed, read together.
        let before = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&b))
            .unwrap();
        let untouched = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&a))
            .unwrap();
        std::fs::remove_file(&a).unwrap();
        write(
            &root.0,
            "b.sparql",
            "SELECT * WHERE { ?s ?p ?o . ?o ?p ?s }\n",
        );
        write(&root.0, "node_modules/y.rq", QUERY);
        ws.sparql_files_changed(&[
            a.clone(),
            b.clone(),
            c.clone(),
            c.clone(),
            root.0.join("node_modules/y.rq"),
            root.0.join("gone.rq"),
        ]);
        let mut now = vec![uri(&b), uri(&c)];
        now.sort();
        assert_eq!(uris(&mut ws), now);
        let after = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&b))
            .unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        drop(untouched);

        // A file written without the watcher saying so is read again when asked.
        write(
            &root.0,
            "b.sparql",
            "SELECT * WHERE { ?s ?p ?o }  # longer than before\n",
        );
        let again = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&b))
            .unwrap();
        assert!(!Arc::ptr_eq(&after, &again));
        let kept = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&b))
            .unwrap();
        assert!(
            Arc::ptr_eq(&again, &kept),
            "an unchanged file is not parsed again"
        );

        // A deleted folder takes its queries with it.
        write(&root.0, "d/e/f.rq", QUERY);
        ws.sparql_files_changed(&[root.0.join("d/e/f.rq")]);
        assert_eq!(ws.sparql_documents().len(), 3);
        std::fs::remove_dir_all(root.0.join("d")).unwrap();
        ws.remove_under(&root.0.join("d"));
        assert_eq!(ws.sparql_documents().len(), 2);
        // A changed prefixes.ttl parses them again with the new prefixes.
        let was = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&c))
            .unwrap();
        let prefixes = write(&root.0, "prefixes.ttl", "@prefix p: <http://p.org/> .\n");
        ws.prefixes_changed(Some(&prefixes));
        let now = ws
            .sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri(&c))
            .unwrap();
        assert!(!Arc::ptr_eq(&was, &now));
        assert!(now.external.contains_key("p"));
    }

    /// An open query is the open text, not the file; a closed one is the file
    /// again. The index of files is not asked before the first call.
    #[test]
    fn open_queries_win_over_their_files_in_the_sparql_documents() {
        let root = TempDir::new("sparql-open");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let a = write(&root.0, "a.rq", QUERY);
        // Before the first call, an event reads nothing.
        ws.sparql_files_changed(std::slice::from_ref(&a));
        assert!(ws.sparql_files.is_none());
        let open = "SELECT * WHERE { ?x ?y ?z }\n";
        ws.open_doc(uri(&a), open.into(), 1);
        let docs = ws.sparql_documents();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].doc.text().contains("?x ?y ?z"));
        ws.close_doc(&uri(&a));
        let docs = ws.sparql_documents();
        assert_eq!(docs.len(), 1);
        assert!(docs[0].doc.text().contains("ex:B"));
    }

    /// One notification's changed files were parsed one at a time, each with a
    /// pass of its own (2,000 files: 108 ms against 73 ms for the parallel
    /// initial index). They are parsed in one pass.
    #[test]
    fn changed_files_are_indexed_in_one_pass() {
        let root = TempDir::new("batch");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let mut paths: Vec<PathBuf> = (0..50)
            .map(|i| write(&root.0, &format!("d{}/f{i}.ttl", i % 5), TTL))
            .collect();
        let before = INDEX_PASSES.with(|c| c.get());
        // Twice listed, not RDF, outside, in a skipped folder, and missing.
        let junk = [
            write(&root.0, "a.txt", TTL),
            write(&root.0, "node_modules/n.ttl", TTL),
            root.0.join("missing.ttl"),
            paths[0].clone(),
        ];
        let mut all = paths.clone();
        all.extend(junk);
        assert!(ws.reindex_disk_files(&all));
        assert_eq!(INDEX_PASSES.with(|c| c.get()) - before, 1);
        assert_eq!(ws.disk.len(), 50);
        // Deletions are dropped in the same call as the changes.
        std::fs::remove_file(&paths[1]).unwrap();
        let fresh = write(&root.0, "fresh.ttl", TTL);
        let before = INDEX_PASSES.with(|c| c.get());
        assert!(ws.reindex_disk_files(&[paths[1].clone(), fresh.clone()]));
        assert_eq!(INDEX_PASSES.with(|c| c.get()) - before, 1);
        assert!(ws.disk.contains_key(&uri(&fresh)) && !ws.disk.contains_key(&uri(&paths[1])));
        paths.push(fresh);
        assert_eq!(ws.disk.len(), 50);
        // Nothing to do, nothing changed.
        assert!(!ws.reindex_disk_files(&[root.0.join("nope.ttl")]));
        assert!(!ws.reindex_disk_files(&[]));
    }

    /// Silences panic messages while it lives, for tests whose panics are
    /// the point. The panic hook is global to the test binary, where tests
    /// run in parallel: a hook taken and set back by two tests at once can
    /// end up silent for good. So the hook is replaced once, by one that
    /// prints as the previous one did unless a `QuietPanics` is alive, and
    /// the guards hold a lock, so one is alive at a time. Dropping the guard
    /// (also when its test panics) makes the hook print again.
    pub(crate) struct QuietPanics {
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    static QUIET_LOCK: Mutex<()> = Mutex::new(());
    static QUIET: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    static QUIET_HOOK: std::sync::Once = std::sync::Once::new();

    impl QuietPanics {
        pub(crate) fn new() -> QuietPanics {
            // A test that failed while holding the lock poisoned it; the
            // guard it held was dropped all the same.
            let lock = QUIET_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            QUIET_HOOK.call_once(|| {
                let previous = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    if !QUIET.load(std::sync::atomic::Ordering::SeqCst) {
                        previous(info);
                    }
                }));
            });
            QUIET.store(true, std::sync::atomic::Ordering::SeqCst);
            QuietPanics { _lock: lock }
        }
    }

    impl Drop for QuietPanics {
        fn drop(&mut self) {
            QUIET.store(false, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[test]
    fn quiet_panics_silence_only_while_alive() {
        // Under the lock no other test's guard is alive.
        let quiet_now = || {
            let _lock = QUIET_LOCK
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            QUIET.load(std::sync::atomic::Ordering::SeqCst)
        };
        let quiet = QuietPanics::new();
        assert!(QUIET.load(std::sync::atomic::Ordering::SeqCst));
        assert!(std::panic::catch_unwind(|| panic!("silenced")).is_err());
        drop(quiet);
        assert!(!quiet_now());
        // A guard dropped while its test panics makes the hook print again.
        let _ = std::panic::catch_unwind(|| {
            let _quiet = QuietPanics::new();
            panic!("silenced too");
        });
        assert!(!quiet_now());
    }

    // ---- incremental sync (textDocument/didChange with ranges) ----

    fn pos(line: u32, character: u32) -> lsp_types::Position {
        lsp_types::Position { line, character }
    }

    /// A ranged change from `(l0, c0)` to `(l1, c1)`.
    fn ranged(
        (l0, c0): (u32, u32),
        (l1, c1): (u32, u32),
        text: &str,
    ) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: Some(lsp_types::Range::new(pos(l0, c0), pos(l1, c1))),
            range_length: None,
            text: text.into(),
        }
    }

    fn full(text: &str) -> TextDocumentContentChangeEvent {
        TextDocumentContentChangeEvent {
            range: None,
            range_length: None,
            text: text.into(),
        }
    }

    #[test]
    fn change_doc_applies_ranged_changes_in_order() {
        let ab = "ab\ncd\n";
        // (what, encoding, text before, the changes in one batch, text after)
        let cases: Vec<(
            &str,
            Encoding,
            &str,
            Vec<TextDocumentContentChangeEvent>,
            &str,
        )> = vec![
            (
                "insert",
                Encoding::Utf16,
                ab,
                vec![ranged((0, 1), (0, 1), "X")],
                "aXb\ncd\n",
            ),
            (
                "delete",
                Encoding::Utf16,
                ab,
                vec![ranged((0, 0), (0, 2), "")],
                "\ncd\n",
            ),
            (
                "delete a line break",
                Encoding::Utf16,
                ab,
                vec![ranged((0, 2), (1, 0), "")],
                "abcd\n",
            ),
            (
                "replace",
                Encoding::Utf16,
                ab,
                vec![ranged((1, 0), (1, 2), "ZZZ")],
                "ab\nZZZ\n",
            ),
            (
                "a batch in order: the second change's positions are in the first's result",
                Encoding::Utf16,
                ab,
                vec![ranged((0, 0), (0, 0), "1\n"), ranged((1, 0), (1, 1), "2")],
                "1\n2b\ncd\n",
            ),
            (
                "a full-text change in a batch, then a ranged one on its text",
                Encoding::Utf16,
                ab,
                vec![full("xyz\n"), ranged((0, 1), (0, 2), "Y")],
                "xYz\n",
            ),
            // U+1F600 is two UTF-16 code units and four UTF-8 bytes.
            (
                "UTF-16 after an astral character",
                Encoding::Utf16,
                "\u{1F600}x\n",
                vec![ranged((0, 2), (0, 3), "y")],
                "\u{1F600}y\n",
            ),
            (
                "UTF-8 after an astral character",
                Encoding::Utf8,
                "\u{1F600}x\n",
                vec![ranged((0, 4), (0, 5), "y")],
                "\u{1F600}y\n",
            ),
            (
                "a UTF-16 column inside the astral character goes to its start",
                Encoding::Utf16,
                "\u{1F600}x\n",
                vec![ranged((0, 1), (0, 1), "Z")],
                "Z\u{1F600}x\n",
            ),
            (
                "a column past the end of its line is the line's end",
                Encoding::Utf16,
                ab,
                vec![ranged((0, 99), (0, 99), "X")],
                "abX\ncd\n",
            ),
            (
                "a line past the end of the file is the file's end",
                Encoding::Utf16,
                ab,
                vec![ranged((7, 0), (9, 9), "!")],
                "ab\ncd\n!",
            ),
            (
                "a range ending past the end of the file",
                Encoding::Utf16,
                ab,
                vec![ranged((1, 1), (9, 0), "")],
                "ab\nc",
            ),
            (
                "a reversed range is the same range",
                Encoding::Utf16,
                ab,
                vec![ranged((1, 1), (0, 1), "-")],
                "a-d\n",
            ),
            (
                "an empty document",
                Encoding::Utf16,
                "",
                vec![ranged((0, 0), (0, 0), "a")],
                "a",
            ),
        ];
        let mut failed = Vec::new();
        for (what, enc, before, changes, after) in cases {
            let mut ws = Workspace::default();
            let uri = "file:///w/a.ttl";
            ws.open_doc(uri.into(), before.into(), 1);
            ws.change_doc(uri, 2, changes, enc);
            if ws.text(uri) != Some(after) {
                failed.push(format!("{what}: {:?}, expected {after:?}", ws.text(uri)));
            }
        }
        assert!(failed.is_empty(), "{}", failed.join("\n"));
    }

    #[test]
    fn change_doc_reanalyzes_and_ignores_documents_that_are_not_open() {
        let mut ws = Workspace::default();
        let uri = "file:///w/a.ttl";
        let text = "@prefix ex: <http://e.org/> .\nex:a ex:p ex:b .\n";
        ws.open_doc(uri.into(), text.into(), 1);
        assert_eq!(ws.analysis(uri).unwrap().parsed.triples.len(), 1);
        // Turn `ex:b .` into `ex:b , ex:c .`: two triples.
        ws.change_doc(
            uri,
            2,
            vec![ranged((1, 14), (1, 14), " , ex:c")],
            Encoding::Utf16,
        );
        assert_eq!(
            ws.text(uri),
            Some("@prefix ex: <http://e.org/> .\nex:a ex:p ex:b , ex:c .\n")
        );
        assert_eq!(ws.analysis(uri).unwrap().parsed.triples.len(), 2);
        assert_eq!(ws.open[uri].version, 2);
        ws.change_doc("file:///w/closed.ttl", 3, vec![full("x")], Encoding::Utf16);
        assert!(ws.text("file:///w/closed.ttl").is_none());
    }

    // ---- effects ----

    /// `Effects` with knowledge changed and SHACL due.
    fn knowledge_and_shacl() -> Effects {
        Effects {
            shacl: true,
            ..Effects::knowledge()
        }
    }

    fn sorted(mut paths: Vec<PathBuf>) -> Vec<PathBuf> {
        paths.sort();
        paths
    }

    #[test]
    fn an_edit_to_a_data_file_is_that_document_alone() {
        let root = TempDir::new("fx-data");
        let a = write(&root.0, "a.ttl", TTL);
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(vec![a.clone()]);
        let one = Effects {
            dirty: Dirty::One(uri(&a)),
            ..Effects::default()
        };
        assert_eq!(ws.open_doc(uri(&a), TTL.into(), 1), one);
        let edit = vec![full("@prefix ex: <http://e.org/> .\nex:C a ex:D .\n")];
        assert_eq!(ws.change_doc(&uri(&a), 2, edit, Encoding::Utf16), one);
        assert_eq!(
            ws.save_doc(&uri(&a)),
            Effects {
                shacl: true,
                ..Effects::default()
            }
        );
    }

    #[test]
    fn an_edit_to_an_open_prefix_file_reaches_open_documents_now_and_indexed_files_on_save() {
        let root = TempDir::new("fx-prefixes");
        let pf = write(
            &root.0,
            "prefixes.ttl",
            "@prefix one: <http://one.org/> .\n",
        );
        let served = vec![
            write(&root.0, "x.ttl", "one:s one:p one:o .\n"),
            write(&root.0, "deeper/y.ttl", "one:s one:p one:o .\n"),
        ];
        // A folder with a prefixes.ttl of its own is not served.
        let own = write(
            &root.0,
            "own/prefixes.ttl",
            "@prefix two: <http://two.org/> .\n",
        );
        let not_served = write(&root.0, "own/z.ttl", "two:s two:p two:o .\n");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(vec![
            pf.clone(),
            served[0].clone(),
            served[1].clone(),
            own,
            not_served,
        ]);
        let open = uri(&root.0.join("open.ttl"));
        ws.open_doc(open.clone(), "one:s one:p one:o .\n".into(), 1);
        assert!(ws.analysis(&open).unwrap().external.contains_key("one"));

        let text = "@prefix one: <http://one.org/> .\n";
        assert_eq!(ws.open_doc(uri(&pf), text.into(), 1), Effects::knowledge());
        let edit = vec![full("@prefix uno: <http://uno.org/> .\n")];
        let fx = ws.change_doc(&uri(&pf), 2, edit, Encoding::Utf16);
        // Every open document is due, and none is left to parse again yet.
        assert_eq!(fx, Effects::knowledge());
        assert!(ws.analysis(&open).unwrap().external.contains_key("uno"));
        let indexed = |ws: &Workspace| ws.indexed_analysis(&uri(&served[0])).unwrap().clone();
        assert!(indexed(&ws).external.contains_key("one"));

        let mut fx = ws.save_doc(&uri(&pf));
        fx.reparse.sort();
        assert_eq!(
            fx,
            Effects {
                reparse: sorted(served),
                ..knowledge_and_shacl()
            }
        );
    }

    #[test]
    fn closing_a_context_file_with_unsaved_edits_reaches_its_users() {
        let root = TempDir::new("fx-context");
        let ctx = write(&root.0, "ctx.jsonld", CTX);
        let closed = write(&root.0, "b.jsonld", USES_CTX);
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(vec![closed.clone()]);
        let user = uri(&root.0.join("a.jsonld"));
        ws.open_doc(user.clone(), USES_CTX.into(), 1);
        let triples = |ws: &mut Workspace| ws.analysis(&user).unwrap().parsed.triples.len();
        assert_eq!(triples(&mut ws), 1);
        let indexed = |ws: &Workspace| ws.indexed_analysis(&uri(&closed)).unwrap().clone();
        let before = indexed(&ws);

        let unsaved = r#"{"@context": {"name": null}}"#;
        assert_eq!(
            ws.open_doc(uri(&ctx), unsaved.into(), 1),
            Effects::knowledge()
        );
        assert_eq!(triples(&mut ws), 0);
        // An edit does not parse the indexed users again.
        assert!(Arc::ptr_eq(&before, &indexed(&ws)));

        assert_eq!(ws.close_doc(&uri(&ctx)), Effects::knowledge());
        assert_eq!(triples(&mut ws), 1);
        assert!(!Arc::ptr_eq(&before, &indexed(&ws)));
    }

    #[test]
    fn a_settings_prefix_change_lists_every_indexed_turtle_file() {
        let root = TempDir::new("fx-settings");
        let turtle = vec![
            write(&root.0, "a.ttl", TTL),
            write(&root.0, "sub/b.ttl", TTL),
            write(&root.0, "c.trig", TTL),
        ];
        let others = [
            write(
                &root.0,
                "d.nt",
                "<http://e.org/s> <http://e.org/p> <http://e.org/o> .\n",
            ),
            write(&root.0, "e.jsonld", r#"{"@id": "http://e.org/s"}"#),
        ];
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(turtle.iter().chain(&others).cloned().collect());
        let mut prefixes = PrefixMap::default();
        prefixes.insert("set".into(), "http://set.org/".into());
        let mut fx = ws.set_settings_prefixes(prefixes);
        fx.reparse.sort();
        assert_eq!(
            fx,
            Effects {
                reparse: sorted(turtle),
                ..knowledge_and_shacl()
            }
        );
    }

    #[test]
    fn a_prefix_file_and_a_data_file_changed_together_parse_the_data_once() {
        let root = TempDir::new("fx-together");
        let pf = write(
            &root.0,
            "prefixes.ttl",
            "@prefix ex: <http://old.example/> .\n",
        );
        let data = write(&root.0, "data.ttl", "ex:a ex:p ex:o .\n");
        let other = write(&root.0, "other.ttl", "ex:a ex:p ex:o .\n");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let created = |p: &PathBuf| (p.clone(), Change::Created);
        ws.files_changed(&[created(&data), created(&pf), created(&other)]);
        assert_eq!(ws.indexed_count(), 3);

        std::fs::write(&pf, "@prefix ex: <http://newer.example/> .\n").unwrap();
        std::fs::write(&data, "ex:b ex:p ex:o .\n").unwrap();
        let before = INDEX_PASSES.with(|c| c.get());
        let fx = ws.files_changed(&[
            (data.clone(), Change::Changed),
            (pf.clone(), Change::Changed),
        ]);
        assert_eq!(INDEX_PASSES.with(|c| c.get()) - before, 1);
        let a = ws.indexed_analysis(&uri(&data)).unwrap();
        assert!(
            a.external
                .get("ex")
                .is_some_and(|i| i == "http://newer.example/")
        );
        // Only the file that did not change is left to parse again.
        assert_eq!(
            fx,
            Effects {
                reparse: vec![other],
                on_disk: vec![data, pf],
                ..knowledge_and_shacl()
            }
        );
    }

    #[test]
    fn a_deleted_folder_forgets_the_files_below_it() {
        let root = TempDir::new("fx-folder");
        let below = [
            write(&root.0, "gone/a.ttl", TTL),
            write(&root.0, "gone/deep/b.ttl", TTL),
        ];
        let kept = write(&root.0, "kept.ttl", TTL);
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.index_paths(below.iter().chain([&kept]).cloned().collect());
        std::fs::remove_dir_all(root.0.join("gone")).unwrap();
        let fx = ws.files_changed(&[(root.0.join("gone"), Change::Deleted)]);
        assert_eq!(fx, knowledge_and_shacl());
        assert!(below.iter().all(|p| !ws.is_indexed(&uri(p))));
        assert!(ws.is_indexed(&uri(&kept)));
        // Nothing indexed below it: nothing changed.
        let fx = ws.files_changed(&[(root.0.join("gone"), Change::Deleted)]);
        assert_eq!(fx, Effects::default());
    }

    #[test]
    fn a_query_changed_on_disk_makes_closed_files_due_but_changes_no_knowledge() {
        let root = TempDir::new("fx-query");
        let q = write(&root.0, "q.rq", "SELECT * WHERE { ?s ?p ?o }\n");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        let fx = ws.files_changed(&[(q, Change::Changed)]);
        assert_eq!(
            fx,
            Effects {
                closed_files: true,
                ..Effects::default()
            }
        );
    }

    #[test]
    fn effects_add_up() {
        let (a, b) = ("file:///w/a.ttl", "file:///w/b.ttl");
        let mut fx = Effects::one(a);
        fx.add(Effects::one(a));
        assert_eq!(fx.dirty, Dirty::One(a.into()));
        fx.add(Effects {
            reparse: vec![PathBuf::from("/w/c.ttl")],
            ..Effects::default()
        });
        assert_eq!(fx.dirty, Dirty::One(a.into()));
        fx.add(Effects::one(b));
        assert_eq!(fx.dirty, Dirty::AllOpen);
        fx.add(Effects {
            shacl: true,
            ..Effects::default()
        });
        assert_eq!(
            fx,
            Effects {
                dirty: Dirty::AllOpen,
                shacl: true,
                reparse: vec![PathBuf::from("/w/c.ttl")],
                ..Effects::default()
            }
        );
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;

    const QUERY: &str = "SELECT * WHERE { ?s ?p ?o }\n";

    /// A fresh folder below the temp dir (canonical), removed when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir().join(format!(
                "turtle-lsp-review-ws-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(canonical(&d))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn uris(ws: &mut Workspace) -> Vec<String> {
        let mut v: Vec<String> = ws
            .sparql_documents()
            .iter()
            .map(|q| q.uri.clone())
            .collect();
        v.sort();
        v
    }

    fn query(ws: &mut Workspace, path: &Path) -> Arc<SparqlAnalysis> {
        let uri = path_to_uri(path).unwrap();
        ws.sparql_documents()
            .into_iter()
            .find(|q| q.uri == uri)
            .unwrap()
    }

    /// Without file watching, nothing reports a query file created later: the
    /// folders are listed again on each call, and the files that did not
    /// change are not parsed again.
    #[test]
    fn without_file_watching_new_query_files_are_found_on_the_next_call() {
        let root = Dir::new("rescan");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        ws.rescan_sparql = true;
        let a = root.0.join("a.rq");
        std::fs::write(&a, QUERY).unwrap();
        assert_eq!(uris(&mut ws), [path_to_uri(&a).unwrap()]);
        let kept = query(&mut ws, &a);

        let b = root.0.join("q").join("b.sparql");
        std::fs::create_dir_all(b.parent().unwrap()).unwrap();
        std::fs::write(&b, QUERY).unwrap();
        std::fs::create_dir_all(root.0.join("node_modules")).unwrap();
        std::fs::write(root.0.join("node_modules").join("x.rq"), QUERY).unwrap();
        let mut both = vec![path_to_uri(&a).unwrap(), path_to_uri(&b).unwrap()];
        both.sort();
        assert_eq!(uris(&mut ws), both);
        assert!(Arc::ptr_eq(&kept, &query(&mut ws, &a)), "a.rq parsed again");

        std::fs::remove_file(&b).unwrap();
        assert_eq!(uris(&mut ws), [path_to_uri(&a).unwrap()]);
    }

    /// With file watching, the index follows the watcher's events and reads
    /// no folder again.
    #[test]
    fn with_file_watching_new_query_files_come_from_the_events() {
        let root = Dir::new("watched");
        let mut ws = Workspace::new(vec![root.0.clone()], PrefixMap::default());
        assert!(!ws.rescan_sparql);
        let a = root.0.join("a.rq");
        std::fs::write(&a, QUERY).unwrap();
        assert_eq!(uris(&mut ws).len(), 1);
        let b = root.0.join("b.rq");
        std::fs::write(&b, QUERY).unwrap();
        assert_eq!(uris(&mut ws).len(), 1);
        ws.sparql_files_changed(std::slice::from_ref(&b));
        assert_eq!(uris(&mut ws).len(), 2);
    }
}
