//! "▶ Run query" (`turtle.runQuery`): run a SPARQL document's query over the
//! workspace and show the results.
//!
//! The command takes the document's current text (the open buffer, unsaved
//! edits included, else the file on disk) and a snapshot of the workspace's
//! RDF documents: every source of the knowledge index but the imported ones
//! (`owl:imports` targets are not workspace files), open documents with their
//! current text. That is the data SHACL validation reads, without the
//! imports. The query sees the prefixes the editor gives the document
//! (prefixes.ttl and the `prefixes` setting).
//!
//! With `query.reasoning` on (`default` or `complete`), the query also reads
//! the workspace's imports (the knowledge index's external sources, as
//! background reasoning reads them) and runs over every triple TurbOWL
//! infers from the snapshot and the imports with that rule set, up to
//! `turtle.reasoning.maxTriples`. The results document then says so in one
//! line under its heading ([`Inferred::sentence`], as the command line
//! writes it): "Includes 12 inferred triples (TurbOWL, default rules)."
//! with "TurbOWL reported 2 findings." when there are any, or "Inferred
//! triples skipped: the workspace has N triples, over the reasoning limit
//! of M.". With `off` no import is read, nothing is built and the document
//! has no such line. An optional second argument, `{"reasoning": "off" |
//! "default" | "complete"}`, overrides `query.reasoning` for that run (a
//! `null` is no argument); any other value is `InvalidParams`.
//!
//! A client that declared `experimental: {"turtle/queryResults": true}` in
//! `initialize` shows the results itself (the VS Code panel): no file is
//! written and no `showDocument` sent, and the request is answered with the
//! result object `{"uri", "name", "files", "elapsedMs", "summary",
//! "reasoning", "prefixes", "answer", "inferred"?}`: `files` the number of
//! documents read, `summary` as below, `reasoning` the rules of the run,
//! `prefixes` the ones the results document shortens IRIs with
//! ([`query::display_prefixes`]), `answer` and `inferred` as `turtle/query`
//! answers them (its helpers, [`super::query::answer_json`] and
//! [`super::query::inferred_json`]), or `answer` `{"kind": "error",
//! "message", "line"?, "column"?}` for a syntax error (with its 1-based
//! position), an UPDATE, a limit or a failure. A cancelled or replaced run
//! answers `{"cancelled": true, "uri"}`, with one exception: a
//! `$/cancelRequest` for the run's request answers the standard LSP error
//! `RequestCanceled`, since the client gave the request up. Progress, the
//! Cancel lens and `$/cancelRequest` are as below.
//!
//! The query runs on a thread of its own (never the main loop), with
//! `query.timeoutSeconds` and `query.maxRows`. For any other client the
//! thread renders the results as Markdown and writes them to
//! `<cache dir>/query-results/<file stem>.md` (overwritten each run; a
//! second document of the same name gets `<file stem>-2.md`), then
//! hands a [`Done`] to the main loop, which opens the file with
//! `window/showDocument` (`takeFocus: false`) when the client supports it,
//! else shows the summary with `window/showMessage`, and answers the
//! `executeCommand` request with the summary ("12 rows in 34 ms"). The
//! request stays pending while the query runs; other requests are answered
//! meanwhile.
//!
//! A second run of the same document cancels the first, whose request is
//! answered at once with "cancelled"; a `$/cancelRequest` for the pending
//! request cancels the run and answers `RequestCanceled`. Embedded (no
//! threads), the query runs inline and the summary is shown as a message.
//!
//! While a run goes on, a client that can show progress shows a cancellable
//! "Running <file>" with the seconds so far; cancelling it
//! (`window/workDoneProgress/cancel`) stops the run. A client that can be
//! asked for code lenses again sees the document's "▶ Run query" lens as
//! "■ Cancel query", which runs `turtle.cancelQuery`. Either way the run's
//! request is answered "cancelled".

use super::query::{Rules, answer_json, inferred_json};
use crate::analysis::Analysis;
use crate::clock::Instant;
use crate::query::{self, Answer, Inference, Inferred, Limits, QueryError};
use crate::server::client::Client;
use crate::server::json;
use crate::server::progress::Progress;
use crate::server::worker::Spawn;
use crate::settings::{Reasoning, Settings};
use crate::syntax::PrefixMap;
use crate::workspace::{Workspace, is_sparql_path, normalize_uri, path_to_uri, uri_to_path};
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_server::{ErrorCode, RequestId, Response};
use lsp_types::request::{CodeLensRefresh, Request as _};
use lsp_types::{CodeLens, ExecuteCommandParams, MessageType, ShowDocumentParams, Uri};
use rustc_hash::FxHashMap;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Run the SPARQL query of a document over the workspace (the "▶ Run query"
/// lens), with the document's URI.
pub(crate) const RUN_QUERY: &str = "turtle.runQuery";

/// Stop the run of a document's query (the "■ Cancel query" lens), with the
/// document's URI.
pub(crate) const CANCEL_QUERY: &str = "turtle.cancelQuery";

/// The `experimental` client capability of a client that shows the results
/// itself: [`Delivery::Object`].
pub(crate) const RESULTS_CAPABILITY: &str = "turtle/queryResults";

/// How the client is given a run's results, from its capabilities.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Delivery {
    /// It declared [`RESULTS_CAPABILITY`]: the answer is the result object.
    Object,
    /// It can show a document: the results document is opened.
    Document,
    /// Neither: the results document is written, the summary is a message.
    Message,
}

/// What `turtle.runQuery` takes after the document URI.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Options {
    /// Overrides `query.reasoning` for this run.
    reasoning: Option<Rules>,
}

/// The commands [`State::command`] runs.
pub(crate) const COMMANDS: [&str; 2] = [RUN_QUERY, CANCEL_QUERY];

/// The title of the lens that runs a query.
pub(crate) const RUN_QUERY_TITLE: &str = "▶ Run query";

/// The title the lens has while the query runs.
pub(crate) const CANCEL_QUERY_TITLE: &str = "■ Cancel query";

/// The runs going on, by document URI, and the channel their results come
/// back on.
pub(crate) struct State {
    delivery: Delivery,
    /// The client can be asked for the code lenses again.
    lens_refresh: bool,
    spawn: Spawn,
    // Runs on a thread of their own hand their results back on this
    // channel; the browser build runs every query inline.
    #[cfg(not(target_arch = "wasm32"))]
    done_tx: Sender<Done>,
    #[cfg(not(target_arch = "wasm32"))]
    done_rx: Receiver<Done>,
    #[cfg(not(target_arch = "wasm32"))]
    next_id: u64,
    /// When each run's progress next says how long it has run.
    #[cfg(not(target_arch = "wasm32"))]
    tick: Option<Instant>,
    running: FxHashMap<String, Running>,
    /// The results file stem of each document run so far.
    stems: FxHashMap<String, String>,
}

struct Running {
    #[cfg(not(target_arch = "wasm32"))]
    id: u64,
    cancel: Arc<AtomicBool>,
    request: RequestId,
    /// The token of its progress, when the client shows progress.
    progress: Option<String>,
    #[cfg(not(target_arch = "wasm32"))]
    started: Instant,
}

/// How often a run's progress says how long it has run.
#[cfg(not(target_arch = "wasm32"))]
const PROGRESS_EVERY: Duration = Duration::from_secs(1);

/// A finished run, for the main loop.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Done {
    id: u64,
    uri: String,
    report: Report,
}

/// What a run produced.
struct Report {
    /// "12 rows in 34 ms", "true in 2 ms", or the error.
    summary: String,
    /// The query failed (the message is a warning).
    failed: bool,
    results: Results,
}

/// Where a run's results are.
enum Results {
    /// The result object, for [`Delivery::Object`].
    Object(Value),
    /// The results document written.
    File(PathBuf),
    /// None: embedded, no cache directory, the file could not be written,
    /// or the run was cancelled.
    None,
}

/// Where a run's results go.
enum Target {
    /// Into the result object.
    Object,
    /// Into a results document in this folder.
    Folder(PathBuf),
    /// Nowhere: the summary only (embedded, or no cache directory).
    Summary,
}

/// One run, as handed to its thread.
struct Job {
    uri: String,
    name: String,
    text: String,
    base: String,
    prefixes: Arc<PrefixMap>,
    docs: Vec<Arc<Analysis>>,
    timeout: Option<Duration>,
    max_rows: Option<usize>,
    max_buffered_rows: Option<usize>,
    inference: Inference,
    cancel: Arc<AtomicBool>,
    target: Target,
}

impl State {
    /// Runs for a client given the results by `delivery`, asked for the
    /// lenses again with the `display_refresh` requests.
    pub(crate) fn new(delivery: Delivery, display_refresh: &[&str], spawn: Spawn) -> State {
        #[cfg(not(target_arch = "wasm32"))]
        let (done_tx, done_rx) = unbounded();
        State {
            delivery,
            lens_refresh: display_refresh.contains(&CodeLensRefresh::METHOD),
            spawn,
            #[cfg(not(target_arch = "wasm32"))]
            done_tx,
            #[cfg(not(target_arch = "wasm32"))]
            done_rx,
            #[cfg(not(target_arch = "wasm32"))]
            next_id: 0,
            #[cfg(not(target_arch = "wasm32"))]
            tick: None,
            running: FxHashMap::default(),
            stems: FxHashMap::default(),
        }
    }

    /// Finished runs, for the main loop.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn results(&self) -> Receiver<Done> {
        self.done_rx.clone()
    }

    /// When [`State::on_tick`] is due, for the main loop's timer.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn due(&self) -> Option<Instant> {
        self.tick
    }

    /// The results file stem of `uri`: its file stem, numbered when another
    /// document run before has the same one (in any case: `Same` and `same`
    /// are one file on macOS and Windows).
    fn stem_for(&mut self, uri: &str) -> String {
        if let Some(s) = self.stems.get(uri) {
            return s.clone();
        }
        let base = stem(uri);
        let mut s = base.clone();
        for n in 2.. {
            let folded = s.to_lowercase();
            if !self.stems.values().any(|t| t.to_lowercase() == folded) {
                break;
            }
            s = format!("{base}-{n}");
        }
        self.stems.insert(uri.to_owned(), s.clone());
        s
    }

    /// Stop every run (the server is going away).
    pub(crate) fn cancel_all(&self) {
        for r in self.running.values() {
            r.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        self.cancel_all();
    }
}

/// `<cache dir>/query-results`.
fn results_dir() -> Option<PathBuf> {
    crate::imports::cache_dir().map(|d| d.join("query-results"))
}

impl State {
    /// One of [`COMMANDS`], asked by request `id`, over `ws`: the answer as
    /// JSON text when answered now, None when it comes with the result.
    pub(crate) fn command(
        &mut self,
        #[cfg_attr(target_arch = "wasm32", allow(unused_variables))] id: RequestId,
        params: ExecuteCommandParams,
        ws: &mut Workspace,
        settings: &Settings,
        c: &mut Client,
        p: &mut Progress,
    ) -> Result<Option<String>, (ErrorCode, String)> {
        let invalid = |e: String| (ErrorCode::InvalidParams, e);
        let (uri, options) = match (params.command.as_str(), params.arguments.as_slice()) {
            (_, [Value::String(uri)]) | (RUN_QUERY, [Value::String(uri), Value::Null]) => {
                (uri, Options::default())
            }
            (RUN_QUERY, [Value::String(uri), options]) => {
                let o = Options::deserialize(options)
                    .map_err(|e| invalid(format!("{RUN_QUERY}: {e}")))?;
                (uri, o)
            }
            (RUN_QUERY, _) => {
                return Err(invalid(format!(
                    "{RUN_QUERY} takes a document URI, then optionally {{\"reasoning\": \"off\" | \"default\" | \"complete\"}}"
                )));
            }
            (command, _) => return Err(invalid(format!("{command} takes one document URI"))),
        };
        let uri = normalize_uri(uri);
        if params.command == CANCEL_QUERY {
            self.cancel(&uri, c, p);
            return json(&Value::Null).map(Some);
        }
        let (text, base, prefixes) = match ws.sparql(&uri) {
            Some(q) => (q.text().to_owned(), q.base().to_owned(), q.external.clone()),
            None => {
                let path = uri_to_path(&uri)
                    .filter(|p| is_sparql_path(p) && !ws.is_open(&uri))
                    .ok_or_else(|| {
                        (
                            ErrorCode::InvalidParams,
                            format!("{uri} is not a SPARQL document"),
                        )
                    })?;
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| (ErrorCode::InvalidParams, format!("{}: {e}", path.display())))?;
                (text, uri.clone(), ws.external_prefixes_for(&uri))
            }
        };
        let rules = options
            .reasoning
            .map_or(settings.query_reasoning, Reasoning::from);
        let docs = super::query::sources(ws.knowledge(), rules, |_| true);
        let job = Job {
            uri: uri.clone(),
            name: file_name(&uri),
            text,
            base,
            prefixes,
            docs,
            timeout: settings.query_timeout,
            max_rows: settings.query_max_rows,
            max_buffered_rows: settings.query_max_buffered_rows,
            inference: Inference {
                rules,
                max_triples: settings.reasoning_max_triples,
            },
            cancel: Arc::default(),
            target: match (self.delivery, self.spawn) {
                (Delivery::Object, _) => Target::Object,
                (_, Spawn::Thread) => results_dir().map_or(Target::Summary, Target::Folder),
                (_, Spawn::Inline) => Target::Summary,
            },
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.spawn == Spawn::Thread {
            return self.start(id, uri, job, c, p);
        }
        let report = execute(&job, &self.stem_for(&uri));
        json(&self.deliver(c, &job.name, report)).map(Some)
    }

    /// The rest of [`State::command`] on a thread: start the run; its
    /// answer comes with the result.
    #[cfg(not(target_arch = "wasm32"))]
    fn start(
        &mut self,
        id: RequestId,
        uri: String,
        job: Job,
        c: &mut Client,
        p: &mut Progress,
    ) -> Result<Option<String>, (ErrorCode, String)> {
        // A newer run of the same document replaces the one going on.
        if let Some(old) = self.stop(&uri, "Replaced by a newer run", c, p) {
            let answer = self.cancelled(&uri, "cancelled: a newer run replaced it");
            respond(c, old.request, &answer);
        }
        self.next_id += 1;
        let run_id = self.next_id;
        let token = p.begin_cancellable(c, &format!("Running {}", job.name));
        if token.is_some() && self.tick.is_none() {
            self.tick = Some(Instant::now() + PROGRESS_EVERY);
        }
        self.running.insert(
            uri.clone(),
            Running {
                id: run_id,
                cancel: job.cancel.clone(),
                request: id,
                progress: token,
                started: Instant::now(),
            },
        );
        if self.lens_refresh {
            c.request(CodeLensRefresh::METHOD, ());
        }
        let out = self.done_tx.clone();
        let stem = self.stem_for(&uri);
        let job = Arc::new(job);
        let failed = job.clone();
        self.spawn
            .run(
                // Also the name a panic's message gives the run.
                "run query",
                16 << 20,
                move || execute(&job, &stem),
                move |report| {
                    let report = report.unwrap_or_else(|msg| panicked(&failed, msg));
                    let _ = out.send(Done {
                        id: run_id,
                        uri,
                        report,
                    });
                },
            )
            .map_err(|e| (ErrorCode::InternalError, e.to_string()))?;
        Ok(None)
    }

    /// A run finished: show it and answer its request (unless it was
    /// replaced or cancelled meanwhile).
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn on_done(&mut self, done: Done, c: &mut Client, p: &mut Progress) {
        if self.running.get(&done.uri).map(|r| r.id) != Some(done.id) {
            return;
        }
        let name = file_name(&done.uri);
        let end = done.report.summary.clone();
        let answer = self.deliver(c, &name, done.report);
        if let Some(run) = self.stop(&done.uri, &end, c, p) {
            respond(c, run.request, &answer);
        }
    }

    /// Stop the run of `uri`, if one goes on: end its progress with `end`
    /// and show the lens that runs it again. The caller answers its request.
    fn stop(&mut self, uri: &str, end: &str, c: &mut Client, p: &mut Progress) -> Option<Running> {
        let run = self.running.remove(uri)?;
        run.cancel.store(true, Ordering::Relaxed);
        if let Some(token) = &run.progress {
            p.end_token(c, token, end.into());
        }
        if self.lens_refresh {
            c.request(CodeLensRefresh::METHOD, ());
        }
        Some(run)
    }

    /// `turtle.cancelQuery` for `uri`: stop its run, answered "cancelled".
    fn cancel(&mut self, uri: &str, c: &mut Client, p: &mut Progress) {
        if let Some(run) = self.stop(uri, "Cancelled", c, p) {
            let answer = self.cancelled(uri, "cancelled");
            respond(c, run.request, &answer);
        }
    }

    /// The answer of `uri`'s cancelled run: `{"cancelled": true, "uri"}` for
    /// [`Delivery::Object`], else `summary`.
    fn cancelled(&self, uri: &str, summary: &str) -> Value {
        match self.delivery {
            Delivery::Object => json!({ "cancelled": true, "uri": uri }),
            Delivery::Document | Delivery::Message => summary.into(),
        }
    }

    /// The client cancelled the progress `token`: when it is a run's, stop
    /// the run.
    pub(crate) fn on_progress_cancel(&mut self, token: &str, c: &mut Client, p: &mut Progress) {
        let uri = self
            .running
            .iter()
            .find(|(_, r)| r.progress.as_deref() == Some(token))
            .map(|(u, _)| u.clone());
        if let Some(uri) = uri {
            self.cancel(&uri, c, p);
        }
    }

    /// [`State::due`]: each run's progress says how long it has run, again
    /// in a second while any shows progress.
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn on_tick(&mut self, c: &Client, p: &Progress) {
        let now = Instant::now();
        self.tick = None;
        for run in self.running.values() {
            if let Some(token) = &run.progress {
                let secs = now.saturating_duration_since(run.started).as_secs();
                p.report(c, token, format!("{secs} s"));
                self.tick = Some(now + PROGRESS_EVERY);
            }
        }
    }

    /// While `uri`'s query runs, its "▶ Run query" lens cancels the run
    /// instead: only for a client that is asked for the lenses again when
    /// the run ends, as one that is not would keep showing "■ Cancel query".
    pub(crate) fn lenses(&self, uri: &str, lenses: &mut [CodeLens]) {
        let uri = normalize_uri(uri);
        if !self.lens_refresh || !self.running.contains_key(&uri) {
            return;
        }
        for c in lenses.iter_mut().filter_map(|l| l.command.as_mut()) {
            if c.command == RUN_QUERY {
                c.title = CANCEL_QUERY_TITLE.into();
                c.command = CANCEL_QUERY.into();
            }
        }
    }

    /// A `$/cancelRequest` for `id`: when it is a pending run, stop it and
    /// answer `RequestCanceled`.
    pub(crate) fn on_cancel_request(&mut self, id: &RequestId, c: &mut Client, p: &mut Progress) {
        let Some(uri) = self
            .running
            .iter()
            .find(|(_, r)| r.request == *id)
            .map(|(u, _)| u.clone())
        else {
            return;
        };
        if let Some(run) = self.stop(&uri, "Cancelled", c, p) {
            let resp = Response::new_err(
                run.request,
                ErrorCode::RequestCanceled as i32,
                "request cancelled".into(),
            );
            let _ = c.send(resp);
        }
    }

    /// The answer to the run's request: the result object, else the
    /// summary, once the results file is opened or the summary shown.
    fn deliver(&self, client: &mut Client, name: &str, report: Report) -> Value {
        let file = match report.results {
            Results::Object(v) => return v,
            Results::File(p) => path_to_uri(&p).and_then(|u| u.parse::<Uri>().ok()),
            Results::None => None,
        };
        match file {
            Some(uri) if self.delivery == Delivery::Document => {
                let params = ShowDocumentParams {
                    uri,
                    external: None,
                    take_focus: Some(false),
                    selection: None,
                };
                client.request(lsp_types::request::ShowDocument::METHOD, params);
            }
            _ => {
                let typ = if report.failed {
                    MessageType::WARNING
                } else {
                    MessageType::INFO
                };
                client.show(typ, format!("{name}: {}", report.summary));
            }
        }
        Value::String(report.summary)
    }
}

/// Answer request `id` with `answer`, as JSON text (`server::wire`).
fn respond(c: &Client, id: RequestId, answer: &Value) {
    let _ = match json(answer) {
        Ok(text) => c.respond(id, text),
        Err((code, message)) => c.send(Response::new_err(id, code as i32, message)),
    };
}

/// The last path segment of `uri`, percent-decoded where it can be.
fn file_name(uri: &str) -> String {
    crate::workspace::file_uri_name(uri)
        .unwrap_or_else(|| uri.rsplit('/').next().unwrap_or(uri).to_owned())
}

/// The results file's stem: the document's name without its extension.
fn stem(uri: &str) -> String {
    let name = file_name(uri);
    let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
    let clean: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':') {
                '_'
            } else {
                c
            }
        })
        .collect();
    if clean.is_empty() {
        "query".into()
    } else {
        clean
    }
}

/// Run `job` and put its results where [`Job::target`] says: the result
/// object, or the results document `<folder>/<stem>.md`.
fn execute(job: &Job, stem: &str) -> Report {
    let start = Instant::now();
    let docs: Vec<_> = job.docs.iter().collect();
    let limits = Limits {
        timeout: job.timeout,
        cancel: Some(job.cancel.clone()),
        max_rows: job.max_rows,
        max_buffered_rows: job.max_buffered_rows,
    };
    let outcome = query::run(
        &docs,
        &job.text,
        Some(&job.base),
        &job.prefixes,
        &limits,
        &job.inference,
    );
    let elapsed = Instant::now().saturating_duration_since(start);
    let (outcome, inferred) = match outcome {
        Ok(o) => (Ok(o.answer), o.inferred),
        Err(e) => (Err(e), Inferred::None),
    };
    let summary = match &outcome {
        Ok(a) => format!("{} in {}", answer_summary(a), duration(elapsed)),
        Err(QueryError::Timeout) => format!(
            "stopped at the time limit ({}, query.timeoutSeconds)",
            duration(job.timeout.unwrap_or(elapsed))
        ),
        Err(QueryError::TooManyRows(n)) => {
            format!("stopped: it kept more than {n} rows in memory (query.maxBufferedRows)")
        }
        Err(e) => e.to_string(),
    };
    let failed = outcome.is_err();
    if matches!(outcome, Err(QueryError::Cancelled)) || job.cancel.load(Ordering::Relaxed) {
        return Report {
            summary: "cancelled".into(),
            failed: true,
            results: Results::None,
        };
    }
    let results = match &job.target {
        Target::Object => {
            let answer = match &outcome {
                Ok(a) => answer_json(a),
                Err(e) => error_json(e, &summary),
            };
            Results::Object(object(
                job,
                elapsed,
                &summary,
                answer,
                inferred_json(inferred),
            ))
        }
        Target::Folder(dir) => {
            let md = document(job, &outcome, inferred, elapsed);
            let path = dir.join(format!("{stem}.md"));
            match std::fs::create_dir_all(dir).and_then(|()| std::fs::write(&path, md)) {
                Ok(()) => Results::File(path),
                Err(_) => Results::None,
            }
        }
        Target::Summary => Results::None,
    };
    Report {
        summary,
        failed,
        results,
    }
}

/// The report of `job`'s run whose thread panicked with `msg`.
#[cfg(not(target_arch = "wasm32"))]
fn panicked(job: &Job, msg: String) -> Report {
    let results = match job.target {
        Target::Object => {
            let answer = json!({ "kind": "error", "message": capitalize(&msg) });
            Results::Object(object(job, Duration::ZERO, &msg, answer, None))
        }
        Target::Folder(_) | Target::Summary => Results::None,
    };
    Report {
        summary: msg,
        failed: true,
        results,
    }
}

/// The result object of `job`'s run.
fn object(
    job: &Job,
    elapsed: Duration,
    summary: &str,
    answer: Value,
    inferred: Option<Value>,
) -> Value {
    let mut v = json!({
        "uri": job.uri,
        "name": job.name,
        "files": job.docs.len(),
        "elapsedMs": u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        "summary": summary,
        "reasoning": query::rules_name(job.inference.rules),
        "prefixes": query::display_prefixes(&job.text, &job.prefixes),
        "answer": answer,
    });
    if let Some(note) = inferred {
        v["inferred"] = note;
    }
    v
}

/// The `answer` of a run that failed with `e`, summarized as `summary`:
/// the parser's message and position for a syntax error, else the summary.
fn error_json(e: &QueryError, summary: &str) -> Value {
    match e {
        QueryError::Syntax {
            message,
            line,
            column,
            ..
        } => json!({ "kind": "error", "message": message, "line": line, "column": column }),
        _ => json!({ "kind": "error", "message": capitalize(summary) }),
    }
}

/// "12 rows", "1 row", "true", "40 triples": the answer's size, the total
/// when rows were left out.
fn answer_summary(a: &Answer) -> String {
    let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    match a {
        Answer::Boolean(b) => b.to_string(),
        Answer::Solutions(s) => count(s.total(), "row"),
        Answer::Triples(t) => count(t.total(), "triple"),
    }
}

/// "34 ms", "1.2 s".
fn duration(d: Duration) -> String {
    if d < Duration::from_secs(1) {
        format!("{} ms", d.as_millis())
    } else if d.subsec_millis() == 0 {
        format!("{} s", d.as_secs())
    } else {
        format!("{:.1} s", d.as_secs_f64())
    }
}

/// The results document: a title with the query's name, how long it took
/// over how many files, what reasoning added, then the results (or the
/// error).
fn document(
    job: &Job,
    outcome: &Result<Answer, QueryError>,
    inferred: Inferred,
    elapsed: Duration,
) -> String {
    let files = job.docs.len();
    let mut out = format!(
        "# {}\n\nRan in {} over {files} file{}.\n\n",
        job.name,
        duration(elapsed),
        if files == 1 { "" } else { "s" }
    );
    if let Some(line) = inferred.sentence() {
        out.push_str(&line);
        out.push_str("\n\n");
    }
    match outcome {
        Ok(a) => {
            let prefixes = query::display_prefixes(&job.text, &job.prefixes);
            out.push_str(&query::render::markdown(a, &prefixes, job.max_rows));
        }
        Err(e) => {
            out.push_str("## Error\n\n");
            let line = match e {
                QueryError::Syntax {
                    message,
                    line,
                    column,
                    ..
                } => format!("Syntax error at {line}:{column}: {message}"),
                QueryError::Timeout => format!(
                    "The query was stopped at the time limit ({}, `query.timeoutSeconds`).",
                    duration(job.timeout.unwrap_or(elapsed))
                ),
                QueryError::TooManyRows(n) => format!(
                    "The query was stopped: it kept more than {n} rows in memory (`query.maxBufferedRows`)."
                ),
                e => capitalize(&e.to_string()),
            };
            out.push_str(&line);
            out.push('\n');
        }
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_stems() {
        assert_eq!(file_name("file:///w/q%20one.rq"), "q one.rq");
        assert_eq!(stem("file:///w/people.rq"), "people");
        assert_eq!(stem("file:///w/a.b.sparql"), "a.b");
        assert_eq!(stem("untitled:Untitled-1"), "untitled_Untitled-1");
    }

    #[test]
    fn stems_differ_by_more_than_case() {
        let mut state = State::new(Delivery::Message, &[], Spawn::Inline);
        assert_eq!(state.stem_for("file:///a/Same.rq"), "Same");
        assert_eq!(state.stem_for("file:///b/same.rq"), "same-2");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_millis(34)), "34 ms");
        assert_eq!(duration(Duration::from_millis(1234)), "1.2 s");
        assert_eq!(duration(Duration::from_secs(30)), "30 s");
    }

    fn file(r: Report) -> PathBuf {
        match r.results {
            Results::File(p) => p,
            Results::Object(_) | Results::None => panic!("no results file: {}", r.summary),
        }
    }

    #[test]
    fn a_run_writes_the_results_document() {
        let dir = std::env::temp_dir().join(format!("turtle-lsp-run-query-{}", std::process::id()));
        let data = Arc::new(Analysis::new(
            "file:///w/d.ttl".into(),
            "@prefix ex: <http://example.org/> .\nex:a ex:p 1 .\nex:b ex:p 2 .\n",
            Arc::new(PrefixMap::default()),
            false,
        ));
        let job = |text: &str| Job {
            uri: "file:///w/q.rq".into(),
            name: "q.rq".into(),
            text: text.into(),
            base: "file:///w/q.rq".into(),
            prefixes: Arc::new(PrefixMap::from_iter([(
                "ex".to_string(),
                "http://example.org/".to_string(),
            )])),
            docs: vec![data.clone()],
            timeout: Some(Duration::from_secs(5)),
            max_rows: Some(1),
            max_buffered_rows: None,
            inference: Inference::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            target: Target::Folder(dir.clone()),
        };
        let r = execute(&job("SELECT ?s WHERE { ?s ex:p ?o } ORDER BY ?o"), "q");
        assert!(r.summary.starts_with("2 rows in "), "{}", r.summary);
        let md = std::fs::read_to_string(file(r)).unwrap();
        assert!(md.starts_with("# q.rq\n\nRan in "), "{md}");
        assert!(md.contains("over 1 file.\n\n2 rows\n"), "{md}");
        assert!(md.contains("| `ex:a` |") && !md.contains("`ex:b`"), "{md}");
        assert!(md.ends_with("_Showing 1 of 2 rows._\n"), "{md}");

        let over = Job {
            max_buffered_rows: Some(1),
            ..job("SELECT ?s WHERE { ?s ex:p ?o } ORDER BY ?o")
        };
        let r = execute(&over, "q");
        assert_eq!(
            (r.failed, r.summary.as_str()),
            (
                true,
                "stopped: it kept more than 1 rows in memory (query.maxBufferedRows)"
            )
        );
        let md = std::fs::read_to_string(file(r)).unwrap();
        assert!(
            md.contains("kept more than 1 rows in memory (`query.maxBufferedRows`)."),
            "{md}"
        );

        let r = execute(&job("SELECT ?s WHERE { ?s ex:p }"), "q");
        assert!(
            r.failed && r.summary.starts_with("syntax error at 1:"),
            "{}",
            r.summary
        );
        let md = std::fs::read_to_string(file(r)).unwrap();
        assert!(md.contains("## Error\n\nSyntax error at 1:"), "{md}");

        let cancelled = job("ASK {}");
        cancelled.cancel.store(true, Ordering::Relaxed);
        let r = execute(&cancelled, "q");
        assert_eq!(r.summary, "cancelled");
        assert!(matches!(r.results, Results::None));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
