//! Workspace diagnostics (`diagnostics.workspace`): diagnostics for the
//! workspace files that are not open, computed in the background and pushed
//! with `publishDiagnostics`.
//!
//! A pass diagnoses the closed files on one thread of its own (never the
//! rayon pool the main loop parses with), one file at a time, with the
//! composer `turtle-lsp check` uses ([`diagnose::document`]), with the
//! server's latest SHACL results.
//! The thread keeps a copy of the knowledge index, brought up to date from
//! the server's at the start of each pass (unchanged sources cost nothing).
//!
//! Scheduling, all on the main loop:
//! - anything that can change other files' diagnostics calls
//!   [`State::invalidate_all`] (the server's `knowledge_changed`: the initial
//!   index, files changed on disk, prefixes, settings, SHACL and import
//!   results, described-set changes): every closed file is due again;
//! - an edit, open or close calls [`State::yield_to_edit`]: the running pass
//!   stops and its unfinished files go first in the next one;
//! - a pass starts [`DEBOUNCE`] after the last of these, so typing never
//!   competes with a pass.
//!
//! There is no per-file dependency tracking: a change that can affect other
//! files makes every closed file due, and a pass is cheap next to parsing.
//!
//! SHACL: closed files show the results of the server's own validation runs
//! (`shacl.enabled`, `shacl.maxTriples`, on save and with `shacl.onType`
//! after typing pauses, and when files change on disk). A pass never
//! validates; a run that changes the results starts a pass.
//!
//! Results come back in chunks on the main loop, which drops those for files
//! opened meanwhile and publishes only what changed. Opening a file hands it
//! to the live diagnostics; closing it publishes its diagnostics as a closed
//! file right away (from the file on disk), not an empty list.

use super::Server;
use crate::analysis::{Encoding, SparqlAnalysis};
use crate::clock::Instant;
use crate::diagnose::{self, DiagnosticInputs, Doc};
use crate::features::shacl_validate;
use crate::imports::Status;
use crate::knowledge::{Knowledge, Source};
use crate::settings::Settings;
use crate::workspace::{
    Workspace, is_rdf_file, is_sparql_path, path_to_uri, scan_roots_matching, uri_to_path,
};
use crossbeam_channel::{Receiver, Sender, never, unbounded};
use lsp_types::Diagnostic;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

/// Pause after the last edit or change before a pass starts.
pub const DEBOUNCE: Duration = Duration::from_millis(300);

/// Files per chunk of results sent back to the main loop.
const CHUNK: usize = 16;

/// The server's side: what is shown, what is due, the pass running.
#[derive(Default)]
pub struct State {
    enabled: bool,
    worker: Option<Worker>,
    results: Option<Receiver<Chunk>>,
    generation: u64,
    running: Option<Running>,
    todo: Option<Todo>,
    due: Option<Instant>,
    /// The non-empty diagnostics last published per closed file.
    shown: FxHashMap<String, Vec<Diagnostic>>,
}

/// Files due for a pass.
#[derive(Default)]
struct Todo {
    /// Every closed file.
    all: bool,
    /// These first, in order (a cancelled pass's unfinished files, files
    /// changed on disk).
    first: Vec<String>,
    /// The SPARQL files too.
    sparql: bool,
}

struct Running {
    generation: u64,
    /// The RDF files asked for, in order, and those whose result came back.
    targets: Vec<String>,
    received: FxHashSet<String>,
    sparql: bool,
    all: bool,
}

/// One pass, as sent to the thread.
struct Job {
    generation: u64,
    /// Every source of the server's knowledge index: (key, source, analysis).
    sources: Vec<(String, Source, Arc<crate::analysis::Analysis>)>,
    /// The closed RDF files to diagnose, in order.
    targets: Vec<String>,
    /// Also diagnose the SPARQL files under these roots that are not open.
    sparql: Option<SparqlScan>,
    settings: Settings,
    enc: Encoding,
    imports: FxHashMap<String, Status>,
    shacl: shacl_validate::ByUri,
}

struct SparqlScan {
    roots: Vec<PathBuf>,
    open: FxHashSet<String>,
}

/// Results of a pass, a few files at a time; the last chunk is `finished`.
pub struct Chunk {
    generation: u64,
    results: Vec<(String, Vec<Diagnostic>)>,
    finished: bool,
}

struct Worker {
    jobs: Sender<Job>,
    latest: Arc<AtomicU64>,
}

impl State {
    pub fn new(enabled: bool) -> State {
        State {
            enabled,
            ..Default::default()
        }
    }

    /// When the next pass is due, for the main loop's timer.
    pub fn due(&self) -> Option<Instant> {
        self.due
    }

    /// Results from the thread, for the main loop.
    pub fn results(&self) -> Receiver<Chunk> {
        self.results.clone().unwrap_or_else(never)
    }

    /// Every closed file is due again (after [`DEBOUNCE`]); the running
    /// pass, which reads the old state, stops.
    pub fn invalidate_all(&mut self) {
        if !self.enabled {
            return;
        }
        self.stop_running();
        let todo = self.todo.get_or_insert_with(Todo::default);
        todo.all = true;
        todo.sparql = true;
        self.due = Some(Instant::now() + DEBOUNCE);
    }

    /// An edit (or an open or close) arrived: stop the running pass and push
    /// the next one back. A pass that stops resumes with its unfinished files.
    pub fn yield_to_edit(&mut self) {
        if !self.enabled || (self.running.is_none() && self.todo.is_none()) {
            return;
        }
        self.stop_running();
        self.due = Some(Instant::now() + DEBOUNCE);
    }

    /// `uri` goes first in the next pass.
    fn prioritize(&mut self, uri: String) {
        let todo = self.todo.get_or_insert_with(Todo::default);
        todo.first.retain(|u| *u != uri);
        todo.first.insert(0, uri);
        self.due = Some(Instant::now() + DEBOUNCE);
    }

    /// Cancel the running pass, keeping its unfinished files for the next.
    fn stop_running(&mut self) {
        let Some(r) = self.running.take() else { return };
        self.generation += 1;
        if let Some(w) = &self.worker {
            w.latest.store(self.generation, Ordering::Relaxed);
        }
        let todo = self.todo.get_or_insert_with(Todo::default);
        let queued: FxHashSet<&String> = todo.first.iter().collect();
        let left: Vec<String> = r
            .targets
            .into_iter()
            .filter(|u| !r.received.contains(u) && !queued.contains(u))
            .collect();
        // Only the unfinished files: the others are up to date unless
        // something marks every file due again.
        todo.first.extend(left);
        todo.sparql |= r.sparql;
    }
}

impl Worker {
    fn start() -> (Worker, Receiver<Chunk>) {
        let (jobs, job_rx) = unbounded::<Job>();
        let (out, out_rx) = unbounded();
        let latest = Arc::new(AtomicU64::new(0));
        let seen = latest.clone();
        std::thread::Builder::new()
            .name("workspace-diagnostics".into())
            .stack_size(16 << 20)
            .spawn(move || {
                let mut k = Mirror::default();
                while let Ok(mut job) = job_rx.recv() {
                    // Only the newest queued job matters.
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    let cancelled = || seen.load(Ordering::Relaxed) != job.generation;
                    if cancelled() {
                        continue;
                    }
                    k.sync(std::mem::take(&mut job.sources));
                    if run(&job, &k.knowledge, &cancelled, &out).is_err() {
                        break;
                    }
                }
            })
            .expect("spawn the workspace diagnostics thread");
        (Worker { jobs, latest }, out_rx)
    }
}

/// The thread's copy of the knowledge index.
#[derive(Default)]
struct Mirror {
    knowledge: Knowledge,
    /// (external, key) of every source registered.
    keys: FxHashSet<(bool, String)>,
}

impl Mirror {
    /// Make the copy hold exactly `sources`. A source registered with the
    /// same analysis is left alone.
    fn sync(&mut self, sources: Vec<(String, Source, Arc<crate::analysis::Analysis>)>) {
        let mut keys = FxHashSet::default();
        for (key, source, a) in sources {
            if source == Source::External {
                self.knowledge.add_external(&key, a);
            } else {
                self.knowledge.set(&key, source, a);
            }
            keys.insert((source == Source::External, key));
        }
        for (external, key) in self.keys.difference(&keys) {
            if *external {
                self.knowledge.remove_external(key);
            } else {
                self.knowledge.remove(key);
            }
        }
        self.keys = keys;
    }
}

/// Diagnose the files of `job`, sending results a chunk at a time. Stops
/// early (sending nothing more) once `cancelled`; Err when the server is gone.
fn run(
    job: &Job,
    k: &Knowledge,
    cancelled: &dyn Fn() -> bool,
    out: &Sender<Chunk>,
) -> Result<(), ()> {
    let mut chunk = Vec::with_capacity(CHUNK);
    let send = |results: Vec<(String, Vec<Diagnostic>)>, finished: bool| {
        out.send(Chunk {
            generation: job.generation,
            results,
            finished,
        })
        .map_err(|_| ())
    };
    for uri in &job.targets {
        if cancelled() {
            return Ok(());
        }
        let d = guarded(|| {
            let a = k.get(uri)?;
            let status = |iri: &str| job.imports.get(iri);
            let inputs = DiagnosticInputs {
                import_status: &status,
                shacl: job.shacl.get(uri).map_or(&[], Vec::as_slice),
                ..DiagnosticInputs::new(&job.settings)
            };
            Some(diagnose::document(Doc::Rdf(a), k, job.enc, &inputs))
        });
        if let Some(d) = d {
            chunk.push((uri.clone(), d));
        }
        if chunk.len() >= CHUNK {
            send(std::mem::take(&mut chunk), false)?;
        }
        std::thread::yield_now();
    }
    if let Some(scan) = &job.sparql {
        // Their folders' prefixes, as the server reads them for a query.
        let mut prefixes = Workspace::new(scan.roots.clone(), job.settings.prefixes.clone());
        for path in scan_roots_matching(&scan.roots, is_sparql_path) {
            if cancelled() {
                return Ok(());
            }
            let Some(uri) = path_to_uri(&path).filter(|u| !scan.open.contains(u)) else {
                continue;
            };
            if let Some(d) = guarded(|| sparql_file(&mut prefixes, &uri, &path, k, job)) {
                chunk.push((uri, d));
            }
            if chunk.len() >= CHUNK {
                send(std::mem::take(&mut chunk), false)?;
            }
            std::thread::yield_now();
        }
    }
    if cancelled() {
        return Ok(());
    }
    send(chunk, true)
}

/// The diagnostics of the SPARQL file at `path`, read from disk.
fn sparql_file(
    prefixes: &mut Workspace,
    uri: &str,
    path: &Path,
    k: &Knowledge,
    job: &Job,
) -> Option<Vec<Diagnostic>> {
    let text = std::fs::read_to_string(path).ok()?;
    let external = prefixes.external_prefixes_for(uri);
    let q = SparqlAnalysis::new(uri.to_string(), &text, external);
    let inputs = DiagnosticInputs::new(&job.settings);
    Some(diagnose::document(Doc::Sparql(&q), k, job.enc, &inputs))
}

/// `f`, with a panic turned into an internal-error diagnostic.
fn guarded(f: impl FnOnce() -> Option<Vec<Diagnostic>>) -> Option<Vec<Diagnostic>> {
    super::guard("workspace diagnostics", f)
        .unwrap_or_else(|msg| Some(vec![super::internal_error_diagnostic(&msg)]))
}

/// Whether `uri` names a file under one of `roots`.
fn under(roots: &[PathBuf], uri: &str) -> bool {
    uri_to_path(uri).is_some_and(|p| roots.iter().any(|r| p.starts_with(r)))
}

impl Server {
    /// `diagnostics.workspace` may have changed: start or stop.
    pub(super) fn wsdiag_setting(&mut self) {
        let on = self.layers.settings().diagnostics_workspace && !self.inline;
        if on == self.wsdiag.enabled {
            return;
        }
        if on {
            self.wsdiag = State::new(true);
            self.wsdiag.invalidate_all();
            return;
        }
        // Off: clear every closed file shown; the thread ends with its channel.
        let old = std::mem::take(&mut self.wsdiag);
        if let Some(w) = &old.worker {
            w.latest.store(old.generation + 1, Ordering::Relaxed);
        }
        for uri in old.shown.into_keys() {
            if !self.ws.is_open(&uri) {
                self.send_diagnostics(&uri, Vec::new(), None);
            }
        }
    }

    /// A document was opened: it shows live diagnostics from now on.
    pub(super) fn wsdiag_opened(&mut self, uri: &str) {
        if !self.wsdiag.enabled {
            return;
        }
        self.wsdiag.yield_to_edit();
        // Pushed live diagnostics replace these; pulled ones do not.
        if self.wsdiag.shown.remove(uri).is_some() && self.refresh.pull() {
            self.send_diagnostics(uri, Vec::new(), None);
        }
    }

    /// A document was closed: show its diagnostics as a closed file, as a
    /// pass would. False when it is not a workspace file (or the setting is
    /// off), and the caller clears them as usual.
    pub(super) fn wsdiag_closed(&mut self, uri: &str) -> bool {
        if !self.wsdiag.enabled || !self.index.merged() || !under(self.ws.roots(), uri) {
            return false;
        }
        self.wsdiag.yield_to_edit();
        let Some(path) = uri_to_path(uri) else {
            return false;
        };
        let d = if is_rdf_file(&path) {
            match self.diagnostics_for(uri) {
                Ok(d) => d,
                Err(msg) => Some(vec![super::internal_error_diagnostic(&msg)]),
            }
        } else if is_sparql_path(&path) {
            let text = std::fs::read_to_string(&path).ok();
            text.map(|text| {
                let external = self.ws.external_prefixes_for(uri);
                let q = SparqlAnalysis::new(uri.to_string(), &text, external);
                let inputs = DiagnosticInputs::new(self.layers.settings());
                diagnose::document(Doc::Sparql(&q), self.ws.knowledge(), self.enc, &inputs)
            })
        } else {
            None
        };
        let Some(d) = d else { return false };
        self.wsdiag.shown.remove(uri);
        if !d.is_empty() {
            self.wsdiag.shown.insert(uri.to_string(), d.clone());
        }
        self.send_diagnostics(uri, d, None);
        true
    }

    /// The RDF files at `paths` changed on disk (they are not open): each
    /// goes first in the next pass (the last one first), or is cleared if it
    /// is gone.
    pub(super) fn wsdiag_disk_changed(&mut self, paths: &[PathBuf]) {
        if !self.wsdiag.enabled {
            return;
        }
        for path in paths {
            let Some(uri) = path_to_uri(path) else {
                continue;
            };
            if path.is_file() {
                self.wsdiag.prioritize(uri);
            } else if self.wsdiag.shown.remove(&uri).is_some() {
                self.send_diagnostics(&uri, Vec::new(), None);
            }
        }
    }

    /// The debounce passed: start the pass that is due.
    pub(super) fn wsdiag_start(&mut self) {
        self.wsdiag.due = None;
        if !self.wsdiag.enabled || !self.index.merged() {
            // The initial index calls `knowledge_changed` when it lands.
            return;
        }
        let Some(todo) = self.wsdiag.todo.take() else {
            return;
        };
        let roots = self.ws.roots().to_vec();
        let k = self.ws.knowledge();
        let sources = k
            .sources()
            .map(|(key, s, a)| (key.to_string(), s, a.clone()))
            .collect();
        let ws = &self.ws;
        let closed = |u: &str| !ws.is_open(u) && ws.is_indexed(u) && under(&roots, u);
        let mut seen = FxHashSet::default();
        let mut targets: Vec<String> = todo
            .first
            .into_iter()
            .filter(|u| closed(u) && seen.insert(u.clone()))
            .collect();
        if todo.all {
            let mut rest: Vec<String> = ws
                .indexed_uris()
                .filter(|u| closed(u) && !seen.contains(*u))
                .map(str::to_owned)
                .collect();
            rest.sort();
            targets.extend(rest);
        }
        let sparql = todo.sparql.then(|| SparqlScan {
            roots: roots.clone(),
            open: ws.open_uris().map(str::to_owned).collect(),
        });
        if targets.is_empty() && sparql.is_none() {
            return;
        }
        let shacl = self
            .shacl
            .diagnostics()
            .iter()
            .filter(|(u, _)| !ws.is_open(u))
            .map(|(u, d)| (u.clone(), d.clone()))
            .collect();
        if self.wsdiag.worker.is_none() {
            let (w, rx) = Worker::start();
            self.wsdiag.worker = Some(w);
            self.wsdiag.results = Some(rx);
        }
        self.wsdiag.generation += 1;
        let generation = self.wsdiag.generation;
        self.wsdiag.running = Some(Running {
            generation,
            targets: targets.clone(),
            received: FxHashSet::default(),
            sparql: sparql.is_some(),
            all: todo.all,
        });
        let job = Job {
            generation,
            sources,
            targets,
            sparql,
            settings: self.layers.settings().clone(),
            enc: self.enc,
            imports: self.imports.statuses().clone(),
            shacl,
        };
        if let Some(w) = &self.wsdiag.worker {
            w.latest.store(generation, Ordering::Relaxed);
            let _ = w.jobs.send(job);
        }
    }

    /// Results of a pass arrived: publish those that changed.
    pub(super) fn wsdiag_chunk(&mut self, chunk: Option<Chunk>) {
        let Some(chunk) = chunk else {
            // The thread is gone (a bug): stop listening to it.
            self.wsdiag.worker = None;
            self.wsdiag.results = None;
            self.wsdiag.running = None;
            return;
        };
        let current = self
            .wsdiag
            .running
            .as_ref()
            .is_some_and(|r| r.generation == chunk.generation);
        if !current || !self.wsdiag.enabled {
            return;
        }
        let mut seen = Vec::with_capacity(chunk.results.len());
        for (uri, d) in chunk.results {
            seen.push(uri.clone());
            if self.ws.is_open(&uri) {
                continue;
            }
            let before = self.wsdiag.shown.get(&uri);
            if before.map_or(d.is_empty(), |b| *b == d) {
                continue;
            }
            if d.is_empty() {
                self.wsdiag.shown.remove(&uri);
            } else {
                self.wsdiag.shown.insert(uri.clone(), d.clone());
            }
            self.send_diagnostics(&uri, d, None);
        }
        let Some(r) = self.wsdiag.running.as_mut() else {
            return;
        };
        r.received.extend(seen);
        if !chunk.finished {
            return;
        }
        let r = self.wsdiag.running.take().expect("checked above");
        // A full pass that did not see a file shown before: it is gone.
        if r.all && r.sparql {
            let gone: Vec<String> = self
                .wsdiag
                .shown
                .keys()
                .filter(|u| !r.received.contains(*u) && !self.ws.is_open(u))
                .cloned()
                .collect();
            for uri in gone {
                self.wsdiag.shown.remove(&uri);
                self.send_diagnostics(&uri, Vec::new(), None);
            }
        }
    }
}
