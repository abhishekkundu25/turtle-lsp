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
//! knowledge outside any one document changed, whether SHACL and reasoning are due, which
//! indexed files to parse again. The workspace decides that; the caller (the
//! server) only acts on it.

use crate::analysis::{Analysis, Encoding, LineIndex, LineStarts, ParsePath, SparqlAnalysis};
use crate::diagnose::{self, DiagnosticInputs};
use crate::features::kept::Kept;
use crate::jsonld::ContextLoader;
use crate::jsonld::loader::{self, Bundled};
use crate::knowledge::{DocFacts, Knowledge, Source};
use crate::syntax::splice::{Edit, Window};
use crate::syntax::{PrefixMap, Severity, Span, Syntax, SyntaxError};
use crate::worker::catch;
use lsp_types::{Diagnostic, TextDocumentContentChangeEvent};
use rayon::prelude::*;
use rustc_hash::FxHashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod contexts;
pub(crate) mod kinds;
mod sparql_files;
mod uri;

use contexts::{ContextUses, DocContexts};
use kinds::is_watched_file;
pub(crate) use kinds::{is_catalog, is_import_source, is_rdf_file, is_sparql_name, is_sparql_path};
use sparql_files::SparqlFile;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use uri::real;
pub(crate) use uri::{
    as_uri, canonical, file_path_of, file_uri_name, file_url_of, normalize_uri, path_to_uri,
    uri_to_path,
};

pub(crate) const PREFIX_FILE: &str = "prefixes.ttl";
pub(crate) const SKIP_DIRS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    ".venv",
    "venv",
    "__pycache__",
];
pub(crate) const MAX_FILES: usize = 20_000;
/// Files larger than this are not indexed (an open file is still analyzed).
pub(crate) const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

/// What an open document is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DocKind {
    /// Turtle, TriG, N-Triples, N-Quads, JSON-LD or RDF/XML.
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
    /// [`Syntax::of`] decides from the language id, the name and `text`.
    pub(crate) fn of(language_id: Option<&str>, uri: &str, text: &str) -> DocKind {
        if language_id.is_some_and(|id| id.eq_ignore_ascii_case("sparql")) {
            return DocKind::Sparql;
        }
        let generic = language_id
            .and_then(Syntax::from_language_id)
            .is_none_or(|s| s == Syntax::Turtle);
        if generic && is_sparql_name(uri) {
            return DocKind::Sparql;
        }
        DocKind::Rdf(Syntax::of(language_id, uri, text))
    }
}

/// Which open documents a change makes due for diagnostics.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum Dirty {
    #[default]
    None,
    /// This document (the one edited).
    One(String),
    /// Every open document.
    AllOpen,
}

/// How a file on disk changed, as the client's watchers report it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Created,
    Changed,
    Deleted,
}

/// What a change to the workspace means beyond itself, for the caller to act
/// on (see the module doc). A plain value: a caller with nothing to act on
/// (`batch`, tests) drops it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Effects {
    /// The open documents whose diagnostics are due again.
    pub(crate) dirty: Dirty,
    /// Knowledge outside any one document changed (prefixes, the disk index,
    /// a context): every open document is due again, whatever `dirty` says,
    /// and so is every closed file shown by workspace diagnostics.
    pub(crate) knowledge: bool,
    /// The workspace's data changed: SHACL validates it again, and
    /// reasoning runs over it again.
    pub(crate) data: bool,
    /// Indexed files whose prefixes changed, to parse again (off the main
    /// loop if the caller can) with [`Workspace::prefix_jobs`].
    pub(crate) reparse: Vec<PathBuf>,
    /// RDF files that changed on disk and are not open, in the order
    /// reported: workspace diagnostics take them first, or forget them when
    /// they are gone.
    pub(crate) on_disk: Vec<PathBuf>,
    /// SPARQL files changed on disk: the closed files' diagnostics are due
    /// again, though knowledge did not change.
    pub(crate) closed_files: bool,
}

impl Effects {
    /// Knowledge outside any one document changed: every open document is due.
    pub(crate) fn knowledge() -> Effects {
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
    pub(crate) fn add(&mut self, other: Effects) {
        self.dirty = match (std::mem::take(&mut self.dirty), other.dirty) {
            (Dirty::None, d) | (d, Dirty::None) => d,
            (Dirty::One(a), Dirty::One(b)) if a == b => Dirty::One(a),
            _ => Dirty::AllOpen,
        };
        self.knowledge |= other.knowledge;
        self.data |= other.data;
        self.reparse.extend(other.reparse);
        self.on_disk.extend(other.on_disk);
        self.closed_files |= other.closed_files;
    }
}

/// Whether `uri` names a prefixes.ttl.
fn is_prefix_uri(uri: &str) -> bool {
    uri.ends_with(&format!("/{PREFIX_FILE}"))
}

pub(crate) struct Doc {
    pub(crate) text: String,
    pub(crate) version: i32,
    pub(crate) kind: DocKind,
    /// Where `kind` comes from.
    from: KindFrom,
    /// For an RDF document: its analysis, or what the next one builds on.
    analyzed: Analyzed,
    /// For a SPARQL document: its parse, until the next edit.
    query: Option<Arc<SparqlAnalysis>>,
    /// A cached copy of an import: the import already contributes its facts.
    cached_import: bool,
    /// For an RDF document: the findings of the checks it keeps between
    /// edits ([`Workspace::diagnostics`]).
    kept: Kept,
}

/// Where an open document's kind comes from.
enum KindFrom {
    /// The caller named it ([`Workspace::open_doc_as`]): it stays.
    Given,
    /// [`DocKind::of`] with this language id, the URI and the text: decided
    /// again whenever the text changes, so an `.owl` whose text turns from
    /// RDF/XML to Turtle is parsed as Turtle, as a fresh open would.
    Detected(Option<Box<str>>),
}

/// An open RDF document's analysis against its text: current, or what the
/// next analysis can build on.
enum Analyzed {
    /// Nothing to build on: not analyzed yet, or the whole text was replaced
    /// since (the next analysis starts from the text alone).
    No,
    /// Of the current text.
    Current(Arc<Analysis>),
    /// Of the current text, but its analysis panicked: the stand-in
    /// ([`failed_analysis`]) is not of the text, so it is nothing to build on.
    Failed(Arc<Analysis>),
    /// Of the current text, with prefixes or contexts that changed since.
    Outdated(Arc<Analysis>),
    /// Of the text before `edit` (the client's changes since, collapsed);
    /// `starts` are the line starts of the text now.
    Edited {
        last: Arc<Analysis>,
        edit: Edit,
        starts: LineStarts,
    },
}

impl Analyzed {
    /// The analysis of the current text, if it is up to date.
    fn current(&self) -> Option<&Arc<Analysis>> {
        match self {
            Analyzed::Current(a) | Analyzed::Failed(a) => Some(a),
            Analyzed::No | Analyzed::Outdated(_) | Analyzed::Edited { .. } => None,
        }
    }

    /// The prefixes or contexts the analysis used changed: it is to be made
    /// again, from the same text.
    fn outdate(&mut self) {
        *self = match std::mem::replace(self, Analyzed::No) {
            Analyzed::Current(a) => Analyzed::Outdated(a),
            Analyzed::Failed(_) | Analyzed::No => Analyzed::No,
            other @ (Analyzed::Outdated(_) | Analyzed::Edited { .. }) => other,
        };
    }

    /// The analysis before the client's edits since, and those edits.
    fn edited(&self) -> Option<(&Analysis, Edit)> {
        match self {
            Analyzed::Edited { last, edit, .. } => Some((last, *edit)),
            _ => None,
        }
    }

    /// The line starts of the document's text, when this keeps them.
    fn starts(&self) -> Option<&LineStarts> {
        match self {
            Analyzed::Current(a) | Analyzed::Outdated(a) => Some(a.lines.starts()),
            Analyzed::Edited { starts, .. } => Some(starts),
            Analyzed::No | Analyzed::Failed(_) => None,
        }
    }

    /// The bytes `range` of the document's text were replaced by `new`,
    /// leaving `text`. `scratch` holds the line starts when this keeps none
    /// ([`Analyzed::starts`]) and is moved instead.
    fn replaced(
        &mut self,
        text: &str,
        range: std::ops::Range<usize>,
        new: &str,
        scratch: &mut Option<LineStarts>,
    ) {
        *self = match std::mem::replace(self, Analyzed::No) {
            Analyzed::Current(last) | Analyzed::Outdated(last) => {
                let mut starts = last.lines.starts().clone();
                starts.replace(text, range.clone(), new);
                let edit = Edit::new(range, new.len());
                Analyzed::Edited { last, edit, starts }
            }
            Analyzed::Edited {
                last,
                edit,
                mut starts,
            } => {
                starts.replace(text, range.clone(), new);
                let edit = edit.then(range, new.len());
                Analyzed::Edited { last, edit, starts }
            }
            Analyzed::No | Analyzed::Failed(_) => {
                if let Some(starts) = scratch {
                    starts.replace(text, range, new);
                }
                Analyzed::No
            }
        };
    }

    /// The line index of `text`, the document's text: from the last
    /// analysis's where there is one.
    fn lines(&self, text: &str) -> LineIndex {
        match self {
            Analyzed::Current(a) | Analyzed::Outdated(a) => a.lines.clone(),
            Analyzed::Edited { last, edit, starts } => last.lines.edited(text, edit, starts),
            Analyzed::No | Analyzed::Failed(_) => LineIndex::new(text),
        }
    }
}

#[derive(Default)]
pub(crate) struct Workspace {
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
    /// for, among the analyses the workspace kept. Never shared: a parse on
    /// another thread returns its list ([`Parsed`]), recorded if the parse
    /// is kept.
    context_uses: ContextUses,
    /// How many times a context changed for the indexed files (saved,
    /// closed, changed on disk, fetched, a new loader). A parse that began
    /// before one asked for contexts its record did not hold yet
    /// ([`Workspace::add_reparsed`]).
    context_changes: u64,
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
    /// Analyze open documents without keeping their tokens (see
    /// [`Workspace::set_keep_tokens`]).
    drop_tokens: bool,
    reparse: Reparse,
    /// How each analysis of an open document was parsed, while the splice
    /// is checked ([`Workspace::set_check_incremental`]).
    parse_log: Vec<ParseRecord>,
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
    let attempt = caught(text, || {
        let (uri, external) = (uri.clone(), external.clone());
        Analysis::with_loader(uri, text, external, keep_tokens, syntax, loader)
    });
    attempt.unwrap_or_else(|msg| failed_analysis(uri, syntax, external, &msg))
}

/// What `work` returns from analyzing `text`, or the message of its panic.
fn caught<T>(text: &str, work: impl FnOnce() -> T) -> Result<T, String> {
    catch("analysis", || {
        #[cfg(test)]
        if text.contains(PANIC_IN_ANALYSIS) {
            panic!("injected analysis panic");
        }
        #[cfg(not(test))]
        let _ = text;
        work()
    })
}

/// How the workspace parses an open document after an edit.
#[derive(Clone, Copy, Default)]
struct Reparse {
    /// In full every time (the setting `parse.incremental` off).
    off: bool,
    /// Check every splice against a full parse, keep the full one when they
    /// differ, and record each analysis's path
    /// ([`Workspace::set_check_incremental`]).
    check: bool,
}

/// How one analysis of an open document was parsed, recorded when the
/// workspace checks the splice ([`Workspace::set_check_incremental`]).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ParseRecord {
    pub(crate) uri: String,
    pub(crate) path: ParsePath,
    /// The first field in which the spliced analysis differed from the
    /// full one, which the document got instead.
    pub(crate) difference: Option<String>,
}

/// An analysis of an open document: what the document keeps, its facts
/// unless it is a cached import, and how it was parsed.
struct OpenAnalysis {
    analyzed: Analyzed,
    facts: Option<DocFacts>,
    path: ParsePath,
    difference: Option<String>,
    /// What the splice parsed again, when the analysis is the splice's.
    window: Option<Window>,
}

/// [`analyze`] for the open document `doc`, building on its last analysis
/// where it can (its line index, and its parse through the statement
/// splice), with its facts unless it is a cached import. A panic in either
/// gives a [`failed_analysis`].
fn analyze_open(
    uri: &str,
    doc: &Doc,
    external: Arc<PrefixMap>,
    keep_tokens: bool,
    syntax: Syntax,
    loader: &dyn ContextLoader,
    reparse: Reparse,
) -> OpenAnalysis {
    let mut path = ParsePath::Whole;
    let mut window = None;
    let attempt = caught(&doc.text, || {
        let text = &doc.text;
        let lines = doc.analyzed.lines(text);
        let parsed = match doc.analyzed.edited() {
            None => None,
            Some(_) if reparse.off => {
                path = ParsePath::Off;
                None
            }
            Some((last, edit)) => match last.splice(uri, text, edit, &external) {
                Ok((p, w)) => {
                    path = ParsePath::Spliced;
                    window = Some(w);
                    Some(p)
                }
                Err(f) => {
                    path = ParsePath::Fallback(f);
                    None
                }
            },
        };
        let (u, ext) = (uri.to_string(), external.clone());
        let a = match parsed {
            Some(p) => Analysis::with_parse(u, text, ext, keep_tokens, p, lines),
            None => Analysis::with_lines(u, text, ext, keep_tokens, syntax, loader, lines),
        };
        if !(reparse.check && path == ParsePath::Spliced) {
            return (a, None);
        }
        // From the text alone, the line index too.
        let (u, ext) = (uri.to_string(), external.clone());
        let full = Analysis::with_loader(u, text, ext, keep_tokens, syntax, loader);
        match crate::analysis::same_analysis(&a, &full) {
            Ok(()) => (a, None),
            Err(diff) => {
                window = None;
                (full, Some(diff))
            }
        }
    })
    .and_then(|(a, difference)| match doc.cached_import {
        true => Ok((a, None, difference)),
        false => facts(&a).map(|f| (a, Some(f), difference)),
    });
    match attempt {
        Ok((a, facts, difference)) => OpenAnalysis {
            analyzed: Analyzed::Current(Arc::new(a)),
            facts,
            path,
            difference,
            window,
        },
        Err(msg) => {
            let failed = failed_analysis(uri.to_string(), syntax, external, &msg);
            let facts = (!doc.cached_import).then(|| DocFacts::new(&failed));
            OpenAnalysis {
                analyzed: Analyzed::Failed(Arc::new(failed)),
                facts,
                path,
                difference: None,
                window: None,
            }
        }
    }
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

/// The facts of `a` for the knowledge index, or the message of the panic
/// that computing them ended in.
fn facts(a: &Analysis) -> Result<DocFacts, String> {
    catch("indexing", || {
        #[cfg(test)]
        if a.text.contains(PANIC_IN_FACTS) {
            panic!("injected facts panic");
        }
        DocFacts::new(a)
    })
}

/// `a` with its facts for the knowledge index. When the facts cannot be
/// computed, a [`failed_analysis`] stands in for it.
fn prepared(a: Analysis) -> (Analysis, DocFacts) {
    match facts(&a) {
        Ok(facts) => (a, facts),
        Err(msg) => {
            let failed = failed_analysis(a.uri.clone(), a.parsed.syntax, a.external.clone(), &msg);
            let facts = DocFacts::new(&failed);
            (failed, facts)
        }
    }
}

impl Workspace {
    pub(crate) fn new(roots: Vec<PathBuf>, settings_prefixes: PrefixMap) -> Workspace {
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
    pub(crate) fn set_rescan_sparql(&mut self, rescan: bool) {
        self.rescan_sparql = rescan;
    }

    /// Whether the analyses of open documents keep their tokens (the
    /// default). Without them an analysis lexes its text again the first
    /// time a feature asks for them ([`Analysis::tokens`]): the choice of a
    /// caller that analyzes each document once, such as `batch`.
    pub(crate) fn set_keep_tokens(&mut self, keep: bool) {
        self.drop_tokens = !keep;
    }

    /// Whether an edit to an open document is parsed with the statement
    /// splice (the default) or in full (the setting `parse.incremental`).
    /// The next analysis goes by it.
    pub(crate) fn set_incremental(&mut self, on: bool) {
        self.reparse.off = !on;
    }

    /// Check every spliced analysis against a full one, give the document
    /// the full one when they differ, and record how each analysis of an
    /// open document was parsed ([`Workspace::take_parse_log`]): for tests
    /// and debugging, as it doubles the work of each analysis.
    pub(crate) fn set_check_incremental(&mut self, on: bool) {
        self.reparse.check = on;
    }

    /// The analyses recorded since the last call, while the splice is
    /// checked.
    pub(crate) fn take_parse_log(&mut self) -> Vec<ParseRecord> {
        std::mem::take(&mut self.parse_log)
    }

    // ---- state ----

    /// The workspace folders, in the order the client gave them.
    pub(crate) fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// The prefixes the settings declare (`turtle.prefixes`).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn settings_prefixes(&self) -> &PrefixMap {
        &self.settings_prefixes
    }

    /// Replace the settings prefixes: every open document is analyzed again,
    /// and the indexed files whose prefixes differ now are to be parsed
    /// again.
    pub(crate) fn set_settings_prefixes(&mut self, prefixes: PrefixMap) -> Effects {
        self.settings_prefixes = prefixes;
        Effects {
            data: true,
            reparse: self.prefixes_changed(None),
            ..Effects::knowledge()
        }
    }

    /// Whether `uri` is an open document.
    pub(crate) fn is_open(&self, uri: &str) -> bool {
        self.open.contains_key(uri)
    }

    /// The open documents' URIs, in no particular order.
    pub(crate) fn open_uris(&self) -> impl Iterator<Item = &str> {
        self.open.keys().map(String::as_str)
    }

    /// The version the client gave the open document `uri`.
    pub(crate) fn version(&self, uri: &str) -> Option<i32> {
        self.open.get(uri).map(|d| d.version)
    }

    /// What the open document `uri` is.
    pub(crate) fn doc_kind(&self, uri: &str) -> Option<DocKind> {
        self.open.get(uri).map(|d| d.kind)
    }

    /// Whether `uri` has an indexed copy from disk. An open RDF document
    /// that is not a cached import has none: its open text replaces the
    /// copy until it is closed ([`Workspace::open_doc_as`]).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn is_indexed(&self, uri: &str) -> bool {
        self.disk.contains_key(uri)
    }

    /// The URIs of the indexed copies from disk, in no particular order.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn indexed_uris(&self) -> impl Iterator<Item = &str> {
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

    /// An open document that is a cached copy of an import: analyzed, but
    /// neither registered with the knowledge index nor diagnosed.
    pub(crate) fn is_cached_import(&self, uri: &str) -> bool {
        self.open.get(uri).is_some_and(|d| d.cached_import)
    }

    // ---- documents ----

    /// The kind comes from the URI and the text ([`DocKind::of`]), and is
    /// decided again at each change of the text.
    pub(crate) fn open_doc(&mut self, uri: String, text: String, version: i32) -> Effects {
        self.open_doc_in(uri, text, version, None)
    }

    /// [`Workspace::open_doc`] with the client's language id, which
    /// [`DocKind::of`] reads too.
    pub(crate) fn open_doc_in(
        &mut self,
        uri: String,
        text: String,
        version: i32,
        language_id: Option<&str>,
    ) -> Effects {
        let kind = DocKind::of(language_id, &uri, &text);
        let from = KindFrom::Detected(language_id.map(Box::from));
        self.open_doc_from(uri, text, version, kind, from)
    }

    /// Open a document of the kind `kind`, which stays whatever the text
    /// becomes. The document is due for diagnostics; opening a prefixes.ttl
    /// or a context file is an edit of it ([`Workspace::change_doc`]).
    pub(crate) fn open_doc_as(
        &mut self,
        uri: String,
        text: String,
        version: i32,
        kind: impl Into<DocKind>,
    ) -> Effects {
        self.open_doc_from(uri, text, version, kind.into(), KindFrom::Given)
    }

    fn open_doc_from(
        &mut self,
        uri: String,
        text: String,
        version: i32,
        kind: DocKind,
        from: KindFrom,
    ) -> Effects {
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
                from,
                analyzed: Analyzed::No,
                query: None,
                cached_import,
                kept: Kept::default(),
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
    pub(crate) fn change_doc(
        &mut self,
        uri: &str,
        version: i32,
        changes: Vec<TextDocumentContentChangeEvent>,
        enc: Encoding,
    ) -> Effects {
        if let Some(doc) = self.open.get_mut(uri) {
            // Each change's positions are in the text the changes before it
            // left. The line starts are those of the last analysis, or found
            // once, when the first ranged change comes, and moved along with
            // each change; the analysis keeps them, and the changes as one
            // edit, for the next analysis.
            let mut scratch: Option<LineStarts> = None;
            for ch in changes {
                match ch.range {
                    None => {
                        doc.text = ch.text;
                        doc.analyzed = Analyzed::No;
                        scratch = None;
                    }
                    Some(r) => {
                        let lines = match doc.analyzed.starts() {
                            Some(starts) => starts,
                            None => scratch.get_or_insert_with(|| LineStarts::new(&doc.text)),
                        };
                        let (Some(s), Some(e)) = (
                            lines.offset(&doc.text, enc, r.start),
                            lines.offset(&doc.text, enc, r.end),
                        ) else {
                            continue;
                        };
                        let (s, e) = (s.min(e) as usize, e.max(s) as usize);
                        let range = s.min(doc.text.len())..e.min(doc.text.len());
                        doc.text.replace_range(range.clone(), &ch.text);
                        doc.analyzed
                            .replaced(&doc.text, range, &ch.text, &mut scratch);
                    }
                }
            }
            doc.version = version;
            doc.query = None;
            // A text whose syntax its content gives may now be in another:
            // what was analyzed is nothing to build on.
            if let KindFrom::Detected(id) = &doc.from {
                let kind = DocKind::of(id.as_deref(), uri, &doc.text);
                if kind != doc.kind {
                    doc.kind = kind;
                    doc.analyzed = Analyzed::No;
                }
            }
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
    /// file parses the indexed files that use it again, and SHACL and reasoning are due.
    pub(crate) fn save_doc(&mut self, uri: &str) -> Effects {
        let mut fx = Effects {
            data: true,
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
    /// ([`Workspace::reindex_disk_file`]). Closing a SPARQL document leaves
    /// the index untouched. Unsaved edits to a context file or a prefixes.ttl
    /// are gone with the buffer: what they changed changes back.
    pub(crate) fn close_doc(&mut self, uri: &str) -> Effects {
        let gone = self.open.remove(uri);
        if !gone.is_some_and(|d| d.cached_import || d.kind == DocKind::Sparql) {
            // Only what the scan would index goes back to the index: an RDF file
            // below a root (a .txt opened as Turtle, or a file elsewhere, does not).
            if let Some(path) = uri_to_path(uri).filter(|p| is_rdf_file(p)) {
                self.reindex_disk_file(&path);
            }
            self.sync(uri);
        }
        if !self.disk.contains_key(uri) {
            // No copy left to use contexts (an `untitled:` document, a file
            // the index does not hold).
            self.record_uses(uri, Vec::new());
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
                if let Some(a) = d.analyzed.current() {
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
    pub(crate) fn knowledge(&mut self) -> &Knowledge {
        let stale: Vec<(String, Syntax)> = self
            .open
            .iter()
            .filter_map(|(u, d)| match d.kind {
                DocKind::Rdf(s) if d.analyzed.current().is_none() => Some((u.clone(), s)),
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
        let keep_tokens = !self.drop_tokens;
        let reparse = self.reparse;
        let contexts = self.contexts();
        let done: Vec<(String, OpenAnalysis)> = jobs
            .into_par_iter()
            .filter_map(|(uri, syntax, ext)| {
                let doc = open.get(&uri)?;
                let loader = DocContexts::new(open, roots, &*contexts);
                let a = analyze_open(&uri, doc, ext, keep_tokens, syntax, &loader, reparse);
                loader.record(uses, &uri, syntax);
                Some((uri, a))
            })
            .collect();
        for (uri, a) in done {
            self.settle(&uri, a);
        }
        &self.knowledge
    }

    /// Read-only sources (e.g. imported ontologies): they add facts to the
    /// knowledge index but are never diagnosed.
    pub(crate) fn knowledge_mut(&mut self) -> &mut Knowledge {
        &mut self.knowledge
    }

    /// The RDF analysis of `uri`: the open document's, else the indexed
    /// copy's. `None` for a SPARQL document (see [`Workspace::sparql`]).
    pub(crate) fn analysis(&mut self, uri: &str) -> Option<Arc<Analysis>> {
        let syntax = match self.open.get(uri) {
            Some(d) if d.analyzed.current().is_some() => return d.analyzed.current().cloned(),
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
        let doc = self.open.get(uri)?;
        let keep_tokens = !self.drop_tokens;
        let a = analyze_open(
            uri,
            doc,
            external,
            keep_tokens,
            syntax,
            &loader,
            self.reparse,
        );
        loader.record(&self.context_uses, uri, syntax);
        self.settle(uri, a)
    }

    /// Give the open document `uri` its new analysis, and the knowledge
    /// index its facts; the analysis, unless it failed.
    fn settle(&mut self, uri: &str, a: OpenAnalysis) -> Option<Arc<Analysis>> {
        let OpenAnalysis {
            analyzed,
            facts,
            path,
            difference,
            window,
        } = a;
        if self.reparse.check {
            let uri = uri.to_string();
            self.parse_log.push(ParseRecord {
                uri,
                path,
                difference,
            });
        }
        let current = analyzed.current().cloned();
        if let (Some(f), Some(a)) = (facts, &current) {
            self.knowledge.set_prepared(uri, Source::Open, a.clone(), f);
        }
        let doc = self.open.get_mut(uri)?;
        match (window, doc.analyzed.edited(), &analyzed) {
            (Some(w), Some((last, _)), Analyzed::Current(now)) => doc.kept.spliced(last, w, now),
            _ => doc.kept.drop_all(),
        }
        doc.analyzed = analyzed;
        current
    }

    /// The diagnostics of the open RDF document `uri`, whose analysis is
    /// `a`, as [`diagnose::document`] composes them, with the findings the
    /// document keeps between edits ([`Kept`]); in full when the setting
    /// `parse.incremental` is off.
    pub(crate) fn diagnostics(
        &mut self,
        uri: &str,
        a: &Arc<Analysis>,
        enc: Encoding,
        inputs: &DiagnosticInputs,
    ) -> Vec<Diagnostic> {
        // The other open documents edited since, as `knowledge` brings in.
        self.knowledge();
        let k = &self.knowledge;
        let kept = match self.open.get_mut(uri) {
            Some(d) if !self.reparse.off => Some(&mut d.kept),
            _ => None,
        };
        diagnose::rdf(a, k, enc, inputs, kept)
    }

    /// The text of the open document `uri`.
    #[cfg(test)]
    pub(crate) fn text(&self, uri: &str) -> Option<&str> {
        self.open.get(uri).map(|d| d.text.as_str())
    }

    // ---- prefixes ----

    /// The prefixes a document at `uri` gets, by its name alone: a name whose
    /// syntax its content gives counts as Turtle here, and a parse whose
    /// text turns out otherwise drops them ([`parse_files`]).
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
    pub(crate) fn find_prefix_file(&self, dir: &Path) -> Option<PathBuf> {
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
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn scan_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
        scan_roots_matching(roots, is_rdf_file).paths
    }

    /// Whether the file at `path` belongs in the disk index, as the scan
    /// reads the roots: an RDF file below a root, in no hidden or skipped
    /// ([`SKIP_DIRS`]) directory, of at most [`MAX_FILE_BYTES`]. The caller
    /// keeps the index within [`MAX_FILES`] files. The scan, a file that
    /// changed on disk and a document that was closed all ask this.
    fn fits(&self, path: &Path) -> bool {
        is_rdf_file(path) && self.scan_reaches(path)
    }

    /// Whether the scan of the roots reaches the file at `path`: below a
    /// root, in no hidden or skipped directory, through no symbolic link
    /// (the scan follows none, so a link in the workspace to a file or
    /// folder outside it is not read), and a file of at most
    /// [`MAX_FILE_BYTES`].
    fn scan_reaches(&self, path: &Path) -> bool {
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
                let text = std::fs::read_to_string(&path).ok()?;
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

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod review_tests;
