//! `owl:imports` resolution.
//!
//! Every `owl:imports <iri>` in the workspace is resolved, in this order:
//! 1. an ontology the workspace describes (`<iri> a owl:Ontology`, or some
//!    ontology's `owl:versionIRI <iri>`): nothing to load;
//! 2. a `catalog-v001.xml` (Protégé convention) anywhere under the workspace
//!    roots, mapping the IRI to a file (relative to the catalog) or a URL;
//! 3. the per-user import cache;
//! 4. the network, when `turtle.imports.network` is on or the user asked to
//!    fetch this import ([`Imports::fetch`]); then cached. Web fetches obey
//!    the rules of [`crate::fetch`]: public addresses only, redirects checked
//!    hop by hop, and a budget of requests and bytes for each pass; what a
//!    pass cannot fetch is reported as unresolved, with the reason.
//!
//! Files and mappings are read only through [`fs_policy`]: inside the
//! workspace roots, on this machine.
//!
//! Loaded ontologies are resolved transitively (up to [`MAX_DEPTH`] levels
//! below a workspace import, cycles stop) and join the knowledge index as
//! read-only external sources: hover, completion and lints know their terms,
//! but they are never diagnosed. Each is read in its own syntax (Turtle, TriG,
//! N-Triples, N-Quads, JSON-LD) with our parser, chosen by the file's
//! extension, else by what it starts with, never by the import IRI; RDF/XML is
//! converted to N-Triples text first, so every source is an [`Analysis`] like
//! any other.
//!
//! All file and network work happens on one background thread, started on the
//! first import and blocked on its job channel when there is nothing to do.
//! Each job carries a generation; a newer job supersedes an unfinished one and
//! results of older jobs are dropped. The main loop selects on
//! [`Imports::results`].

use crate::analysis::{Analysis, Encoding};
use crate::features::diagnostics::SOURCE;
use crate::knowledge::{DocFacts, Knowledge, Source};
use crate::server::worker::{Fed, Feed};
use crate::syntax::{self, Iri, Node, PrefixMap};
use crate::workspace::{MAX_FILE_BYTES, MAX_FILES, SKIP_DIRS, path_to_uri};
use crossbeam_channel::{Receiver, RecvError, Sender, unbounded};
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
    /// Fetch from loopback, private and other internal addresses too (see
    /// `fetch::ALLOW_PRIVATE_ENV`).
    allow_private: bool,
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

/// The workspace's imports resolved once, on the calling thread (the command
/// line): their status, and the ontologies loaded, keyed as the server keys
/// them in the knowledge index.
#[derive(Clone, Default)]
pub struct Resolved {
    pub status: FxHashMap<String, Status>,
    pub loaded: Vec<(String, Arc<Analysis>)>,
}

impl Resolved {
    /// The imports that are not loaded, sorted.
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
}

/// Resolve the imports of the workspace documents in `k` as the server's
/// background thread does (workspace, catalogs under `roots`, the import
/// cache, then the network if `network`), but now and on this thread.
pub fn resolve_now(k: &Knowledge, roots: &[PathBuf], network: bool) -> Resolved {
    let (mut imports, mut local) = (BTreeSet::new(), BTreeSet::new());
    for (_, _, a) in k.docs().filter(|(_, src, _)| *src != Source::External) {
        let d = doc_onto(a);
        imports.extend(d.imports.iter().map(|i| i.to_string()));
        local.extend(d.ontologies.iter().map(|i| norm(i).to_string()));
    }
    if imports.is_empty() {
        return Resolved::default();
    }
    let job = Job {
        generation: 0,
        req: Request {
            imports: imports.into_iter().collect(),
            local: local.into_iter().collect(),
            roots: roots.to_vec(),
            cache: import_cache_dir(),
            network,
            fetch: Vec::new(),
            allow_private: allow_private_hosts(),
        },
        refetch: None,
    };
    let Some(done) = resolve(&job, &mut Memo::default(), &|| false) else {
        return Resolved::default();
    };
    Resolved {
        status: done.status.into_iter().collect(),
        loaded: done
            .loaded
            .into_iter()
            .map(|l| (l.key, l.analysis))
            .collect(),
    }
}

/// `owl:imports` state of the server: what was asked, what came back.
pub struct Imports {
    /// Resolve without loading or threads (see [`Imports::new_inline`]).
    inline: bool,
    jobs: Option<Sender<Job>>,
    /// The answers of the worker thread. The worker holds the only sender,
    /// so the channel disconnects when it ends.
    results: Feed<Done>,
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
    /// See [`Request::allow_private`].
    allow_private: bool,
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
        Imports {
            inline: false,
            jobs: None,
            results: Feed::default(),
            generation: 0,
            answered: 0,
            last: None,
            seen: None,
            status: FxHashMap::default(),
            registered: FxHashSet::default(),
            per_doc: FxHashMap::default(),
            cache: import_cache_dir(),
            allow_private: allow_private_hosts(),
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

    /// Answers from the background thread, for the main loop to select on
    /// (a channel that never fires while there is no thread).
    pub fn results(&self) -> Receiver<Done> {
        self.results.rx()
    }

    /// What a receive on [`Imports::results`] returned: the answer to
    /// [`Imports::apply`], or, when the thread is gone, the answer that ends
    /// the pass it died in (if one was running): every import that pass was
    /// to resolve is unresolved, with the reason. The next change to the
    /// imports starts a new thread.
    pub fn received(&mut self, got: Result<Done, RecvError>) -> Option<Done> {
        match self.results.take(got) {
            Fed::Got(done) => Some(done),
            Fed::Gone => {
                self.jobs = None;
                let req = self.last.as_ref().filter(|_| self.busy())?;
                Some(failed(self.generation, &req.imports, CRASHED))
            }
        }
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
            allow_private: self.allow_private,
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
        if self.jobs.is_none() {
            let (tx, rx) = unbounded();
            let (out, results) = unbounded();
            let started = crate::server::worker::spawn_guarded(
                "turtle-lsp-imports",
                16 << 20,
                move || worker(rx, out),
                |_| {},
            );
            if started.is_err() {
                // No thread: the pass ends at once, unresolved.
                return Some(failed(self.generation, &job.req.imports, NO_THREAD));
            }
            self.jobs = Some(tx);
            self.results = Feed::open(results);
        }
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
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

    /// The status of every import asked for so far, by IRI.
    pub fn statuses(&self) -> &FxHashMap<String, Status> {
        &self.status
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

/// The import cache: [`cache_dir`]`/imports`.
pub fn import_cache_dir() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("imports"))
}

fn without_fragment(iri: &str) -> &str {
    iri.split_once('#').map_or(iri, |(b, _)| b)
}

/// The IRI used to match imports, ontology IRIs and catalog entries: an
/// ontology is often named `.../onto#` and imported as `.../onto`.
fn norm(iri: &str) -> &str {
    iri.trim_end_matches(['#', '/'])
}

mod catalog;
mod load;
mod rdfxml;
mod worker;

use worker::{CRASHED, Memo, NO_THREAD, failed, resolve, worker};

#[cfg(not(target_arch = "wasm32"))]
pub use crate::fetch::{FETCH_TIMEOUT, MAX_FETCH_BYTES, hash_name};
pub use catalog::{Catalog, Target, parse_catalog};
pub use load::{Format, format_of, fs_policy, fs_policy_path, is_network_path, sniff};
pub use rdfxml::rdf_xml_to_ntriples;

/// Whether web fetches may reach internal addresses: set by the environment
/// (`fetch::ALLOW_PRIVATE_ENV`), never by a file of the workspace.
#[cfg(not(target_arch = "wasm32"))]
fn allow_private_hosts() -> bool {
    crate::fetch::Policy::from_env().allow_private
}

#[cfg(target_arch = "wasm32")]
fn allow_private_hosts() -> bool {
    false
}

#[cfg(test)]
mod testing {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    pub fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("turtle-imports-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A local HTTP server: `/ok*` answers Turtle, anything else 404. Returns
    /// its base URL and the log of requested paths.
    pub fn serve() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        serve_with(|path| {
            if path.starts_with("/ok") {
                document(
                    "text/turtle",
                    "<#T> a <http://www.w3.org/2002/07/owl#Class> .\n",
                )
            } else {
                NOT_FOUND.into()
            }
        })
    }

    pub const NOT_FOUND: &str =
        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";

    /// A 200 response with `body` of `content_type`.
    pub fn document(content_type: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    /// A local HTTP server; `route` turns a request path into the whole
    /// response. Returns its base URL and the log of requested paths.
    pub fn serve_with(
        route: impl Fn(&str) -> String + Send + 'static,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
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
                let _ = stream.write_all(route(&path).as_bytes());
            }
        });
        (base, log)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Knowledge with one document importing each of `iris`.
    fn importing(iris: &[&str]) -> Knowledge {
        let mut k = Knowledge::default();
        let text: String = iris
            .iter()
            .map(|i| format!("<http://ex.com/me> <{OWL_IMPORTS}> <{i}> .\n"))
            .collect();
        k.set(
            "file:///a.ttl",
            Source::Disk,
            Arc::new(Analysis::new(
                "file:///a.ttl".into(),
                &text,
                Arc::default(),
                false,
            )),
        );
        k
    }

    fn unresolved_with(imports: &Imports, iri: &str, what: &str) -> bool {
        matches!(imports.status(iri), Some(Status::Unresolved(why)) if why.contains(what))
    }

    #[test]
    fn a_panic_resolving_imports_ends_that_pass_unresolved_and_the_worker_goes_on() {
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let mut k = importing(&["http://ex.com/turtle-lsp-test-panic"]);
        let mut imports = Imports::new();
        imports.cache = None;
        assert!(imports.sync(&k, &[], false).is_none());
        assert!(imports.busy());
        let done = imports
            .results()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        std::panic::set_hook(hook);
        assert!(imports.apply(done, &mut k));
        // Not busy: the progress shown for it can end.
        assert!(!imports.busy());
        assert!(unresolved_with(
            &imports,
            "http://ex.com/turtle-lsp-test-panic",
            "internal error"
        ));
        // The same worker answers the next pass.
        let k = importing(&["http://ex.com/other"]);
        imports.retry();
        assert!(imports.sync(&k, &[], false).is_none());
        let done = imports
            .results()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(done.generation, imports.generation);
        assert!(
            done.status.iter().all(
                |(_, s)| !matches!(s, Status::Unresolved(why) if why.contains("internal error"))
            )
        );
    }

    #[test]
    fn a_dead_worker_ends_the_pass_it_died_in_and_the_next_change_starts_another() {
        let mut k = importing(&["http://ex.com/lost"]);
        let mut imports = Imports::new();
        imports.cache = None;
        // Nothing runs yet: a disconnected channel answers nothing.
        assert!(imports.received(Err(RecvError)).is_none());
        assert!(imports.sync(&k, &[], false).is_none());
        assert!(imports.busy());
        // The worker is gone before it answered (here: we stop listening).
        let done = imports.received(Err(RecvError)).expect("the pass ends");
        assert!(imports.apply(done, &mut k));
        assert!(!imports.busy(), "progress would never end");
        assert!(unresolved_with(&imports, "http://ex.com/lost", "stopped"));
        // Its channel is closed: the main loop is not woken again.
        assert!(imports.results().try_recv().is_err());
        assert!(matches!(
            imports.results().recv_timeout(Duration::from_millis(20)),
            Err(crossbeam_channel::RecvTimeoutError::Timeout)
        ));
        // A worker that ends while idle changes nothing.
        assert!(imports.received(Err(RecvError)).is_none());
        // A change of the imports starts a new worker, which answers.
        let k = importing(&["http://ex.com/lost", "http://ex.com/new"]);
        imports.retry();
        assert!(imports.sync(&k, &[], false).is_none());
        let done = imports
            .results()
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(done.generation, imports.generation);
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
