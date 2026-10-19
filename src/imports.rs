//! `owl:imports` resolution.
//!
//! Every `owl:imports <iri>` in the workspace is resolved, in this order:
//! 1. an ontology the workspace describes (`<iri> a owl:Ontology`, or some
//!    ontology's `owl:versionIRI <iri>`): nothing to load;
//! 2. a `catalog-v001.xml` (Protégé convention) anywhere under the workspace
//!    roots, mapping the IRI to a file (relative to the catalog) or a URL;
//! 3. the per-user import cache;
//! 4. the network, when `turtle.imports.network` is on or the user asked to
//!    fetch this import ([`Imports::fetch`]); then cached.
//!
//! Loaded ontologies are resolved transitively (up to [`MAX_DEPTH`] levels
//! below a workspace import, cycles stop) and join the knowledge index as
//! read-only external sources: hover, completion and lints know their terms,
//! but they are never diagnosed. Turtle and N-Triples are read with our parser;
//! RDF/XML is converted to N-Triples text first, so every source is an
//! [`Analysis`] like any other.
//!
//! All file and network work happens on one background thread, started on the
//! first import and blocked on its job channel when there is nothing to do.
//! Each job carries a generation; a newer job supersedes an unfinished one and
//! results of older jobs are dropped. The main loop selects on
//! [`Imports::results`].

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::knowledge::{DocFacts, Knowledge, Source};
use crate::syntax::{self, Iri, Node, PrefixMap};
use crate::workspace::{MAX_FILE_BYTES, MAX_FILES, SKIP_DIRS, path_to_uri};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOrCommand, CodeActionParams, Command, Diagnostic,
    DiagnosticSeverity, NumberOrString,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::{BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime};

pub const OWL_IMPORTS: &str = "http://www.w3.org/2002/07/owl#imports";
const OWL_ONTOLOGY: &str = "http://www.w3.org/2002/07/owl#Ontology";
const OWL_VERSION_IRI: &str = "http://www.w3.org/2002/07/owl#versionIRI";

pub const CATALOG_FILE: &str = "catalog-v001.xml";
/// Levels of imports followed below a workspace import.
pub const MAX_DEPTH: u32 = 5;
pub const MAX_FETCH_BYTES: u64 = 20 * 1024 * 1024;
pub const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
#[cfg(not(target_arch = "wasm32"))]
const ACCEPT: &str = "text/turtle, application/n-triples;q=0.9, application/rdf+xml;q=0.8";
/// RDF/XML with a longer start tag is refused: quick-xml's cost per start tag
/// grows faster than its size (RUSTSEC-2026-0194, RUSTSEC-2026-0195).
const MAX_XML_TAG_BYTES: usize = 64 * 1024;
/// Entity declarations allowed in an RDF/XML DOCTYPE, and their total size.
const MAX_XML_ENTITIES: usize = 256;
const MAX_XML_ENTITY_BYTES: usize = 64 * 1024;
/// Prefix for external source keys in the knowledge index.
const KEY_PREFIX: &str = "import:";
/// Command that fetches one import (arguments: `[iri]`), offered as a quick fix
/// on its unresolved-import diagnostic.
pub const FETCH_COMMAND: &str = "turtle.fetchImport";

/// How an import was resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Described in the workspace.
    Local,
    /// Loaded from this file or URL.
    Loaded(String),
    /// Not loaded; the message says why and how to fix it.
    Unresolved(String),
}

/// Everything a job needs; two equal requests give the same answer.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Request {
    /// Imports stated in workspace documents (sorted, unique).
    imports: Vec<String>,
    /// Ontology IRIs the workspace describes (sorted, unique).
    local: Vec<String>,
    roots: Vec<PathBuf>,
    cache: Option<PathBuf>,
    network: bool,
    /// Imports the user asked to fetch, network or not (sorted, unique).
    fetch: Vec<String>,
}

struct Job {
    generation: u64,
    req: Request,
    /// An import to fetch again now, even if fetching it failed lately.
    refetch: Option<String>,
}

/// One loaded ontology.
pub struct Loaded {
    key: String,
    analysis: Arc<Analysis>,
    /// Computed on the worker when the ontology was parsed for this job.
    facts: Option<DocFacts>,
}

/// The answer to one job.
pub struct Done {
    generation: u64,
    status: Vec<(String, Status)>,
    loaded: Vec<Loaded>,
}

/// What one workspace document says about ontologies.
#[derive(Default)]
struct DocOnto {
    imports: Vec<Iri>,
    ontologies: Vec<Iri>,
}

struct Preds {
    imports: Iri,
    rdf_type: Iri,
    ontology: Iri,
    version_iri: Iri,
}

fn preds() -> &'static Preds {
    static P: OnceLock<Preds> = OnceLock::new();
    P.get_or_init(|| Preds {
        imports: Iri::intern(OWL_IMPORTS),
        rdf_type: Iri::intern(syntax::RDF_TYPE),
        ontology: Iri::intern(OWL_ONTOLOGY),
        version_iri: Iri::intern(OWL_VERSION_IRI),
    })
}

fn doc_onto(a: &Analysis) -> DocOnto {
    let p = preds();
    let mut d = DocOnto::default();
    for t in &a.parsed.triples {
        match (&t.s, &t.o) {
            (_, Node::Iri(o)) if t.p == p.imports => d.imports.push(*o),
            (Node::Iri(s), Node::Iri(o)) if t.p == p.rdf_type && *o == p.ontology => {
                d.ontologies.push(*s)
            }
            (_, Node::Iri(o)) if t.p == p.version_iri => d.ontologies.push(*o),
            _ => {}
        }
    }
    d
}

/// Why an import is not loaded where nothing can be (the browser build).
pub fn unloaded_note(iri: &str) -> String {
    format!(
        "Import <{iri}> is not loaded: imports are not followed in the browser. Add the imported ontology's file to the workspace to use it."
    )
}

/// The workspace's imports that stay unresolved when nothing can be loaded
/// (the browser build): every `owl:imports` no workspace document declares
/// as its ontology or version IRI, sorted. What [`Imports::unresolved`]
/// reports when every load fails.
pub fn unresolved_without_loading(k: &Knowledge) -> Vec<String> {
    let (mut imports, mut local) = (BTreeSet::new(), FxHashSet::default());
    for (_, _, a) in k.docs().filter(|(_, src, _)| *src != Source::External) {
        let d = doc_onto(a);
        imports.extend(d.imports.iter().map(|i| i.to_string()));
        local.extend(d.ontologies.iter().map(|i| norm(i).to_string()));
    }
    imports
        .into_iter()
        .filter(|i| !local.contains(norm(i)))
        .collect()
}

/// `owl:imports` state of the server: what was asked, what came back.
pub struct Imports {
    /// Resolve without loading or threads (see [`Imports::new_inline`]).
    inline: bool,
    jobs: Option<Sender<Job>>,
    results_tx: Sender<Done>,
    results: Receiver<Done>,
    generation: u64,
    /// The generation of the last answer taken (see [`Imports::busy`]).
    answered: u64,
    last: Option<Request>,
    /// Knowledge generation and network setting the last request was built from.
    seen: Option<(u64, bool)>,
    status: FxHashMap<String, Status>,
    /// External source keys registered with the knowledge index.
    registered: FxHashSet<String>,
    /// Workspace URI -> its analysis when [`DocOnto`] was computed.
    per_doc: FxHashMap<String, (Weak<Analysis>, DocOnto)>,
    cache: Option<PathBuf>,
    /// Imports the user asked to fetch this session.
    fetch: BTreeSet<String>,
    /// For the next job: see [`Job::refetch`].
    refetch: Option<String>,
}

impl Default for Imports {
    fn default() -> Self {
        Imports::new()
    }
}

impl Imports {
    pub fn new() -> Imports {
        let (results_tx, results) = unbounded();
        Imports {
            inline: false,
            jobs: None,
            results_tx,
            results,
            generation: 0,
            answered: 0,
            last: None,
            seen: None,
            status: FxHashMap::default(),
            registered: FxHashSet::default(),
            per_doc: FxHashMap::default(),
            cache: cache_dir().map(|d| d.join("imports")),
            fetch: BTreeSet::new(),
            refetch: None,
        }
    }

    /// Imports resolved at once and without loading anything (the browser
    /// build): one the workspace declares is present, any other unresolved,
    /// with [`unloaded_note`].
    pub fn new_inline() -> Imports {
        Imports {
            inline: true,
            cache: None,
            ..Imports::new()
        }
    }

    /// Answers from the background thread, for the main loop to select on.
    pub fn results(&self) -> &Receiver<Done> {
        &self.results
    }

    /// A resolution pass is running: its answer has not been taken yet.
    pub fn busy(&self) -> bool {
        self.answered != self.generation
    }

    pub fn status(&self, iri: &str) -> Option<&Status> {
        self.status.get(iri)
    }

    /// The workspace's imports that are not loaded, sorted.
    pub fn unresolved(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .status
            .iter()
            .filter(|(_, s)| matches!(s, Status::Unresolved(_)))
            .map(|(i, _)| i.clone())
            .collect();
        v.sort();
        v
    }

    /// Look at the workspace's imports and start resolving them if they (or
    /// the setting) changed. Cheap when nothing did. With no imports at all
    /// the answer is immediate and returned.
    pub fn sync(&mut self, k: &Knowledge, roots: &[PathBuf], network: bool) -> Option<Done> {
        let seen = Some((k.generation(), network));
        if self.seen == seen {
            return None;
        }
        self.seen = seen;
        let mut ids: Vec<_> = [OWL_IMPORTS, OWL_ONTOLOGY, OWL_VERSION_IRI]
            .iter()
            .flat_map(|p| k.docs_referencing(*p))
            .filter(|(_, src, _)| *src != Source::External)
            .map(|(id, _, _)| id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        let mut per_doc = FxHashMap::default();
        let (mut imports, mut local) = (FxHashSet::default(), FxHashSet::default());
        for id in ids {
            let uri = &k.doc(id).uri;
            let Some(a) = k.get(uri) else { continue };
            let entry = match self.per_doc.remove(uri) {
                Some((w, d)) if std::ptr::eq(w.as_ptr(), Arc::as_ptr(a)) => (w, d),
                _ => (Arc::downgrade(a), doc_onto(a)),
            };
            imports.extend(entry.1.imports.iter().map(|i| i.to_string()));
            local.extend(entry.1.ontologies.iter().map(|i| norm(i).to_string()));
            per_doc.insert(uri.clone(), entry);
        }
        self.per_doc = per_doc;
        let sorted = |s: FxHashSet<String>| {
            let mut v: Vec<String> = s.into_iter().collect();
            v.sort();
            v
        };
        let req = Request {
            imports: sorted(imports),
            local: sorted(local),
            roots: roots.to_vec(),
            cache: self.cache.clone(),
            network,
            fetch: self.fetch.iter().cloned().collect(),
        };
        if self.last.as_ref() == Some(&req) {
            return None;
        }
        self.generation += 1;
        let empty = req.imports.is_empty();
        self.last = Some(req.clone());
        if empty {
            return Some(Done {
                generation: self.generation,
                status: vec![],
                loaded: vec![],
            });
        }
        if self.inline {
            let local: FxHashSet<&str> = req.local.iter().map(String::as_str).collect();
            let status = req
                .imports
                .iter()
                .map(|i| {
                    let st = if local.contains(norm(i)) {
                        Status::Local
                    } else {
                        Status::Unresolved(unloaded_note(i))
                    };
                    (i.clone(), st)
                })
                .collect();
            return Some(Done {
                generation: self.generation,
                status,
                loaded: vec![],
            });
        }
        let job = Job {
            generation: self.generation,
            req,
            refetch: self.refetch.take(),
        };
        let jobs = self.jobs.get_or_insert_with(|| {
            let (tx, rx) = unbounded();
            let out = self.results_tx.clone();
            let _ = std::thread::Builder::new()
                .name("turtle-lsp-imports".into())
                .stack_size(16 << 20)
                .spawn(move || worker(rx, out));
            tx
        });
        let _ = jobs.send(job);
        None
    }

    /// Something on disk may have changed (a file was saved): resolve the
    /// imports again on the next [`Imports::sync`].
    pub fn retry(&mut self) {
        // Loaded files may have changed too; unchanged ones come from the memo.
        self.last = None;
        self.seen = None;
    }

    /// The user asked to fetch `iri` (with [`FETCH_COMMAND`]): allow fetching
    /// it for the rest of the session, and try it on the next
    /// [`Imports::sync`] even if it failed a moment ago.
    pub fn fetch(&mut self, iri: &str) {
        self.fetch.insert(iri.to_string());
        self.refetch = Some(iri.to_string());
        self.last = None;
        self.seen = None;
    }

    /// Take a job's answer: register loaded ontologies with the knowledge
    /// index. Returns whether anything visible changed.
    pub fn apply(&mut self, done: Done, k: &mut Knowledge) -> bool {
        if done.generation != self.generation {
            return false;
        }
        self.answered = done.generation;
        let mut changed = false;
        let keep: FxHashSet<String> = done.loaded.iter().map(|l| l.key.clone()).collect();
        for key in self.registered.difference(&keep) {
            k.remove_external(key);
            changed = true;
        }
        for l in done.loaded {
            if k.has_external(&l.key, &l.analysis) {
                continue;
            }
            match l.facts {
                Some(f) => k.add_external_prepared(&l.key, l.analysis, f),
                None => k.add_external(&l.key, l.analysis),
            }
            changed = true;
        }
        self.registered = keep;
        let status: FxHashMap<String, Status> = done.status.into_iter().collect();
        if status != self.status {
            self.status = status;
            changed = true;
        }
        changed
    }

    /// Information diagnostics on the unresolved imports of `a`.
    pub fn diagnostics(&self, a: &Analysis, enc: Encoding) -> Vec<Diagnostic> {
        diagnostics_with(a, enc, |iri| self.status.get(iri))
    }
}

/// [`Imports::diagnostics`] with the status looked up by `status`.
pub fn diagnostics_with<'s>(
    a: &Analysis,
    enc: Encoding,
    status: impl Fn(&str) -> Option<&'s Status>,
) -> Vec<Diagnostic> {
    if !a.refs_by_iri.contains_key(OWL_IMPORTS) {
        return vec![];
    }
    let p = preds();
    a.parsed
        .triples
        .iter()
        .filter(|t| t.p == p.imports)
        .filter_map(|t| {
            let Node::Iri(o) = &t.o else { return None };
            let Some(Status::Unresolved(message)) = status(o) else {
                return None;
            };
            Some(Diagnostic {
                range: a.range(enc, t.o_span),
                severity: Some(DiagnosticSeverity::INFORMATION),
                code: Some(NumberOrString::String("unresolved-import".into())),
                source: Some(SOURCE.to_string()),
                message: message.clone(),
                ..Default::default()
            })
        })
        .collect()
}

/// "Fetch <iri> and cache it" quick fixes for the unresolved web imports among
/// the diagnostics of `params`; each runs [`FETCH_COMMAND`].
pub fn fetch_actions(
    a: &Analysis,
    enc: Encoding,
    params: &CodeActionParams,
) -> Vec<CodeActionOrCommand> {
    let only = params.context.only.as_deref();
    if !crate::features::refactor::wanted(only, &CodeActionKind::QUICKFIX) {
        return vec![];
    }
    let p = preds();
    let mut out = Vec::new();
    for d in &params.context.diagnostics {
        if d.source.as_deref() != Some(SOURCE)
            || d.code != Some(NumberOrString::String("unresolved-import".into()))
        {
            continue;
        }
        let Some(iri) = a.parsed.triples.iter().find_map(|t| match &t.o {
            Node::Iri(o) if t.p == p.imports && a.range(enc, t.o_span) == d.range => Some(*o),
            _ => None,
        }) else {
            continue;
        };
        if !(iri.starts_with("http://") || iri.starts_with("https://")) {
            continue;
        }
        out.push(CodeActionOrCommand::CodeAction(CodeAction {
            title: format!("Fetch {iri} and cache it"),
            kind: Some(CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![d.clone()]),
            command: Some(Command {
                title: format!("Fetch {iri} and cache it"),
                command: FETCH_COMMAND.to_string(),
                arguments: Some(vec![serde_json::Value::String(iri.to_string())]),
            }),
            ..Default::default()
        }));
    }
    out
}

// ---- background thread ----

#[derive(Default)]
struct Memo {
    /// Path -> (modified, length, base IRI, parsed ontology).
    files: FxHashMap<PathBuf, (SystemTime, u64, String, Ontology)>,
    /// URL -> when fetching it failed, and why.
    failed: FxHashMap<String, (Instant, String)>,
}

/// A failed fetch is not tried again for this long.
const RETRY_FETCH_AFTER: Duration = Duration::from_secs(300);
/// A job waits this long for a newer one first, so editing an import IRI
/// neither rescans the workspace nor fetches every intermediate spelling.
const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone)]
struct Ontology {
    analysis: Arc<Analysis>,
    imports: Vec<String>,
}

fn worker(jobs: Receiver<Job>, out: Sender<Done>) {
    let mut memo = Memo::default();
    while let Ok(mut job) = jobs.recv() {
        // Only the latest job matters.
        // A superseded job's refetch still counts.
        let take = |job: &mut Job, mut j: Job| {
            j.refetch = j.refetch.or(job.refetch.take());
            *job = j;
        };
        loop {
            while let Ok(j) = jobs.try_recv() {
                take(&mut job, j);
            }
            // Every keystroke on an import line is a new job: wait for typing
            // to pause before walking the workspace for catalogs (or fetching).
            match jobs.recv_timeout(DEBOUNCE) {
                Ok(j) => take(&mut job, j),
                Err(_) => break,
            }
        }
        let superseded = || !jobs.is_empty();
        let Some(done) = resolve(&job, &mut memo, &superseded) else {
            continue;
        };
        if out.send(done).is_err() {
            break;
        }
    }
}

/// Resolve a job's imports transitively; `None` when a newer job arrived.
fn resolve(job: &Job, memo: &mut Memo, superseded: &dyn Fn() -> bool) -> Option<Done> {
    let req = &job.req;
    if let Some(iri) = &job.refetch {
        memo.failed.remove(without_fragment(iri));
    }
    let catalog = Catalog::scan(&req.roots);
    let local: FxHashSet<&str> = req.local.iter().map(String::as_str).collect();
    let mut queue: VecDeque<(String, u32)> = req.imports.iter().map(|i| (i.clone(), 0)).collect();
    let mut seen: FxHashSet<String> = FxHashSet::default();
    let mut status = Vec::new();
    let mut loaded = Vec::new();
    while let Some((iri, depth)) = queue.pop_front() {
        if !seen.insert(norm(&iri).to_string()) {
            continue;
        }
        if superseded() {
            return None;
        }
        let st = if local.contains(norm(&iri)) {
            Status::Local
        } else {
            match load(&iri, req, &catalog, memo) {
                Ok((location, onto, fresh)) => {
                    if depth < MAX_DEPTH {
                        queue.extend(onto.imports.iter().map(|i| (i.clone(), depth + 1)));
                    }
                    loaded.push(Loaded {
                        key: format!("{KEY_PREFIX}{iri}"),
                        facts: fresh.then(|| DocFacts::new(&onto.analysis)),
                        analysis: onto.analysis,
                    });
                    Status::Loaded(location)
                }
                Err(msg) => Status::Unresolved(msg),
            }
        };
        status.push((iri, st));
    }
    // `<x>` and `<x#>` are one import; each spelling gets its status.
    for i in &req.imports {
        if !status.iter().any(|(s, _)| s == i)
            && let Some(st) = status
                .iter()
                .find(|(s, _)| norm(s) == norm(i))
                .map(|(_, st)| st.clone())
        {
            status.push((i.clone(), st));
        }
    }
    Some(Done {
        generation: job.generation,
        status,
        loaded,
    })
}

/// Local imports are read only from inside the workspace: a cloned repository
/// must not make the editor read arbitrary files of the user's.
fn inside_workspace(path: &Path, roots: &[PathBuf]) -> Result<(), String> {
    let real = path.canonicalize().map_err(|e| e.to_string())?;
    if roots
        .iter()
        .filter_map(|r| r.canonicalize().ok())
        .any(|r| real.starts_with(r))
    {
        Ok(())
    } else {
        Err("it is outside the workspace, so it is not read".into())
    }
}

/// Load one import: (where from, the ontology, freshly parsed).
fn load(
    iri: &str,
    req: &Request,
    catalog: &Catalog,
    memo: &mut Memo,
) -> Result<(String, Ontology, bool), String> {
    let base = without_fragment(iri);
    let mut url = base.to_string();
    if let Some((target, cat)) = catalog.lookup(iri) {
        match target {
            Target::File(path) => {
                inside_workspace(path, &req.roots).map_err(|e| {
                    format!(
                        "Import <{iri}> is mapped to {} by {}: {e}",
                        path.display(),
                        cat.display()
                    )
                })?;
                return load_file(path, None, base, memo)
                    .map(|(o, fresh)| (path.display().to_string(), o, fresh))
                    .map_err(|e| {
                        format!(
                            "Import <{iri}> is mapped to {} by {}, which could not be loaded: {e}",
                            path.display(),
                            cat.display()
                        )
                    });
            }
            Target::Url(u) => url = u.clone(),
        }
    }
    if let Some(path) = url::Url::parse(&url)
        .ok()
        .filter(|u| u.scheme() == "file")
        .and_then(|u| crate::workspace::file_path_of(&u))
    {
        inside_workspace(&path, &req.roots)
            .map_err(|e| format!("Import <{iri}> names {}: {e}", path.display()))?;
        return load_file(&path, None, base, memo)
            .map(|(o, fresh)| (path.display().to_string(), o, fresh))
            .map_err(|e| {
                format!(
                    "Import <{iri}> could not be loaded from {}: {e}",
                    path.display()
                )
            });
    }
    let cached = req.cache.as_ref().map(|dir| cache_file(dir, &url));
    if let Some(path) = cached.as_ref().filter(|p| p.is_file())
        && let Ok((o, fresh)) = load_file(path, Some(&url), base, memo)
    {
        return Ok((url, o, fresh));
    }
    let web = url.starts_with("http://") || url.starts_with("https://");
    let allowed = req.network || req.fetch.iter().any(|f| norm(f) == norm(iri));
    if !allowed || !web {
        let how = if !web {
            "Map it to a local file in a catalog-v001.xml"
        } else {
            "Use the quick fix to fetch it once, map it to a local file in a catalog-v001.xml, or enable turtle.imports.network to fetch web imports automatically"
        };
        return Err(format!(
            "Import <{iri}> is not resolved: no ontology with this IRI in the workspace, no catalog-v001.xml entry and nothing in the import cache. {how}"
        ));
    }
    if let Some((at, msg)) = memo.failed.get(&url)
        && at.elapsed() < RETRY_FETCH_AFTER
    {
        return Err(msg.clone());
    }
    let fetched = fetch_turtle(iri, &url).and_then(|text| {
        let parsed = parse(text.as_bytes(), Format::Turtle, base).map_err(|e| {
            format!("Import <{iri}> was fetched from {url} but could not be read: {e}")
        })?;
        Ok((text, parsed))
    });
    let (text, mut parsed) = match fetched {
        Ok(v) => v,
        Err(msg) => {
            memo.failed.insert(url, (Instant::now(), msg.clone()));
            return Err(msg);
        }
    };
    let a = Arc::get_mut(&mut parsed.analysis).expect("fresh analysis");
    a.uri = url.clone();
    // A failed write only costs a fetch next time; locations then name the URL.
    if let Some(path) = &cached
        && write_atomic(path, text.as_bytes()).is_ok()
        && let Some(file) = path_to_uri(path)
    {
        a.uri = file;
        a.origin = Some(url.clone());
    }
    Ok((url, parsed, true))
}

/// Fetch `url` as Turtle text (RDF/XML is converted to N-Triples) whose
/// relative IRIs, if any, resolve against where the document was finally
/// found, after redirects. This text is what the cache holds.
fn fetch_turtle(iri: &str, url: &str) -> Result<String, String> {
    let (bytes, content_type, final_url) = fetch(url).map_err(|e| {
        format!(
            "Import <{iri}> could not be fetched from {url}: {e}. Map it to a local file in a catalog-v001.xml"
        )
    })?;
    let format = match content_type.as_deref().map(str::to_ascii_lowercase) {
        Some(ct) if ct.contains("xml") => Format::RdfXml,
        Some(ct) if ct.contains("turtle") || ct.contains("n-triples") => Format::Turtle,
        _ => sniff(&bytes),
    };
    Ok(match format {
        Format::Turtle => {
            let text = String::from_utf8_lossy(&bytes);
            let text = text.trim_start_matches('\u{feff}');
            match oxiri::Iri::parse(final_url.as_str()) {
                Ok(_) => format!("@base <{final_url}> .\n{text}"),
                Err(_) => text.to_string(),
            }
        }
        Format::RdfXml => rdf_xml_to_ntriples(&bytes, &final_url).map_err(|e| {
            format!("Import <{iri}> was fetched from {url} but could not be read: {e}")
        })?,
    })
}

/// Load an ontology file; its analysis is named by the file's URI. `origin` is
/// the URL a cached copy came from.
fn load_file(
    path: &Path,
    origin: Option<&str>,
    base: &str,
    memo: &mut Memo,
) -> Result<(Ontology, bool), String> {
    let meta = std::fs::metadata(path).map_err(|e| e.to_string())?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("larger than {} MB", MAX_FILE_BYTES / (1024 * 1024)));
    }
    let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    if let Some((m, len, b, o)) = memo.files.get(path)
        && *m == modified
        && *len == meta.len()
        && b == base
    {
        return Ok((o.clone(), false));
    }
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let format = match path.extension().and_then(|e| e.to_str()) {
        Some("ttl" | "turtle" | "nt") => Format::Turtle,
        _ => sniff(&bytes),
    };
    let mut o = parse(&bytes, format, base)?;
    // Locations open the file; hover names the URL a cached copy came from.
    let a = Arc::get_mut(&mut o.analysis).expect("fresh analysis");
    match path_to_uri(path) {
        Some(uri) => {
            a.uri = uri;
            a.origin = origin.map(str::to_string);
        }
        None => {
            if let Some(origin) = origin {
                a.uri = origin.to_string();
            }
        }
    }
    memo.files.insert(
        path.to_path_buf(),
        (modified, meta.len(), base.to_string(), o.clone()),
    );
    Ok((o, true))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Turtle,
    RdfXml,
}

/// RDF/XML or Turtle (N-Triples is Turtle), from the first bytes.
pub fn sniff(bytes: &[u8]) -> Format {
    let s = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    let s = s.trim_start_matches('\u{feff}').trim_start();
    if s.starts_with("<?xml") || s.starts_with("<!") {
        return Format::RdfXml;
    }
    let Some(rest) = s.strip_prefix('<') else {
        return Format::Turtle;
    };
    let name_len = rest
        .find(|c: char| !(c.is_alphanumeric() || "_-.:".contains(c)))
        .unwrap_or(rest.len());
    let after = &rest[name_len..];
    let xml = name_len > 0
        && (after.starts_with(char::is_whitespace)
            || after.starts_with("/>")
            || after
                .strip_prefix('>')
                .is_some_and(|t| t.trim_start().starts_with('<')));
    if xml { Format::RdfXml } else { Format::Turtle }
}

fn parse(bytes: &[u8], format: Format, base: &str) -> Result<Ontology, String> {
    let text = match format {
        Format::Turtle => {
            let text = String::from_utf8_lossy(bytes);
            let text = text.trim_start_matches('\u{feff}');
            // Relative IRIs resolve against the import IRI; on the first line,
            // so line numbers stay those of the file.
            match oxiri::Iri::parse(base) {
                Ok(_) => format!("@base <{base}> . {text}"),
                Err(_) => text.to_string(),
            }
        }
        Format::RdfXml => rdf_xml_to_ntriples(bytes, base)?,
    };
    let a = Analysis::new(
        base.to_string(),
        &text,
        Arc::new(PrefixMap::default()),
        false,
    );
    if a.parsed.triples.is_empty()
        && let Some(e) = a.parsed.errors.first()
    {
        return Err(e.message.clone());
    }
    let p = preds();
    let imports = a
        .parsed
        .triples
        .iter()
        .filter(|t| t.p == p.imports)
        .filter_map(|t| match &t.o {
            Node::Iri(o) => Some(o.to_string()),
            _ => None,
        })
        .collect();
    Ok(Ontology {
        analysis: Arc::new(a),
        imports,
    })
}

/// RDF/XML as N-Triples text. Stops at the first error; fails only when that
/// leaves no triple at all.
pub fn rdf_xml_to_ntriples(bytes: &[u8], base: &str) -> Result<String, String> {
    check_xml_tags(bytes)?;
    check_xml_entities(bytes)?;
    let parser = oxrdfxml::RdfXmlParser::new();
    let parser = parser.clone().with_base_iri(base).unwrap_or(parser);
    let mut out = String::new();
    let mut count = 0usize;
    for t in parser.for_slice(bytes) {
        match t {
            Ok(t) => {
                use std::fmt::Write;
                let _ = writeln!(out, "{t} .");
                count += 1;
            }
            Err(e) if count == 0 => return Err(e.to_string()),
            Err(_) => break,
        }
    }
    Ok(out)
}

/// Refuse XML with a start tag longer than [`MAX_XML_TAG_BYTES`].
fn check_xml_tags(bytes: &[u8]) -> Result<(), String> {
    let mut i = 0;
    while let Some(off) = bytes[i..].iter().position(|&b| b == b'<') {
        let start = i + off;
        let rest = &bytes[start..];
        let skip_to = |end: &[u8]| {
            rest.windows(end.len())
                .position(|w| w == end)
                .map_or(bytes.len(), |p| start + p + end.len())
        };
        if rest.starts_with(b"<!--") {
            i = skip_to(b"-->");
            continue;
        }
        if rest.starts_with(b"<![CDATA[") {
            i = skip_to(b"]]>");
            continue;
        }
        let mut quote = None;
        let mut end = bytes.len();
        for (j, &b) in rest.iter().enumerate().skip(1) {
            match (quote, b) {
                (None, b'"' | b'\'') => quote = Some(b),
                (Some(q), _) if q == b => quote = None,
                (None, b'>') => {
                    end = start + j + 1;
                    break;
                }
                _ => {}
            }
            if j > MAX_XML_TAG_BYTES {
                return Err(format!(
                    "an XML tag is longer than {} KiB",
                    MAX_XML_TAG_BYTES / 1024
                ));
            }
        }
        i = end;
    }
    Ok(())
}

/// Refuse DOCTYPE entities that could expand beyond the input: an entity whose
/// value refers to another entity (the "billion laughs" pattern), parameter or
/// external entities, and too many or too large declarations. Ontologies
/// exported by Protégé and friends only use flat namespace entities
/// (`<!ENTITY owl "http://www.w3.org/2002/07/owl#">`), which pass.
fn check_xml_entities(bytes: &[u8]) -> Result<(), String> {
    let Some(dt) = find(bytes, b"<!DOCTYPE") else {
        return Ok(());
    };
    let refuse = |why: &str| {
        Err(format!(
            "its DOCTYPE {why}; refused to avoid entity expansion"
        ))
    };
    let mut i = dt;
    let (mut count, mut total) = (0usize, 0usize);
    while let Some(off) = find(&bytes[i..], b"<!ENTITY") {
        let mut j = i + off + b"<!ENTITY".len();
        count += 1;
        if count > MAX_XML_ENTITIES {
            return refuse(&format!("declares more than {MAX_XML_ENTITIES} entities"));
        }
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        if bytes.get(j) == Some(&b'%') {
            return refuse("declares a parameter entity");
        }
        // Name, then either a quoted value or SYSTEM/PUBLIC (external).
        while bytes.get(j).is_some_and(|b| !b.is_ascii_whitespace()) {
            j += 1;
        }
        while bytes.get(j).is_some_and(u8::is_ascii_whitespace) {
            j += 1;
        }
        let Some(&q) = bytes.get(j).filter(|b| matches!(b, b'"' | b'\'')) else {
            return refuse("declares an external entity");
        };
        let value_start = j + 1;
        let Some(len) = bytes[value_start..].iter().position(|&b| b == q) else {
            return refuse("has an unterminated entity value");
        };
        let value = &bytes[value_start..value_start + len];
        if value.contains(&b'&') || value.contains(&b'%') {
            return refuse("declares an entity that refers to another entity");
        }
        total += len;
        if total > MAX_XML_ENTITY_BYTES {
            return refuse(&format!(
                "declares more than {} KiB of entity text",
                MAX_XML_ENTITY_BYTES / 1024
            ));
        }
        i = value_start + len + 1;
    }
    Ok(())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// GET `url`: (body, content type, final URL after redirects).
#[cfg(target_arch = "wasm32")]
fn fetch(_url: &str) -> Result<(Vec<u8>, Option<String>, String), String> {
    Err("imports are not fetched in the browser build".into())
}

/// GET `url`: (body, content type, final URL after redirects).
#[cfg(not(target_arch = "wasm32"))]
fn fetch(url: &str) -> Result<(Vec<u8>, Option<String>, String), String> {
    use ureq::ResponseExt;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(FETCH_TIMEOUT))
        .max_redirects(10)
        .user_agent(concat!("turtle-lsp/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut resp = agent
        .get(url)
        .header("Accept", ACCEPT)
        .call()
        .map_err(|e| e.to_string())?;
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let final_url = resp.get_uri().to_string();
    let body = resp
        .body_mut()
        .with_config()
        .limit(MAX_FETCH_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())?;
    Ok((body, content_type, final_url))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

// ---- cache ----

/// The per-user cache directory of turtle-lsp: `$TURTLE_LSP_CACHE_DIR`, else
/// `~/Library/Caches/turtle-lsp` (macOS), `%LOCALAPPDATA%\turtle-lsp`
/// (Windows), `$XDG_CACHE_HOME/turtle-lsp` or `~/.cache/turtle-lsp`.
pub fn cache_dir() -> Option<PathBuf> {
    let var = |k: &str| {
        std::env::var_os(k)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(d) = var("TURTLE_LSP_CACHE_DIR") {
        return Some(d);
    }
    let home = || var(if cfg!(windows) { "USERPROFILE" } else { "HOME" });
    let base = if cfg!(target_os = "macos") {
        home()?.join("Library").join("Caches")
    } else if cfg!(windows) {
        var("LOCALAPPDATA")?
    } else {
        var("XDG_CACHE_HOME").or_else(|| home().map(|h| h.join(".cache")))?
    };
    Some(base.join("turtle-lsp"))
}

/// The cache file for `url`: Turtle (RDF/XML is cached as N-Triples).
fn cache_file(dir: &Path, url: &str) -> PathBuf {
    dir.join(format!("{}.ttl", hash_name(url)))
}

/// 64-bit FNV-1a of `s`, as 16 hex digits.
pub fn hash_name(s: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

fn without_fragment(iri: &str) -> &str {
    iri.split_once('#').map_or(iri, |(b, _)| b)
}

/// The IRI used to match imports, ontology IRIs and catalog entries: an
/// ontology is often named `.../onto#` and imported as `.../onto`.
fn norm(iri: &str) -> &str {
    iri.trim_end_matches(['#', '/'])
}

// ---- catalogs ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    File(PathBuf),
    Url(String),
}

/// `<uri name=... uri=...>` entries of every catalog-v001.xml under the roots;
/// the first catalog (by path) that maps an IRI wins.
#[derive(Default)]
pub struct Catalog {
    map: FxHashMap<String, (Target, PathBuf)>,
}

impl Catalog {
    pub fn scan(roots: &[PathBuf]) -> Catalog {
        let mut files = Vec::new();
        let mut stack: Vec<PathBuf> = roots.to_vec();
        let mut seen = 0usize;
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for e in entries.flatten() {
                seen += 1;
                if seen > MAX_FILES * 4 {
                    stack.clear();
                    break;
                }
                let name = e.file_name();
                let name = name.to_string_lossy();
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_ref()) {
                        stack.push(e.path());
                    }
                } else if ft.is_file() && name == CATALOG_FILE {
                    files.push(e.path());
                }
            }
        }
        files.sort();
        let mut c = Catalog::default();
        for f in files {
            let Ok(text) = std::fs::read_to_string(&f) else {
                continue;
            };
            let dir = f.parent().unwrap_or(Path::new(""));
            for (name, target) in parse_catalog(&text, dir) {
                c.map
                    .entry(norm(&name).to_string())
                    .or_insert((target, f.clone()));
            }
        }
        c
    }

    fn lookup(&self, iri: &str) -> Option<(&Target, &PathBuf)> {
        self.map.get(norm(iri)).map(|(t, f)| (t, f))
    }
}

/// The `<uri name="..." uri="..."/>` entries of an XML catalog, with targets
/// resolved against `dir`. Comments are skipped; anything else is ignored.
pub fn parse_catalog(text: &str, dir: &Path) -> Vec<(String, Target)> {
    let mut out = Vec::new();
    let base = crate::workspace::file_url_of(dir, true);
    let mut rest = text;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix("!--") {
            rest = r.find("-->").map_or("", |j| &r[j + 3..]);
            continue;
        }
        let end = tag_end(rest);
        let tag = &rest[..end];
        rest = &rest[end..];
        let Some(attrs) = tag.strip_prefix("uri") else {
            continue;
        };
        if !attrs.starts_with(char::is_whitespace) {
            continue;
        }
        let attrs = attributes(attrs);
        let get = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let (Some(name), Some(uri)) = (get("name"), get("uri")) else {
            continue;
        };
        let target = match url::Url::parse(&uri) {
            Ok(u) if u.scheme() == "file" => crate::workspace::file_path_of(&u).map(Target::File),
            // A one-letter scheme is a Windows drive.
            Ok(u) if u.scheme().len() > 1 => Some(Target::Url(uri)),
            _ => base
                .as_ref()
                .and_then(|b| b.join(&uri).ok())
                .and_then(|u| crate::workspace::file_path_of(&u))
                .or_else(|| Some(dir.join(&uri)))
                .map(Target::File),
        };
        if let Some(t) = target {
            out.push((name, t));
        }
    }
    out
}

/// Length of a tag's content up to its closing `>` (quotes respected).
fn tag_end(s: &str) -> usize {
    let mut quote = None;
    for (i, c) in s.char_indices() {
        match (quote, c) {
            (None, '"' | '\'') => quote = Some(c),
            (Some(q), _) if q == c => quote = None,
            (None, '>') => return i,
            _ => {}
        }
    }
    s.len()
}

/// `name="value"` pairs of a tag, values unescaped.
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == '/');
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim().to_string();
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
            break;
        };
        let Some(close) = after[1..].find(q) else {
            break;
        };
        out.push((name, unescape(&after[1..1 + close])));
        rest = &after[close + 2..];
    }
    out
}

fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rdf_xml_entity_expansion_is_refused() {
        let mut laughs =
            String::from("<?xml version=\"1.0\"?>\n<!DOCTYPE rdf:RDF [\n<!ENTITY lol0 \"lol\">\n");
        for n in 1..9 {
            let refs: String = (0..10).map(|_| format!("&lol{};", n - 1)).collect();
            laughs.push_str(&format!("<!ENTITY lol{n} \"{refs}\">\n"));
        }
        laughs.push_str("]>\n<rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"http://e/a\"><rdf:value>&lol8;</rdf:value></rdf:Description></rdf:RDF>\n");
        assert!(check_xml_entities(laughs.as_bytes()).is_err());
        let flat = "<!DOCTYPE rdf:RDF [\n <!ENTITY owl \"http://www.w3.org/2002/07/owl#\" >\n <!ENTITY xsd 'http://www.w3.org/2001/XMLSchema#'>\n]>\n<rdf:RDF/>";
        assert!(check_xml_entities(flat.as_bytes()).is_ok());
        assert!(
            check_xml_entities(b"<!DOCTYPE x [<!ENTITY e SYSTEM \"file:///etc/passwd\">]>")
                .is_err()
        );
        assert!(check_xml_entities(b"<!DOCTYPE x [<!ENTITY % p \"x\">]>").is_err());
    }
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("turtle-imports-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn catalog_entries_resolve_relative_to_the_catalog() {
        let dir = Path::new(if cfg!(windows) { "C:\\p\\q" } else { "/p/q" });
        let xml = r#"<?xml version="1.0"?>
<catalog prefer="public" xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <!-- <uri name="http://ex.com/hidden" uri="no.ttl"/> -->
  <uri id="Imports Wizard Entry" name="http://ex.com/a" uri="vendor/a%20b.ttl"/>
  <uri name='http://ex.com/b?x=1&amp;y=2' uri="../b.owl" />
  <uri name="http://ex.com/c" uri="https://mirror.example.com/c.ttl"/>
  <uriSuffix uriSuffix="x" uri="y"/>
</catalog>"#;
        let got = parse_catalog(xml, dir);
        assert_eq!(got.len(), 3, "{got:?}");
        assert_eq!(got[0].0, "http://ex.com/a");
        assert_eq!(got[0].1, Target::File(dir.join("vendor").join("a b.ttl")));
        assert_eq!(got[1].0, "http://ex.com/b?x=1&y=2");
        assert_eq!(got[1].1, Target::File(dir.parent().unwrap().join("b.owl")));
        assert_eq!(
            got[2].1,
            Target::Url("https://mirror.example.com/c.ttl".into())
        );
        // Garbage never panics.
        for s in [
            "<uri",
            "<uri name=\"a",
            "<uri name='a' uri=",
            "&#xZZ;<uri name=\"&#x41;\" uri=\"x\">",
        ] {
            let _ = parse_catalog(s, dir);
        }
        assert_eq!(unescape("a&lt;b&#65;&#x42;&bogus;"), "a<bAB&bogus;");
    }

    #[test]
    fn formats_are_sniffed() {
        assert_eq!(sniff(b"<?xml version='1.0'?><rdf:RDF/>"), Format::RdfXml);
        assert_eq!(
            sniff(b"\xEF\xBB\xBF  <rdf:RDF xmlns:rdf='x'>"),
            Format::RdfXml
        );
        assert_eq!(sniff(b"<Ontology>\n  <Prefix/>"), Format::RdfXml);
        assert_eq!(
            sniff(b"<http://ex.com/a> <http://ex.com/p> \"x\" ."),
            Format::Turtle
        );
        assert_eq!(sniff(b"<urn:x:a> a <urn:x:C> ."), Format::Turtle);
        assert_eq!(sniff(b"@prefix ex: <http://ex.com/> ."), Format::Turtle);
        assert_eq!(sniff(b""), Format::Turtle);
    }

    #[test]
    fn rdf_xml_becomes_ntriples() {
        let xml = br##"<?xml version="1.0"?>
<rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"
         xmlns:rdfs="http://www.w3.org/2000/01/rdf-schema#"
         xmlns:owl="http://www.w3.org/2002/07/owl#">
  <owl:Class rdf:about="#Gear">
    <rdfs:label xml:lang="en">Gear "wheel"</rdfs:label>
  </owl:Class>
</rdf:RDF>"##;
        let nt = rdf_xml_to_ntriples(xml, "http://ex.com/rx").unwrap();
        let o = parse(nt.as_bytes(), Format::Turtle, "http://ex.com/rx").unwrap();
        assert!(o.analysis.parsed.errors.is_empty(), "{nt}");
        assert_eq!(
            o.analysis.label("http://ex.com/rx#Gear"),
            Some("Gear \"wheel\"")
        );
        assert!(rdf_xml_to_ntriples(b"<rdf:RDF", "http://ex.com/").is_err());
        let mut long = b"<?xml version='1.0'?><rdf:RDF ".to_vec();
        long.extend(std::iter::repeat_n(b' ', MAX_XML_TAG_BYTES + 10));
        long.extend(b"/>");
        assert!(
            rdf_xml_to_ntriples(&long, "http://ex.com/")
                .unwrap_err()
                .contains("KiB")
        );
    }

    #[test]
    fn hash_names_are_stable() {
        assert_eq!(hash_name(""), "cbf29ce484222325");
        assert_ne!(hash_name("http://a/"), hash_name("http://b/"));
        assert_eq!(hash_name("x").len(), 16);
    }

    fn job(imports: &[&str], local: &[&str], roots: Vec<PathBuf>, cache: Option<PathBuf>) -> Job {
        Job {
            generation: 1,
            req: Request {
                imports: imports.iter().map(|s| s.to_string()).collect(),
                local: local.iter().map(|s| s.to_string()).collect(),
                roots,
                cache,
                network: false,
                fetch: vec![],
            },
            refetch: None,
        }
    }

    #[test]
    fn local_imports_outside_the_workspace_are_not_read() {
        let root = tmp("outside");
        let ws = root.join("ws");
        fs::create_dir_all(&ws).unwrap();
        fs::write(
            root.join("private.ttl"),
            "<http://ex.com/p> a <http://ex.com/C> .\n",
        )
        .unwrap();
        fs::write(
            ws.join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/p" uri="../private.ttl"/></catalog>"#,
        )
        .unwrap();
        let file_iri = url::Url::from_file_path(root.join("private.ttl"))
            .unwrap()
            .to_string();
        let j = job(&["http://ex.com/p", &file_iri], &[], vec![ws.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert!(done.loaded.is_empty(), "{:?}", done.status);
        for (_, st) in &done.status {
            assert!(
                matches!(st, Status::Unresolved(m) if m.contains("outside the workspace")),
                "{st:?}"
            );
        }
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn imports_resolve_transitively_through_catalog_and_cache_with_cycles() {
        let root = tmp("transitive");
        let ws = root.join("ws");
        fs::create_dir_all(ws.join("sub")).unwrap();
        let cache = root.join("cache");
        fs::create_dir_all(&cache).unwrap();
        fs::write(
            ws.join("sub").join(CATALOG_FILE),
            r#"<catalog><uri name="http://ex.com/a" uri="../a.ttl"/></catalog>"#,
        )
        .unwrap();
        // a imports b (cache) and itself; b imports a (cycle) and c (missing) and the local one.
        fs::write(
            ws.join("a.ttl"),
            "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<> owl:imports <http://ex.com/b>, <http://ex.com/a> .\n<#T> a owl:Class .\n",
        )
        .unwrap();
        fs::write(
            cache.join(format!("{}.ttl", hash_name("http://ex.com/b"))),
            "<http://ex.com/b> <http://www.w3.org/2002/07/owl#imports> <http://ex.com/a#>, <http://ex.com/c>, <http://ex.com/local> .\n",
        )
        .unwrap();
        let j = job(
            &["http://ex.com/a"],
            &["http://ex.com/local"],
            vec![ws.clone()],
            Some(cache.clone()),
        );
        let mut memo = Memo::default();
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        let st: FxHashMap<_, _> = done.status.iter().cloned().collect();
        assert!(matches!(st["http://ex.com/a"], Status::Loaded(_)), "{st:?}");
        assert!(matches!(st["http://ex.com/b"], Status::Loaded(_)), "{st:?}");
        assert_eq!(st["http://ex.com/local"], Status::Local);
        assert!(
            matches!(&st["http://ex.com/c"], Status::Unresolved(m) if m.contains("turtle.imports.network")),
            "{st:?}"
        );
        // The cycle back to a#: visited once.
        assert_eq!(done.status.len(), 4, "{st:?}");
        assert_eq!(done.loaded.len(), 2);
        // a imports <http://ex.com/a#> (via b): the same import, same status.
        assert!(
            done.status
                .iter()
                .all(|(_, s)| !matches!(s, Status::Unresolved(m) if m.contains("http://ex.com/a")))
        );
        // Relative IRIs resolve against the import IRI.
        let a = &done
            .loaded
            .iter()
            .find(|l| l.key.ends_with("/a"))
            .unwrap()
            .analysis;
        assert!(a.defs.contains_key("http://ex.com/a#T"));
        assert!(a.uri.starts_with("file:"), "{}", a.uri);
        assert!(done.loaded.iter().all(|l| l.facts.is_some()));
        // Unchanged files are not parsed again.
        let again = resolve(&j, &mut memo, &|| false).unwrap();
        assert!(again.loaded.iter().all(|l| l.facts.is_none()));
        assert!(Arc::ptr_eq(
            &again.loaded[0].analysis,
            &done.loaded[0].analysis
        ));
        // A newer job supersedes this one.
        assert!(resolve(&j, &mut memo, &|| true).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn depth_is_limited() {
        let root = tmp("depth");
        let mut catalog = String::from("<catalog>");
        for i in 0..10 {
            fs::write(
                root.join(format!("o{i}.ttl")),
                format!(
                    "<http://ex.com/o{i}> <{OWL_IMPORTS}> <http://ex.com/o{}> .\n",
                    i + 1
                ),
            )
            .unwrap();
            catalog.push_str(&format!(
                "<uri name=\"http://ex.com/o{i}\" uri=\"o{i}.ttl\"/>"
            ));
        }
        catalog.push_str("</catalog>");
        fs::write(root.join(CATALOG_FILE), catalog).unwrap();
        let j = job(&["http://ex.com/o0"], &[], vec![root.clone()], None);
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        // o0 (direct) plus 5 levels below it.
        assert_eq!(done.loaded.len(), 1 + MAX_DEPTH as usize);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unresolved_imports_get_an_information_diagnostic() {
        let text = "@prefix owl: <http://www.w3.org/2002/07/owl#> .\n<http://ex.com/me> owl:imports <http://ex.com/x>, <http://ex.com/y> .\n";
        let a = Analysis::new(
            "file:///d.ttl".into(),
            text,
            Arc::new(PrefixMap::default()),
            false,
        );
        let missing = Status::Unresolved("nope".into());
        let ok = Status::Local;
        let d = diagnostics_with(&a, Encoding::Utf16, |i| {
            Some(if i.ends_with('x') { &missing } else { &ok })
        });
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::INFORMATION));
        assert_eq!(d[0].range.start.character, 31);
        assert_eq!(d[0].message, "nope");
    }

    /// A local HTTP server: `/ok*` answers Turtle, anything else 404. Returns
    /// its base URL and the log of requested paths.
    fn serve() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let log = Arc::new(std::sync::Mutex::new(Vec::new()));
        let seen = log.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]);
                let path = head.split(' ').nth(1).unwrap_or("").to_string();
                seen.lock().unwrap().push(path.clone());
                let resp = if path.starts_with("/ok") {
                    let body = "<#T> a <http://www.w3.org/2002/07/owl#Class> .\n";
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/turtle\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                } else {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .into()
                };
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        (base, log)
    }

    #[test]
    fn fetch_loads_only_the_listed_import_and_names_it_by_its_cache_file() {
        let root = tmp("fetch");
        let cache = root.join("cache");
        let (base, log) = serve();
        let (a, b) = (format!("{base}/ok-a"), format!("{base}/ok-b"));
        let mut j = job(&[&a, &b], &[], vec![root.clone()], Some(cache.clone()));
        // Listed as `<a#>`: the same import.
        j.req.fetch = vec![format!("{a}#")];
        let mut memo = Memo::default();
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        let st: FxHashMap<_, _> = done.status.iter().cloned().collect();
        assert_eq!(st[&a], Status::Loaded(a.clone()));
        assert!(
            matches!(&st[&b], Status::Unresolved(m) if m.contains("quick fix") && m.contains("turtle.imports.network")),
            "{st:?}"
        );
        assert_eq!(*log.lock().unwrap(), ["/ok-a"]);
        let check = |done: &Done| {
            let l = &done.loaded[0].analysis;
            let path = crate::workspace::uri_to_path(&l.uri).unwrap();
            assert_eq!(path, cache.join(format!("{}.ttl", hash_name(&a))));
            assert_eq!(l.origin.as_deref(), Some(a.as_str()));
            assert!(l.defs.contains_key(format!("{a}#T").as_str()));
        };
        check(&done);
        // Next session: a cache hit, named the same way, without a request.
        let j = job(&[&a], &[], vec![root.clone()], Some(cache.clone()));
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert_eq!(done.status, [(a.clone(), Status::Loaded(a.clone()))]);
        check(&done);
        assert_eq!(log.lock().unwrap().len(), 1);
        // No cache directory: named by the URL, as before.
        let mut j = job(&[&a], &[], vec![root.clone()], None);
        j.req.fetch = vec![a.clone()];
        let done = resolve(&j, &mut Memo::default(), &|| false).unwrap();
        assert_eq!(done.loaded[0].analysis.uri, a);
        assert_eq!(done.loaded[0].analysis.origin, None);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn refetch_tries_a_recently_failed_import_again() {
        let root = tmp("refetch");
        let (base, log) = serve();
        let c = format!("{base}/missing");
        let mut j = job(&[&c], &[], vec![root.clone()], None);
        j.req.fetch = vec![c.clone()];
        let mut memo = Memo::default();
        memo.failed
            .insert(c.clone(), (Instant::now(), "earlier".into()));
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        assert_eq!(
            done.status,
            [(c.clone(), Status::Unresolved("earlier".into()))]
        );
        assert!(log.lock().unwrap().is_empty());
        j.refetch = Some(c.clone());
        let done = resolve(&j, &mut memo, &|| false).unwrap();
        assert!(
            matches!(&done.status[0].1, Status::Unresolved(m) if m.contains("could not be fetched")),
            "{:?}",
            done.status
        );
        assert_eq!(*log.lock().unwrap(), ["/missing"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fetch_actions_are_offered_on_unresolved_web_imports() {
        let text = format!(
            "<http://ex.com/me> <{OWL_IMPORTS}> <http://ex.com/x>, <urn:x:y>, <https://ex.com/z> .\n"
        );
        let a = Analysis::new(
            "file:///d.ttl".into(),
            &text,
            Arc::new(PrefixMap::default()),
            false,
        );
        let missing = Status::Unresolved("nope".into());
        let ok = Status::Local;
        let diags = diagnostics_with(&a, Encoding::Utf16, |i| {
            Some(if i.ends_with('z') { &ok } else { &missing })
        });
        assert_eq!(diags.len(), 2);
        let mut params = CodeActionParams {
            text_document: lsp_types::TextDocumentIdentifier {
                uri: "file:///d.ttl".parse().unwrap(),
            },
            range: lsp_types::Range::default(),
            context: lsp_types::CodeActionContext {
                diagnostics: diags.clone(),
                ..Default::default()
            },
            work_done_progress_params: Default::default(),
            partial_result_params: Default::default(),
        };
        let got = fetch_actions(&a, Encoding::Utf16, &params);
        assert_eq!(got.len(), 1, "{got:?}");
        let CodeActionOrCommand::CodeAction(action) = &got[0] else {
            panic!("{got:?}")
        };
        assert_eq!(action.title, "Fetch http://ex.com/x and cache it");
        assert_eq!(action.kind, Some(CodeActionKind::QUICKFIX));
        assert_eq!(action.diagnostics.as_deref(), Some(&diags[..1]));
        assert!(action.edit.is_none());
        let cmd = action.command.as_ref().unwrap();
        assert_eq!(cmd.command, FETCH_COMMAND);
        assert_eq!(
            cmd.arguments,
            Some(vec![serde_json::json!("http://ex.com/x")])
        );
        // A diagnostic on no import gets nothing.
        let mut moved = diags[0].clone();
        moved.range.start.character += 1;
        params.context.diagnostics = vec![moved];
        assert!(fetch_actions(&a, Encoding::Utf16, &params).is_empty());
        params.context.diagnostics = diags;
        params.context.only = Some(vec![CodeActionKind::REFACTOR]);
        assert!(fetch_actions(&a, Encoding::Utf16, &params).is_empty());
        params.context.only = Some(vec![CodeActionKind::QUICKFIX]);
        assert_eq!(fetch_actions(&a, Encoding::Utf16, &params).len(), 1);
    }

    #[test]
    fn fetch_sends_a_job_that_allows_and_refetches_the_import() {
        let mut k = Knowledge::default();
        let text = format!("<http://ex.com/me> <{OWL_IMPORTS}> <http://ex.com/other> .\n");
        k.set(
            "file:///a.ttl",
            Source::Disk,
            Arc::new(Analysis::new(
                "file:///a.ttl".into(),
                &text,
                Arc::new(PrefixMap::default()),
                false,
            )),
        );
        let mut imports = Imports::new();
        imports.cache = None;
        imports.sync(&k, &[], false);
        let g = imports.generation;
        imports.fetch("http://ex.com/other");
        imports.fetch("http://ex.com/other");
        imports.sync(&k, &[], false);
        assert_eq!(imports.generation, g + 1);
        assert_eq!(
            imports.last.as_ref().unwrap().fetch,
            ["http://ex.com/other"]
        );
        assert!(imports.refetch.is_none());
        // Asking again sends another job, though the request is the same.
        imports.fetch("http://ex.com/other");
        imports.sync(&k, &[], false);
        assert_eq!(imports.generation, g + 2);
    }

    #[test]
    fn workspace_ontologies_and_imports_are_found() {
        let mut k = Knowledge::default();
        let none = Arc::new(PrefixMap::default());
        let text = format!(
            "<http://ex.com/me> a <{OWL_ONTOLOGY}> ; <{OWL_VERSION_IRI}> <http://ex.com/me/1.0> ; <{OWL_IMPORTS}> <http://ex.com/me#>, <http://ex.com/other> .\n"
        );
        k.set(
            "file:///a.ttl",
            Source::Disk,
            Arc::new(Analysis::new("file:///a.ttl".into(), &text, none, false)),
        );
        let mut imports = Imports::new();
        imports.cache = None;
        assert!(imports.sync(&k, &[], false).is_none());
        let req = imports.last.clone().unwrap();
        assert_eq!(req.imports, ["http://ex.com/me#", "http://ex.com/other"]);
        assert_eq!(req.local, ["http://ex.com/me", "http://ex.com/me/1.0"]);
        // Nothing changed: no new job.
        let g = imports.generation;
        imports.seen = None;
        assert!(imports.sync(&k, &[], false).is_none());
        assert_eq!(imports.generation, g);
        let done = imports
            .results()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(imports.apply(done, &mut k));
        assert_eq!(imports.status("http://ex.com/me#"), Some(&Status::Local));
        assert!(matches!(
            imports.status("http://ex.com/other"),
            Some(Status::Unresolved(_))
        ));
        // Without imports the answer is immediate.
        k.remove("file:///a.ttl");
        let done = imports.sync(&k, &[], false).unwrap();
        assert!(imports.apply(done, &mut k));
        assert!(imports.status("http://ex.com/other").is_none());
    }
}
