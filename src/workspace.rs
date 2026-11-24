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
use std::path::PathBuf;
use std::sync::Arc;

mod contexts;
mod disk;
pub(crate) mod kinds;
mod prefixes;
mod sparql_files;
mod uri;

use contexts::{ContextUses, DocContexts};
pub(crate) use disk::{Parsed, Reparsed, scan_roots_matching};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use disk::{Scan, scan_roots_capped};
use disk::{Stamp, Touched, stamp_of};
pub(crate) use kinds::{is_catalog, is_import_source, is_rdf_file, is_sparql_name, is_sparql_path};
pub(crate) use prefixes::find_prefix_file;
use prefixes::{PrefixEntry, is_prefix_uri};
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
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod review_tests;
