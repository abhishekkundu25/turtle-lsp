//! LSP main loop.
//!
//! Single-threaded state, with two kinds of background work:
//! - the initial workspace index is built on a separate thread (in parallel via
//!   rayon) and merged when it arrives, so `initialize` answers immediately;
//!   indexed files parsed again after a `prefixes.ttl` change come the same way;
//! - diagnostics are published only after the incoming queue is drained, so a
//!   burst of keystrokes costs one analysis, not one per keystroke.
//!
//! The loop runs on a thread with a large stack (`main`), so a deeply nested
//! document cannot overflow it. Each thing it wakes for (a message, a worker's
//! answer, a timer) and each phase after a batch runs on its own under
//! [`guard`]: a bug in one is reported on stderr and the session goes on. The
//! documents themselves are analyzed under a guard too (`Workspace`): a
//! document whose analysis panics shows an internal-error diagnostic. Threads
//! start through `worker::Spawn` (inline when embedded, where there are no
//! threads) and their answers come back through `worker::Feed`; the timers
//! are keyed in `timers::Timers`.
//!
//! The queue is drained as one batch before anything is answered, so a request
//! whose `$/cancelRequest` is already queued is never computed.
//!
//! Diagnostics are pushed (`publishDiagnostics`), or pulled
//! (`textDocument/diagnostic`) when the client supports that. Pulled reports
//! carry a result id made of the document version and the knowledge
//! generation: anything outside the document that can change its diagnostics
//! (prefixes.ttl, the disk index, settings) bumps the generation.
//!
//! Requests are answered by `ide::Session`, which routes each by document
//! kind: RDF documents to `features`, SPARQL documents to
//! `sparql::features`; neither runs on the other.
//! The one custom request, `turtle/graph` (listed under `experimental`),
//! runs on a thread of its own over a copy of the knowledge index and
//! answers from there.
//!
//! The server's state is in groups, each a struct of its own that the server
//! drives: `progress` (work-done progress), `shacl` (validation scheduling
//! and results), `refresh` (pulled-diagnostics result ids and refresh
//! requests), `layers` (the settings and their layers) and `index` (the
//! workspace index as it arrives). They send to the client through
//! `client::Client`, the connection's sender and request ids.
//!
//! The connection is `wire`'s: a request's result goes out as JSON text,
//! never as a `serde_json::Value`, through one ordered channel to a writer
//! thread, with the bytes lsp-server would write. Embedded, the host gets
//! `lsp_server::Message`s as before.

use crate::analysis::Encoding;
use crate::clock::Instant;
use crate::diagnose::DiagnosticInputs;
use crate::features::graph::{GraphParams, GraphResult, InvalidParams};
use crate::features::{diagnostics, navigation, shacl_validate};
use crate::ide;
use crate::settings::{SETTINGS_FILE, Settings};
use crate::workspace::{
    Change, Dirty, DocKind, Effects, Workspace, is_import_source, normalize_uri, uri_to_path,
};
use crossbeam_channel::{Receiver, Sender, never, unbounded};
use lsp_server::{ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::*;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::Arc;
use std::time::Duration;

mod client;
mod index;
mod layers;
mod progress;
mod refresh;
mod run_query;
mod shacl;
mod timers;
mod wire;
pub(crate) mod worker;
mod workspace_diagnostics;

use client::Client;
use index::Index;
use layers::Layers;
use progress::{Progress, Work};
use refresh::Refresh;
use shacl::Shacl;
use timers::{Timer, Timers};
use wire::Out;
use worker::{Fed, Feed, Spawn};

/// The files the client is asked to report changes of: RDF documents, SPARQL
/// queries, JSON-LD context files (`.json` too), imports sources and the
/// settings file (a `.json` file). It is the glob of every row of the
/// file-kind table ([`crate::workspace::kinds::ROWS`]), which a test checks.
pub(crate) const WATCHED_FILES: &str = "**/{*.ttl,*.turtle,*.trig,*.nt,*.nq,*.jsonld,*.json,*.rdf,*.owl,*.rq,*.sparql,*.ru,catalog-v001.xml}";

/// Pause after a described-set change before other documents are refreshed.
const DESCRIBED_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// How long a client that has sent `shutdown` is waited for to send `exit`.
const EXIT_WAIT: Duration = Duration::from_secs(30);

/// The stack of the thread that runs the main loop (see `main`): deep
/// documents nest deeply, and the OS default (1 MiB on Windows) is too
/// little for the deepest the parsers accept.
pub const MAIN_STACK: usize = 64 << 20;

/// Stack of the threads that parse and analyze.
#[cfg(not(target_arch = "wasm32"))]
const WORKER_STACK: usize = 16 << 20;

/// How the session ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ended {
    /// `shutdown`, then `exit`.
    Exit,
    /// `exit` without `shutdown`.
    ExitWithoutShutdown,
    /// The client closed the connection.
    ClientGone,
    /// `shutdown` came and `exit` did not, within [`EXIT_WAIT`]. The reader
    /// of the connection is blocked on the client's pipe: leave it.
    ShutdownTimedOut,
}

impl Ended {
    /// The process should exit with code 0 (the LSP spec asks 1 otherwise).
    #[cfg(not(target_arch = "wasm32"))]
    fn clean(self) -> bool {
        self == Ended::Exit
    }
}

/// Pause after the last edit before the workspace is validated (`shacl.onType`).
const SHACL_TYPING_DEBOUNCE: Duration = Duration::from_secs(1);

/// The environment variable that, set to `1`, makes the server parse each
/// edited document both ways (the statement splice and in full) and log how
/// each analysis was parsed and the first difference (see
/// [`Workspace::set_check_incremental`]). For tests and debugging.
const CHECK_INCREMENTAL: &str = "TURTLE_LSP_CHECK_INCREMENTAL";

/// A SHACL run shows progress only once it has lasted this long.
const SHACL_PROGRESS_DELAY: Duration = Duration::from_millis(200);

pub(crate) struct Server {
    client: Client,
    /// Every message read so far, queued as the reader reads them, so that
    /// a batch holds every message that has arrived.
    inbox: Receiver<Message>,
    ws: Workspace,
    enc: Encoding,
    dirty: FxHashSet<String>,
    /// How the client spelled an open document's URI, when that differs
    /// from the key (`%7E` for `~`, `c%3A` for `C:`): diagnostics go back
    /// under the client's spelling.
    spelling: FxHashMap<String, String>,
    index: Index,
    watch_dynamic: bool,
    /// The client resolves a code action's edit lazily (`codeAction/resolve`).
    resolve_edits: bool,
    layers: Layers,
    refresh: Refresh,
    /// What waits for a time to come: acting on a described-set change (see
    /// `check_described`), validating after typing stops (`shacl.onType`;
    /// each edit pushes it back), showing a long SHACL run's progress.
    timers: Timers,
    imports: crate::imports::Imports,
    /// JSON-LD contexts fetched in the background (with `imports.network`),
    /// by URL, once each has arrived or failed.
    contexts: Feed<String>,
    shacl: Shacl,
    progress: Progress,
    /// How the jobs run: on threads, or inline when embedded in a host
    /// without threads (see [`Embedded`]), where the host drives the timers.
    spawn: Spawn,
    /// Diagnostics of the files that are not open (`diagnostics.workspace`).
    wsdiag: workspace_diagnostics::State,
    /// The "Run query" runs going on.
    queries: run_query::State,
    /// Tests: panic once after the next batch.
    #[cfg(test)]
    fault: bool,
}

impl Server {
    /// A server for the client's `initialize` params (`raw`), sending on
    /// `sender`, not started, with the initialize result to answer (JSON
    /// text) and the workspace roots. It reads no messages and indexes
    /// nothing: the caller wires both.
    #[allow(clippy::type_complexity)]
    fn configure(
        sender: Sender<Out>,
        raw: Value,
        spawn: Spawn,
    ) -> Result<(Server, String, Vec<PathBuf>), Box<dyn std::error::Error + Send + Sync>> {
        let params: InitializeParams = serde_json::from_value(raw.clone())?;
        let utf8 = params
            .capabilities
            .general
            .as_ref()
            .and_then(|g| g.position_encodings.as_ref())
            .is_some_and(|e| e.contains(&PositionEncodingKind::UTF8));
        let enc = if utf8 {
            Encoding::Utf8
        } else {
            Encoding::Utf16
        };
        let watch_dynamic = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.did_change_watched_files.as_ref())
            .and_then(|d| d.dynamic_registration)
            .unwrap_or(false);
        // The spec names this `workspace.diagnostics`; lsp-types reads `diagnostic`.
        let refresh_support = ["diagnostics", "diagnostic"].iter().any(|k| {
            raw.pointer(&format!("/capabilities/workspace/{k}/refreshSupport"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        });
        // Pull only when the client can also be told to pull again: a change in one
        // file (prefixes.ttl, SHACL shapes) must reach every open document, and
        // without refresh a pull client keeps stale diagnostics. Push covers that.
        let pull = refresh_support
            && params
                .capabilities
                .text_document
                .as_ref()
                .is_some_and(|t| t.diagnostic.is_some());
        let display_refresh = [
            (
                "inlayHint",
                lsp_types::request::InlayHintRefreshRequest::METHOD,
            ),
            (
                "semanticTokens",
                lsp_types::request::SemanticTokensRefresh::METHOD,
            ),
            // Reference counts, and the codeLens.references setting.
            ("codeLens", lsp_types::request::CodeLensRefresh::METHOD),
        ]
        .into_iter()
        .filter(|(k, _)| {
            raw.pointer(&format!("/capabilities/workspace/{k}/refreshSupport"))
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .map(|(_, m)| m)
        .collect();
        let work_done_progress = raw
            .pointer("/capabilities/window/workDoneProgress")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let show_document = raw
            .pointer("/capabilities/window/showDocument/support")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let config_pull = params
            .capabilities
            .workspace
            .as_ref()
            .and_then(|w| w.configuration)
            .unwrap_or(false);
        let resolve_edits = params
            .capabilities
            .text_document
            .as_ref()
            .and_then(|t| t.code_action.as_ref())
            .and_then(|c| c.resolve_support.as_ref())
            .is_some_and(|r| r.properties.iter().any(|p| p == "edit"));

        let mut roots: Vec<PathBuf> = params
            .workspace_folders
            .iter()
            .flatten()
            .filter_map(|f| uri_to_path(f.uri.as_str()))
            .collect();
        #[allow(deprecated)]
        if roots.is_empty()
            && let Some(r) = params
                .root_uri
                .as_ref()
                .and_then(|u| uri_to_path(u.as_str()))
        {
            roots.push(r);
        }
        let file_settings = match spawn {
            Spawn::Thread => layers::read_file(roots.first()),
            Spawn::Inline => (Value::Null, None),
        };
        let init_options = params.initialization_options.clone().unwrap_or_default();
        let layers = Layers::new(file_settings, init_options, config_pull);
        let settings = layers.settings();
        // Embedded, every file is open.
        let wsdiag = workspace_diagnostics::State::new(
            settings.diagnostics_workspace && spawn == Spawn::Thread,
        );
        let mut ws = Workspace::new(roots.clone(), settings.prefixes.clone());
        // Without watchers (see `register_watchers`), nothing reports new files.
        ws.set_rescan_sparql(!watch_dynamic);
        ws.set_incremental(incremental(settings, spawn));
        ws.set_check_incremental(std::env::var(CHECK_INCREMENTAL).is_ok_and(|v| v == "1"));
        let contexts = use_network(&mut ws, settings.imports_network, spawn);

        let result = wire::encode(&Initialized::new(capabilities(enc, pull)))?;

        let server = Server {
            client: Client::new(sender),
            inbox: never(),
            ws,
            enc,
            dirty: FxHashSet::default(),
            spelling: FxHashMap::default(),
            index: Index::new(),
            watch_dynamic,
            resolve_edits,
            layers,
            refresh: Refresh::new(pull, refresh_support, display_refresh),
            timers: Timers::default(),
            imports: crate::imports::Imports::with_spawn(spawn),
            contexts,
            shacl: Shacl::default(),
            progress: Progress::new(work_done_progress),
            wsdiag,
            queries: run_query::State::new(show_document),
            spawn,
            #[cfg(test)]
            fault: false,
        };
        Ok((server, result, roots))
    }
}

/// Whether edits are parsed with the statement splice: as the settings say,
/// but not embedded (the browser engine parses in full, as `check` does).
fn incremental(settings: &Settings, spawn: Spawn) -> bool {
    settings.parse_incremental && spawn == Spawn::Thread
}

/// Load JSON-LD contexts with the network when `network` is on: in the
/// background, so the main loop never waits for a fetch (inline when
/// embedded, where there are no threads). The receiver names each context
/// fetched.
fn use_network(ws: &mut Workspace, network: bool, spawn: Spawn) -> Feed<String> {
    #[cfg(not(target_arch = "wasm32"))]
    if network && spawn == Spawn::Thread {
        let (loader, fetched) =
            crate::jsonld::loader::DesktopLoader::in_background(crate::imports::import_cache_dir());
        ws.set_context_loader(Arc::new(loader));
        return Feed::open(fetched);
    }
    let _ = spawn;
    ws.set_network(network);
    Feed::default()
}

/// Returns whether the session ended cleanly (`shutdown` then `exit`); the
/// process should exit with code 1 otherwise, as the LSP spec asks.
#[cfg(not(target_arch = "wasm32"))]
pub fn run() -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // Workspace files are parsed on rayon threads; give them room beyond the
    // 2 MB default (parser nesting is bounded, this is defence in depth).
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(8 << 20)
        .build_global();
    let (out, inbox, io) = wire::stdio()?;
    let (id, raw) = wire::initialize_start(&out, &inbox)?;

    let (mut server, result, roots) = Server::configure(out.clone(), raw, Spawn::Thread)?;
    wire::initialize_finish(&out, &inbox, id, result)?;
    // The server holds the only sender from here (and a `turtle/graph`
    // thread while it runs): the writer ends with them.
    drop(out);

    // Index the workspace in the background. What changes on disk meanwhile
    // is remembered, so that the index does not undo it when it lands.
    server.ws.expect_initial_index();
    let (tx, index_rx) = unbounded();
    let prefixes = server.ws.settings_prefixes().clone();
    // The same context loader: a context is fetched once, in the background.
    let contexts = server.ws.shared_contexts();
    let failed = tx.clone();
    let started = server.spawn.run(
        "index",
        WORKER_STACK,
        move || {
            let mut tmp = Workspace::new(roots.clone(), prefixes);
            tmp.use_shared_contexts(contexts);
            let files = Workspace::scan_roots(&roots);
            tmp.parse_paths(files)
        },
        move |files| {
            let _ = tx.send(files);
        },
    );
    if let Err(e) = started {
        let _ = failed.send(Err(format!(
            "turtle-lsp could not start its indexing thread: {e}"
        )));
    }
    server.index.expect(index_rx);

    server.inbox = inbox;

    let ended = server.main_loop()?;
    // The writer thread ends only when every sender is gone; joining while the
    // server still holds one would keep the process alive forever after the
    // editor disappears (stdin closed without shutdown/exit).
    drop(server);
    end_session(ended, || io.join())
}

/// Wait for the connection's threads, unless the client sent `shutdown` and
/// never `exit`: its reader waits on a pipe that stays open, and the process
/// ends without it. Whether the session ended cleanly.
#[cfg(not(target_arch = "wasm32"))]
fn end_session<E>(
    ended: Ended,
    join: impl FnOnce() -> Result<(), E>,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>>
where
    Box<dyn std::error::Error + Send + Sync>: From<E>,
{
    if ended != Ended::ShutdownTimedOut {
        join()?;
    }
    Ok(ended.clean())
}

fn capabilities(enc: Encoding, pull: bool) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(match enc {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(TextDocumentSyncCapability::Options(
            TextDocumentSyncOptions {
                open_close: Some(true),
                change: Some(TextDocumentSyncKind::INCREMENTAL),
                save: Some(TextDocumentSyncSaveOptions::SaveOptions(SaveOptions {
                    include_text: Some(false),
                })),
                ..Default::default()
            },
        )),
        diagnostic_provider: pull.then(|| {
            DiagnosticServerCapabilities::Options(DiagnosticOptions {
                identifier: Some("turtle".into()),
                inter_file_dependencies: true,
                workspace_diagnostics: false,
                work_done_progress_options: Default::default(),
            })
        }),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![":".into(), "@".into(), "?".into(), "$".into()]),
            resolve_provider: Some(true),
            ..Default::default()
        }),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        definition_provider: Some(OneOf::Left(true)),
        references_provider: Some(OneOf::Left(true)),
        type_definition_provider: Some(TypeDefinitionProviderCapability::Simple(true)),
        implementation_provider: Some(ImplementationProviderCapability::Simple(true)),
        document_highlight_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Right(RenameOptions {
            prepare_provider: Some(true),
            work_done_progress_options: Default::default(),
        })),
        document_symbol_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        folding_range_provider: Some(FoldingRangeProviderCapability::Simple(true)),
        document_formatting_provider: Some(OneOf::Left(true)),
        semantic_tokens_provider: Some(SemanticTokensServerCapabilities::SemanticTokensOptions(
            SemanticTokensOptions {
                legend: ide::Session::semantic_tokens_legend(),
                range: Some(true),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                work_done_progress_options: Default::default(),
            },
        )),
        inlay_hint_provider: Some(OneOf::Left(true)),
        code_lens_provider: Some(CodeLensOptions {
            resolve_provider: Some(true),
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                CodeActionKind::REFACTOR_REWRITE,
            ]),
            resolve_provider: Some(true),
            ..Default::default()
        })),
        execute_command_provider: Some(ExecuteCommandOptions {
            commands: crate::commands::COMMANDS
                .iter()
                .map(|c| c.to_string())
                .collect(),
            work_done_progress_options: Default::default(),
        }),
        // Custom requests, by method: `{"turtle/graph": true}`.
        experimental: Some(serde_json::json!({ GRAPH: true })),
        ..Default::default()
    }
}

use worker::catch as guard;

fn internal_error_diagnostic(msg: &str) -> Diagnostic {
    Diagnostic {
        range: Range::default(),
        severity: Some(DiagnosticSeverity::ERROR),
        source: Some(diagnostics::SOURCE.to_string()),
        message: format!(
            "{msg}. The rest of the server keeps working; please report this file at https://github.com/abhishekkundu25/turtle-lsp/issues"
        ),
        ..Default::default()
    }
}

/// The custom request for the workspace graph (`features::graph`).
const GRAPH: &str = "turtle/graph";

/// A `turtle/graph` answer as JSON text; parameters it cannot answer are
/// `InvalidParams`.
fn graph_json(r: Result<GraphResult, InvalidParams>) -> Result<String, (ErrorCode, String)> {
    match r {
        Ok(g) => json(&g),
        Err(e) => Err((ErrorCode::InvalidParams, e.to_string())),
    }
}

/// `result` as JSON text, for the answer to a request.
fn json(result: &impl serde::Serialize) -> Result<String, (ErrorCode, String)> {
    wire::encode(result).map_err(|e| (ErrorCode::InternalError, e.to_string()))
}

/// The `initialize` result: lsp-types 0.97 has no field for
/// `typeHierarchyProvider`, so the capabilities carry it beside theirs.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct Initialized {
    capabilities: InitializedCapabilities,
    server_info: ServerInfo,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct InitializedCapabilities {
    #[serde(flatten)]
    lsp: ServerCapabilities,
    type_hierarchy_provider: bool,
}

impl Initialized {
    fn new(capabilities: ServerCapabilities) -> Initialized {
        Initialized {
            capabilities: InitializedCapabilities {
                lsp: capabilities,
                type_hierarchy_provider: true,
            },
            server_info: ServerInfo {
                name: "turtle-lsp".into(),
                version: Some(crate::VERSION.into()),
            },
        }
    }
}

fn parse<T: DeserializeOwned>(v: Value) -> Option<T> {
    serde_json::from_value(v).ok()
}

/// The id a `$/cancelRequest` notification cancels.
fn cancel_id(msg: &Message) -> Option<RequestId> {
    match msg {
        Message::Notification(n) if n.method == lsp_types::notification::Cancel::METHOD => {
            serde_json::from_value(n.params.get("id")?.clone()).ok()
        }
        _ => None,
    }
}

impl Server {
    /// How the session ended.
    #[cfg(not(target_arch = "wasm32"))]
    fn main_loop(&mut self) -> Result<Ended, Box<dyn std::error::Error + Send + Sync>> {
        self.start();
        loop {
            let timer = self.timers.next().map_or_else(never, crossbeam_channel::at);
            // What the select wakes for is handled by `guarded`: a bug in one
            // of them is reported and the server goes on with the next.
            let first = crossbeam_channel::select! {
                recv(self.inbox) -> m => match m { Ok(m) => Some(m), Err(_) => return Ok(Ended::ClientGone) },
                recv(self.index.rx()) -> got => {
                    let got = self.index.take(got);
                    self.guarded("the initial index", |s| s.index_arrived(got));
                    None
                }
                recv(self.index.reparsed()) -> done => {
                    if let Ok(done) = done {
                        self.guarded("reindexing", |s| s.reindexed(done));
                    }
                    None
                }
                recv(timer) -> _ => {
                    self.fire_due();
                    None
                }
                recv(self.contexts.rx()) -> got => {
                    // Gone: the loader was replaced, or its threads ended.
                    if let Fed::Got(url) = self.contexts.take(got) {
                        self.guarded("a fetched context", |s| s.context_fetched(&url));
                    }
                    None
                }
                recv(self.imports.results()) -> got => {
                    if let Some(done) = self.imports.received(got) {
                        self.guarded("the imports", |s| s.imports_done(done));
                    }
                    None
                }
                recv(self.wsdiag.due().map_or_else(never, crossbeam_channel::at)) -> _ => {
                    self.guarded("workspace diagnostics", |s| s.wsdiag_start());
                    None
                }
                recv(self.wsdiag.results()) -> chunk => {
                    self.guarded("workspace diagnostics", |s| s.wsdiag_chunk(chunk.ok()));
                    None
                }
                recv(self.queries.results()) -> done => {
                    if let Ok(done) = done {
                        self.guarded("a query run", |s| s.query_done(done));
                    }
                    None
                }
                recv(self.shacl.rx()) -> got => {
                    match self.shacl.take(got) {
                        Fed::Got(o) => self.guarded("SHACL validation", |s| s.shacl_outcome(o)),
                        // The thread is gone.
                        Fed::Gone => self.guarded("SHACL validation", |s| s.shacl_gone()),
                    }
                    None
                }
            };
            if let Some(ended) = self.step(first)? {
                return Ok(ended);
            }
        }
    }

    /// Run `f`; when it panics, the server goes on. The default panic hook
    /// has printed the details to stderr (stdout carries the protocol).
    fn guarded(&mut self, what: &str, f: impl FnOnce(&mut Server)) {
        let _ = guard(what, || f(self));
    }

    /// The initial index arrived, or its thread ended without one.
    fn index_arrived(&mut self, got: Fed<index::Initial>) {
        self.merge_index(match got {
            Fed::Got(files) => files,
            Fed::Gone => Err("turtle-lsp internal error: the indexing thread ended".into()),
        });
    }

    /// The validation thread is gone, or could not start.
    fn shacl_gone(&mut self) {
        self.shacl.gone();
        self.timers.clear(Timer::ShaclProgress);
        self.progress
            .end(&self.client, Work::Shacl, "Validation stopped".into());
    }

    /// Before the first message: register file watchers, show indexing.
    fn start(&mut self) {
        self.register_watchers();
        if let Some(e) = self.layers.take_file_error() {
            self.client.warn(e);
        }
        if !self.index.merged() {
            self.progress
                .begin(&mut self.client, Work::Index, "Indexing workspace");
        }
    }

    /// Handle `first` and every message queued behind it, then publish
    /// diagnostics, resolve imports and ask for refreshes. Some when the
    /// session ended (see [`run`]).
    fn step(
        &mut self,
        first: Option<Message>,
    ) -> Result<Option<Ended>, Box<dyn std::error::Error + Send + Sync>> {
        // Take everything queued, so cancellations are known before any
        // answer is computed and a burst of edits costs one analysis.
        let mut batch: Vec<Message> = first.into_iter().collect();
        batch.extend(self.inbox.try_iter());
        let mut cancelled: FxHashSet<RequestId> = batch.iter().filter_map(cancel_id).collect();
        let mut rest: std::collections::VecDeque<Message> = batch.into();
        while let Some(msg) = rest.pop_front() {
            // A cancel can arrive while earlier messages of the batch are
            // handled: look again just before computing an answer.
            if matches!(msg, Message::Request(_)) {
                for m in self.inbox.try_iter() {
                    cancelled.extend(cancel_id(&m));
                    rest.push_back(m);
                }
            }
            match msg {
                Message::Request(req) if req.method == lsp_types::request::Shutdown::METHOD => {
                    if self.spawn == Spawn::Inline {
                        // The host ends the session; nothing waits for `exit`.
                        self.client.send(Response::new_ok(req.id, ()))?;
                        return Ok(Some(Ended::Exit));
                    }
                    return self.shutdown(req, rest.into_iter(), EXIT_WAIT).map(Some);
                }
                Message::Notification(n) if n.method == lsp_types::notification::Exit::METHOD => {
                    return Ok(Some(Ended::ExitWithoutShutdown));
                }
                Message::Request(req) if cancelled.contains(&req.id) => {
                    let resp = Response::new_err(
                        req.id,
                        ErrorCode::RequestCanceled as i32,
                        "request cancelled".into(),
                    );
                    self.client.send(resp)?;
                }
                msg => self.handle(msg)?,
            }
        }
        // Past the messages, each phase runs on its own: the documents it
        // touches are analyzed here, outside any request, and a bug in one
        // must not end the session or skip the phases after it.
        #[cfg(test)]
        self.guarded("a test", |s| {
            if std::mem::take(&mut s.fault) {
                panic!("injected fault after a batch");
            }
        });
        self.guarded("the described set", |s| s.check_described());
        self.guarded("diagnostics", |s| s.publish_dirty());
        self.guarded("the incremental check", |s| s.log_parses());
        self.guarded("the imports", |s| s.sync_imports());
        self.guarded("the imports", |s| s.track_imports_progress());
        self.guarded("refreshes", |s| s.refresh_if_needed());
        Ok(None)
    }

    /// The earliest timer pending, if any.
    fn next_due(&self) -> Option<Instant> {
        self.timers.next()
    }

    /// Run the timers due by now, as the main loop does when they fire.
    fn fire_due(&mut self) {
        for timer in self.timers.take_due(Instant::now()) {
            self.guarded("a timer", |s| match timer {
                Timer::Described => s.described_settled(),
                Timer::ShaclTyping => s.validate_shacl(),
                Timer::ShaclProgress => {
                    s.progress
                        .begin(&mut s.client, Work::Shacl, "Validating SHACL shapes");
                }
                Timer::QueryProgress => s.query_progress_tick(),
            });
        }
    }

    /// An edit that changes which terms are described (or which prefixes are
    /// bound) changes other open documents' lint results; most keystrokes don't.
    ///
    /// Typing a new subject changes the described set on every keystroke, so
    /// other documents (and the client's semantic tokens and inlay hints) are
    /// refreshed once typing pauses, not per keystroke; the edited document
    /// itself is re-diagnosed right away as usual.
    fn check_described(&mut self) {
        // Analyzes the documents edited in this batch (diagnostics would anyway).
        let g = self.ws.knowledge().described_generation();
        if self.refresh.described_moved(g) {
            self.timers
                .set(Timer::Described, Instant::now() + DESCRIBED_DEBOUNCE);
        }
    }

    /// The pause after a described-set change has passed.
    fn described_settled(&mut self) {
        let g = self.ws.knowledge().described_generation();
        if self.refresh.described_settled(g) {
            self.knowledge_changed();
        }
    }

    /// Answer `shutdown`, then wait up to `exit_wait` for `exit`; other
    /// requests are refused.
    fn shutdown(
        &self,
        req: Request,
        rest: impl Iterator<Item = Message>,
        exit_wait: Duration,
    ) -> Result<Ended, Box<dyn std::error::Error + Send + Sync>> {
        self.queries.cancel_all();
        self.client.send(Response::new_ok(req.id, ()))?;
        let mut rest = rest;
        loop {
            let msg = match rest.next() {
                Some(m) => m,
                None => match self.inbox.recv_timeout(exit_wait) {
                    Ok(m) => m,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                        return Ok(Ended::ShutdownTimedOut);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                        return Ok(Ended::ClientGone);
                    }
                },
            };
            match msg {
                Message::Notification(n) if n.method == lsp_types::notification::Exit::METHOD => {
                    return Ok(Ended::Exit);
                }
                Message::Request(r) => {
                    let resp = Response::new_err(
                        r.id,
                        ErrorCode::InvalidRequest as i32,
                        "the server is shutting down".into(),
                    );
                    self.client.send(resp)?;
                }
                _ => {}
            }
        }
    }

    fn handle(&mut self, msg: Message) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match msg {
            Message::Request(req) => {
                // Requests see the current text, so pending diagnostics go out first.
                self.publish_dirty();
                self.ensure_indexed();
                let id = req.id.clone();
                let method = req.method.clone();
                // A bug in one feature must not take the server down for the session.
                let result = guard(&method, || self.request(req))
                    .unwrap_or_else(|msg| Err((ErrorCode::InternalError, msg)));
                match result {
                    // Answered later (a query run).
                    Ok(None) => {}
                    Ok(Some(json)) => self.client.respond(id, json)?,
                    Err((code, message)) => {
                        self.client
                            .send(Response::new_err(id, code as i32, message))?;
                    }
                }
            }
            Message::Notification(n) => {
                let method = n.method.clone();
                let _ = guard(&method, || self.notification(n));
            }
            Message::Response(r) => {
                self.progress
                    .accepted(&self.client, &r.id, r.response_result.is_ok());
                if self.layers.config_answered(&r.id) {
                    // One item per requested section: [turtle].
                    if let Some(section) = r.response_result.ok().and_then(|v| v.get(0).cloned()) {
                        let _ = guard("workspace/configuration", || self.set_config(section));
                    }
                }
            }
        }
        Ok(())
    }

    fn notification(&mut self, n: Notification) {
        use lsp_types::notification::*;
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Some(p) = parse::<DidOpenTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    let doc = p.text_document;
                    if doc.uri.as_str() == uri {
                        self.spelling.remove(&uri);
                    } else {
                        self.spelling.insert(uri.clone(), doc.uri.to_string());
                    }
                    let kind = DocKind::detect(Some(&doc.language_id), &uri);
                    let fx = self
                        .ws
                        .open_doc_as(uri.clone(), doc.text, doc.version, kind);
                    self.wsdiag_opened(&uri);
                    self.after_edit(fx);
                    self.workspace_changed_inline();
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Some(p) = parse::<DidChangeTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    let fx = self.ws.change_doc(
                        &uri,
                        p.text_document.version,
                        p.content_changes,
                        self.enc,
                    );
                    self.after_edit(fx);
                    // SHACL results stay until typing pauses (or the next save).
                    if self.layers.settings().shacl_on_type {
                        self.timers
                            .set(Timer::ShaclTyping, Instant::now() + SHACL_TYPING_DEBOUNCE);
                    }
                }
            }
            DidSaveTextDocument::METHOD => {
                if let Some(p) = parse::<DidSaveTextDocumentParams>(n.params) {
                    self.on_save(&normalize_uri(p.text_document.uri.as_str()));
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Some(p) = parse::<DidCloseTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    let fx = self.ws.close_doc(&uri);
                    self.dirty.remove(&uri);
                    self.apply(fx);
                    // Clear diagnostics for the closed buffer, unless it goes
                    // back to the workspace diagnostics.
                    if !self.wsdiag_closed(&uri) && !self.refresh.pull() {
                        self.send_diagnostics(&uri, vec![], None);
                    }
                    self.spelling.remove(&uri);
                    self.workspace_changed_inline();
                }
            }
            DidChangeConfiguration::METHOD => {
                let settings = n.params.get("settings").unwrap_or(&Value::Null);
                match settings.get("turtle") {
                    Some(section) => self.set_config(section.clone()),
                    // Clients that only signal "something changed" are asked.
                    None => self.layers.request_config(&mut self.client),
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Some(p) = parse::<DidChangeWatchedFilesParams>(n.params) {
                    self.watched_files_changed(p.changes);
                }
            }
            Cancel::METHOD => {
                if let Some(p) = parse::<CancelParams>(n.params) {
                    let id = match p.id {
                        NumberOrString::Number(n) => RequestId::from(n),
                        NumberOrString::String(s) => RequestId::from(s),
                    };
                    self.query_cancel_request(&id);
                }
            }
            WorkDoneProgressCancel::METHOD => {
                if let Some(p) = parse::<WorkDoneProgressCancelParams>(n.params)
                    && let NumberOrString::String(token) = p.token
                {
                    self.query_progress_cancelled(&token);
                }
            }
            _ => {}
        }
    }

    /// Embedded, the open documents are the whole workspace (there is no
    /// disk to index): opening or closing one changes the data SHACL
    /// validates, so validate once the host pauses, as after typing.
    fn workspace_changed_inline(&mut self) {
        if self.spawn == Spawn::Inline {
            self.timers
                .set(Timer::ShaclTyping, Instant::now() + SHACL_TYPING_DEBOUNCE);
        }
    }

    /// Files changed on disk, as the client's watchers report. The settings
    /// file and the imports sources (catalogs, imported RDF/XML files, which
    /// count wherever they are) are the server's; the workspace says what
    /// the rest means ([`Workspace::files_changed`]).
    fn watched_files_changed(&mut self, changes: Vec<FileEvent>) {
        let settings_file = self.ws.roots().first().map(|r| r.join(SETTINGS_FILE));
        let mut files: Vec<(PathBuf, Change)> = Vec::new();
        for ch in changes {
            let Some(path) = uri_to_path(ch.uri.as_str()) else {
                continue;
            };
            if settings_file.as_ref() == Some(&path) {
                self.reload_settings_file();
                continue;
            }
            if is_import_source(&path) {
                // A catalog or an imported RDF/XML ontology changed.
                self.imports.retry();
            }
            let change = match ch.typ {
                FileChangeType::CREATED => Change::Created,
                FileChangeType::DELETED => Change::Deleted,
                _ => Change::Changed,
            };
            files.push((path, change));
        }
        let fx = self.ws.files_changed(&files);
        self.apply(fx);
    }

    /// Parse indexed files again (their prefixes changed), on a thread of
    /// their own so that the main loop goes on; embedded, where there are
    /// no threads, at once. The result is merged by [`Server::reindexed`].
    fn reindex_in_background(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let jobs = self.ws.prefix_jobs(paths);
        let reader = self.ws.reader();
        #[cfg(not(target_arch = "wasm32"))]
        if self.spawn == Spawn::Thread {
            let (tx, failed) = (self.index.reparsed_sender(), self.index.reparsed_sender());
            let started = self.spawn.run(
                "reindex",
                WORKER_STACK,
                move || reader.parse(jobs),
                move |files| {
                    let _ = tx.send(files);
                },
            );
            if let Err(e) = started {
                let _ = failed.send(Err(format!("turtle-lsp could not start a thread: {e}")));
            }
            return;
        }
        let files = reader.parse(jobs);
        self.reindexed(Ok(files));
    }

    /// Files parsed again after a prefixes.ttl changed.
    fn reindexed(&mut self, done: Result<Vec<crate::workspace::Reparsed>, String>) {
        match done {
            Ok(files) => {
                if self.ws.add_reparsed(files) > 0 {
                    self.apply(Effects {
                        shacl: true,
                        ..Effects::knowledge()
                    });
                }
            }
            Err(msg) => self.client.warn(msg),
        }
    }

    /// A document was saved: a catalog or ontology file may have changed too.
    fn on_save(&mut self, uri: &str) {
        let fx = self.ws.save_doc(uri);
        self.imports.retry();
        self.timers.clear(Timer::ShaclTyping);
        self.apply(fx);
    }

    /// Start resolving `owl:imports` if they changed (never before the
    /// initial index, which may describe the imported ontologies).
    fn sync_imports(&mut self) {
        if !self.index.merged() {
            return;
        }
        let roots = self.ws.roots().to_vec();
        let network = self.layers.settings().imports_network;
        let k = self.ws.knowledge();
        if let Some(done) = self.imports.sync(k, &roots, network) {
            self.imports_done(done);
        }
    }

    /// A JSON-LD context fetched in the background arrived (or failed):
    /// the documents waiting for it are analyzed again.
    #[cfg(not(target_arch = "wasm32"))]
    fn context_fetched(&mut self, url: &str) {
        self.ensure_indexed();
        let fx = self.ws.context_fetched(url);
        self.apply(fx);
    }

    fn imports_done(&mut self, done: crate::imports::Done) {
        if self.imports.apply(done, self.ws.knowledge_mut()) {
            self.knowledge_changed();
            // Loaded ontologies are data for SHACL, and unresolved imports
            // soften its results.
            self.validate_shacl();
        }
        self.track_imports_progress();
    }

    /// Show progress while a resolution pass is running.
    fn track_imports_progress(&mut self) {
        match (self.imports.busy(), self.progress.is_shown(Work::Imports)) {
            (true, false) => {
                self.progress
                    .begin(&mut self.client, Work::Imports, "Resolving owl:imports");
            }
            (false, true) => {
                let n = self.imports.unresolved().len();
                let msg = match n {
                    0 => "All imports resolved".to_string(),
                    1 => "1 import unresolved".to_string(),
                    n => format!("{n} imports unresolved"),
                };
                self.progress.end(&self.client, Work::Imports, msg);
            }
            _ => {}
        }
    }

    /// The client's `turtle` configuration section changed (see [`Layers`]).
    fn set_config(&mut self, section: Value) {
        if self.layers.set_config(section) {
            self.relayer_settings();
        }
    }

    /// `turtle-lsp.json` changed on disk: read it again. An invalid file
    /// warns and counts as absent.
    fn reload_settings_file(&mut self) {
        if let Some(e) = self.layers.reload_file(self.ws.roots().first()) {
            self.client.warn(e);
        }
        self.relayer_settings();
    }

    /// Recompute the settings from their layers and act on any change.
    fn relayer_settings(&mut self) {
        let Some(change) = self.layers.relayer() else {
            return;
        };
        // Any setting can change every document's diagnostics, and SHACL's.
        let mut fx = Effects {
            shacl: true,
            ..Effects::knowledge()
        };
        if change.prefixes() {
            // The background index was built with the old prefixes.
            self.ensure_indexed();
            fx.add(
                self.ws
                    .set_settings_prefixes(change.settings().prefixes.clone()),
            );
        }
        if change.network() {
            // Indexed JSON-LD files are parsed again with the new loader.
            self.ensure_indexed();
            let network = change.settings().imports_network;
            self.contexts = use_network(&mut self.ws, network, self.spawn);
        }
        self.layers.commit(change);
        self.wsdiag_setting();
        self.ws
            .set_incremental(incremental(self.layers.settings(), self.spawn));
        self.apply(fx);
    }

    /// Something outside any one document changed that can change its
    /// diagnostics (see [`Server::apply`]).
    fn knowledge_changed(&mut self) {
        self.apply(Effects::knowledge());
    }

    /// Act on what a change to the workspace means beyond itself.
    fn apply(&mut self, fx: Effects) {
        let Effects {
            dirty,
            knowledge,
            shacl,
            reparse,
            on_disk,
            closed_files,
        } = fx;
        // Diagnostics read the knowledge index (add-prefix candidates, lints),
        // so with knowledge every open document is due again.
        if knowledge || dirty == Dirty::AllOpen {
            self.dirty.extend(self.ws.open_uris().map(str::to_owned));
        }
        if let Dirty::One(uri) = dirty {
            self.dirty.insert(uri);
        }
        self.wsdiag_disk_changed(&on_disk);
        self.reindex_in_background(reparse);
        if knowledge {
            self.refresh.knowledge_changed();
        }
        // And the diagnostics of every closed file.
        if knowledge || closed_files {
            self.wsdiag.invalidate_all();
        }
        if shacl {
            self.validate_shacl();
        }
    }

    fn refresh_if_needed(&mut self) {
        self.refresh.send(&mut self.client);
    }

    /// The initial index arrived (or failed, with the reason). Either way
    /// the server goes on: documents that are open are analyzed as ever.
    fn merge_index(&mut self, files: index::Initial) {
        let files = match files {
            Ok(files) => {
                self.progress.end(
                    &self.client,
                    Work::Index,
                    match files.len() {
                        1 => "1 file".to_string(),
                        n => format!("{n} files"),
                    },
                );
                files
            }
            Err(msg) => {
                self.progress
                    .end(&self.client, Work::Index, "Indexing failed".into());
                self.client.warn(format!(
                    "{msg}. The workspace's files are not indexed; the documents you open still work."
                ));
                Vec::new()
            }
        };
        self.ws.merge_initial_index(files);
        self.index.set_merged();
        self.apply(Effects {
            shacl: true,
            ..Effects::knowledge()
        });
    }

    /// Answers that look across the workspace (definition, references, rename,
    /// ...) must not come from a half-built index: a rename sent right after
    /// start-up would silently miss files. Wait for the initial index once.
    fn ensure_indexed(&mut self) {
        if !self.index.merged() {
            let got = self.index.wait();
            self.index_arrived(got);
        }
    }

    /// A document was opened or edited: act on what that means, and let
    /// workspace diagnostics make way for the edit.
    fn after_edit(&mut self, fx: Effects) {
        self.apply(fx);
        self.wsdiag.yield_to_edit();
    }

    /// Diagnostics for `uri`, or the panic message if computing them failed.
    fn diagnostics_for(&mut self, uri: &str) -> Result<Option<Vec<Diagnostic>>, String> {
        let cached_import = self.ws.is_cached_import(uri);
        guard("diagnostics", || {
            let status = |iri: &str| self.imports.statuses().get(iri);
            let inputs = DiagnosticInputs {
                import_status: &status,
                shacl: self.shacl.diagnostics().get(uri).map_or(&[], Vec::as_slice),
                indexed: self.index.merged(),
                cached_import,
                ..DiagnosticInputs::new(self.layers.settings())
            };
            // Not `self.session()`: `inputs` borrows other fields of `self`.
            let settings = self.layers.settings();
            let mut s = ide::Session::new(&mut self.ws, self.enc, settings, self.resolve_edits);
            s.diagnostics(uri, &inputs)
        })
        // A cached copy of an import is read-only: nothing to fix there.
        .map(|d| d.or_else(|| cached_import.then(Vec::new)))
    }

    /// With [`CHECK_INCREMENTAL`]: log how each analysis since the last
    /// call was parsed, and, as an error, a splice that differed from a full
    /// parse.
    fn log_parses(&mut self) {
        for r in self.ws.take_parse_log() {
            let (typ, message) = match r.difference {
                Some(d) => (
                    MessageType::ERROR,
                    format!("incremental: {} differs from a full parse: {d}", r.uri),
                ),
                None => (
                    MessageType::LOG,
                    format!("incremental: {} {:?}", r.uri, r.path),
                ),
            };
            let params = LogMessageParams { typ, message };
            self.client
                .notify(lsp_types::notification::LogMessage::METHOD, params);
        }
    }

    fn publish_dirty(&mut self) {
        let dirty: Vec<String> = self.dirty.drain().collect();
        if self.refresh.pull() {
            return;
        }
        for uri in dirty {
            let version = self.ws.version(&uri);
            match self.diagnostics_for(&uri) {
                Ok(Some(d)) => self.send_diagnostics(&uri, d, version),
                Ok(None) => {}
                // Tell the user instead of silently showing nothing.
                Err(msg) => {
                    self.send_diagnostics(&uri, vec![internal_error_diagnostic(&msg)], version)
                }
            }
        }
    }

    /// `textDocument/diagnostic`: a full report, or `unchanged` when neither
    /// the document nor the knowledge generation moved since `previousResultId`.
    fn pull_diagnostics(&mut self, p: DocumentDiagnosticParams) -> DocumentDiagnosticReportResult {
        let uri = normalize_uri(p.text_document.uri.as_str());
        let version = self
            .ws
            .version(&uri)
            .map_or_else(|| "disk".to_string(), |v| v.to_string());
        let described = self.ws.knowledge().described_generation();
        let mut result_id = Some(self.refresh.report_id(&version, described));
        if p.previous_result_id.is_some() && p.previous_result_id == result_id {
            return DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
                related_documents: None,
                unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport {
                    result_id: result_id.unwrap_or_default(),
                },
            })
            .into();
        }
        let items = match self.diagnostics_for(&uri) {
            Ok(d) => d.unwrap_or_default(),
            Err(msg) => {
                // Not cached: the next pull tries again.
                result_id = None;
                vec![internal_error_diagnostic(&msg)]
            }
        };
        DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport { result_id, items },
        })
        .into()
    }

    fn send_diagnostics(&self, uri: &str, diagnostics: Vec<Diagnostic>, version: Option<i32>) {
        let params = PublishDiagnosticsParams {
            uri: navigation::uri(self.spelling.get(uri).map_or(uri, String::as_str)),
            diagnostics,
            version,
        };
        self.client
            .notify(lsp_types::notification::PublishDiagnostics::METHOD, params);
    }

    fn register_watchers(&mut self) {
        if !self.watch_dynamic {
            return;
        }
        let opts = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![
                FileSystemWatcher {
                    glob_pattern: GlobPattern::String(WATCHED_FILES.into()),
                    kind: None,
                },
                // A deleted or renamed folder takes the files below it along,
                // and no file glob reports that: ask for every deletion.
                FileSystemWatcher {
                    glob_pattern: GlobPattern::String("**/*".into()),
                    kind: Some(WatchKind::Delete),
                },
            ],
        };
        let params = RegistrationParams {
            registrations: vec![Registration {
                id: "turtle-watch".into(),
                method: lsp_types::notification::DidChangeWatchedFiles::METHOD.into(),
                register_options: serde_json::to_value(opts).ok(),
            }],
        };
        self.client
            .request(lsp_types::request::RegisterCapability::METHOD, params);
    }

    /// The editor session over this server's workspace, for one request.
    fn session(&mut self) -> ide::Session<'_> {
        ide::Session::new(
            &mut self.ws,
            self.enc,
            self.layers.settings(),
            self.resolve_edits,
        )
    }

    /// The answer to `req`, its result as JSON text: None when it is
    /// answered later (a query run).
    fn request(&mut self, req: Request) -> Result<Option<String>, (ErrorCode, String)> {
        use lsp_types::request::*;
        let bad = |e: serde_json::Error| (ErrorCode::InvalidParams, e.to_string());
        macro_rules! params {
            ($t:ty) => {
                serde_json::from_value::<$t>(req.params.clone()).map_err(bad)?
            };
        }

        let answer = match req.method.as_str() {
            Completion::METHOD => {
                let p = params!(CompletionParams);
                json(&self.session().completion(&p))
            }
            ResolveCompletionItem::METHOD => {
                let item = params!(CompletionItem);
                json(&self.session().resolve_completion(item))
            }
            HoverRequest::METHOD => {
                let p = params!(HoverParams);
                let at = p.text_document_position_params;
                json(
                    &self
                        .session()
                        .hover(at.text_document.uri.as_str(), at.position),
                )
            }
            GotoDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let at = p.text_document_position_params;
                let locs = self
                    .session()
                    .definition(at.text_document.uri.as_str(), at.position);
                json(&locs.map(GotoDefinitionResponse::Array))
            }
            GotoTypeDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let at = p.text_document_position_params;
                let locs = self
                    .session()
                    .type_definition(at.text_document.uri.as_str(), at.position);
                json(&locs.map(GotoDefinitionResponse::Array))
            }
            GotoImplementation::METHOD => {
                let p = params!(GotoDefinitionParams);
                let at = p.text_document_position_params;
                let locs = self
                    .session()
                    .implementations(at.text_document.uri.as_str(), at.position);
                json(&locs.map(GotoDefinitionResponse::Array))
            }
            References::METHOD => {
                let p = params!(ReferenceParams);
                let at = p.text_document_position;
                json(&self.session().references(
                    at.text_document.uri.as_str(),
                    at.position,
                    p.context.include_declaration,
                ))
            }
            DocumentHighlightRequest::METHOD => {
                let p = params!(DocumentHighlightParams);
                let at = p.text_document_position_params;
                json(
                    &self
                        .session()
                        .highlights(at.text_document.uri.as_str(), at.position),
                )
            }
            PrepareRenameRequest::METHOD => {
                let p = params!(TextDocumentPositionParams);
                match self
                    .session()
                    .prepare_rename(p.text_document.uri.as_str(), p.position)
                {
                    Ok(r) => json(&r),
                    Err(msg) => Err((ErrorCode::InvalidParams, msg)),
                }
            }
            Rename::METHOD => {
                let p = params!(RenameParams);
                let at = p.text_document_position;
                match self
                    .session()
                    .rename(at.text_document.uri.as_str(), at.position, &p.new_name)
                {
                    Ok(edit) => json(&edit),
                    Err(msg) => Err((ErrorCode::InvalidParams, msg)),
                }
            }
            DocumentSymbolRequest::METHOD => {
                let p = params!(DocumentSymbolParams);
                let syms = self
                    .session()
                    .document_symbols(p.text_document.uri.as_str());
                json(&syms.map(DocumentSymbolResponse::Nested))
            }
            WorkspaceSymbolRequest::METHOD => {
                let p = params!(WorkspaceSymbolParams);
                json(&WorkspaceSymbolResponse::Flat(
                    self.session().workspace_symbols(&p.query),
                ))
            }
            FoldingRangeRequest::METHOD => {
                let p = params!(FoldingRangeParams);
                json(&self.session().folding_ranges(p.text_document.uri.as_str()))
            }
            DocumentDiagnosticRequest::METHOD => {
                let p = params!(DocumentDiagnosticParams);
                json(&self.pull_diagnostics(p))
            }
            CodeActionRequest::METHOD => {
                let p = params!(CodeActionParams);
                json(&self.session().code_actions(&p))
            }
            ExecuteCommand::METHOD => {
                match crate::commands::parse(params!(ExecuteCommandParams))? {
                    // The job starts when this batch ends; its answer
                    // republishes diagnostics like any other resolution pass.
                    crate::commands::Command::FetchImport { iri } => {
                        self.imports.fetch(&iri);
                        json(&Value::Null)
                    }
                    // Answered when the query is done (see `run_query`).
                    crate::commands::Command::RunQuery { uri } => {
                        match self.run_query(req.id.clone(), &uri)? {
                            Some(v) => json(&v),
                            None => return Ok(None),
                        }
                    }
                    crate::commands::Command::CancelQuery { uri } => {
                        self.cancel_query(&uri);
                        json(&Value::Null)
                    }
                }
            }
            CodeActionResolveRequest::METHOD => {
                let action = params!(CodeAction);
                json(&self.session().resolve_code_action(action))
            }
            Formatting::METHOD => {
                let p = params!(DocumentFormattingParams);
                json(
                    &self
                        .session()
                        .formatting(p.text_document.uri.as_str(), &p.options),
                )
            }
            TypeHierarchyPrepare::METHOD => {
                let p = params!(TypeHierarchyPrepareParams);
                let at = p.text_document_position_params;
                json(
                    &self
                        .session()
                        .prepare_type_hierarchy(at.text_document.uri.as_str(), at.position),
                )
            }
            TypeHierarchySupertypes::METHOD | TypeHierarchySubtypes::METHOD => {
                let item = params!(TypeHierarchySupertypesParams).item;
                let mut s = self.session();
                json(&if req.method == TypeHierarchySupertypes::METHOD {
                    s.supertypes(&item)
                } else {
                    s.subtypes(&item)
                })
            }
            SemanticTokensFullRequest::METHOD => {
                let p = params!(SemanticTokensParams);
                json(
                    &self
                        .session()
                        .semantic_tokens_full(p.text_document.uri.as_str()),
                )
            }
            SemanticTokensRangeRequest::METHOD => {
                let p = params!(SemanticTokensRangeParams);
                json(
                    &self
                        .session()
                        .semantic_tokens_range(p.text_document.uri.as_str(), p.range),
                )
            }
            InlayHintRequest::METHOD => {
                let p = params!(InlayHintParams);
                json(
                    &self
                        .session()
                        .inlay_hints(p.text_document.uri.as_str(), p.range),
                )
            }
            CodeLensRequest::METHOD => {
                let p = params!(CodeLensParams);
                let uri = p.text_document.uri.as_str();
                let mut lenses = self.session().code_lenses(uri);
                if let Some(l) = &mut lenses {
                    self.query_lenses(uri, l);
                }
                json(&lenses)
            }
            CodeLensResolve::METHOD => {
                let lens = params!(CodeLens);
                json(&self.session().resolve_code_lens(lens))
            }
            GRAPH => {
                let p = match req.params {
                    Value::Null => GraphParams::default(),
                    _ => params!(GraphParams),
                };
                // A big layout takes seconds: on a thread of its own, which
                // answers the request itself.
                #[cfg(not(target_arch = "wasm32"))]
                if self.spawn == Spawn::Thread {
                    self.graph_on_thread(req.id.clone(), p)?;
                    return Ok(None);
                }
                graph_json(self.session().graph(p))
            }
            _ => Err((
                ErrorCode::MethodNotFound,
                format!("unhandled method {}", req.method),
            )),
        };
        answer.map(Some)
    }

    /// Answer `turtle/graph` request `id` from a thread of its own, over a
    /// copy of the knowledge index (`Session::graph_job`): the main loop goes
    /// on meanwhile. Nothing is kept: a `$/cancelRequest` does not stop it,
    /// and the answer comes when the layout ends.
    #[cfg(not(target_arch = "wasm32"))]
    fn graph_on_thread(
        &mut self,
        id: RequestId,
        params: GraphParams,
    ) -> Result<(), (ErrorCode, String)> {
        let job = self.session().graph_job(params);
        let out = self.client.sender();
        self.spawn
            .run(
                "graph",
                WORKER_STACK,
                move || graph_json(job.run()),
                move |answer| {
                    let msg = match answer.unwrap_or_else(|e| Err((ErrorCode::InternalError, e))) {
                        Ok(json) => Out::Result { id, json },
                        Err((code, message)) => {
                            Out::Message(Response::new_err(id, code as i32, message).into())
                        }
                    };
                    let _ = out.send(msg);
                },
            )
            .map_err(|e| {
                (
                    ErrorCode::InternalError,
                    format!("turtle-lsp could not start a thread for the graph: {e}"),
                )
            })
    }

    /// Start validating the workspace in the background (once indexed, when
    /// enabled and some workspace document has shapes); otherwise clear the
    /// results shown.
    fn validate_shacl(&mut self) {
        if !self.index.merged() {
            return;
        }
        let docs: Vec<_> = self
            .ws
            .knowledge()
            .docs()
            .map(|(_, s, a)| (s, a.clone()))
            .collect();
        let imports = &self.imports;
        let job = self.shacl.job(docs, self.layers.settings(), self.enc, || {
            imports.unresolved()
        });
        let Some(job) = job else {
            self.timers.clear(Timer::ShaclProgress);
            self.progress
                .end(&self.client, Work::Shacl, "Validation off".into());
            if self.shacl.clear() {
                self.knowledge_changed();
            }
            return;
        };
        if self.spawn == Spawn::Inline {
            let outcome = shacl::run_inline(&job);
            self.shacl_outcome(outcome);
            return;
        }
        // A superseded run counts as the same validation going on.
        if self.progress.supported()
            && !self.progress.is_shown(Work::Shacl)
            && !self.timers.is_set(Timer::ShaclProgress)
        {
            self.timers
                .set(Timer::ShaclProgress, Instant::now() + SHACL_PROGRESS_DELAY);
        }
        if let Err(e) = self.shacl.submit(job) {
            self.client.warn(format!(
                "turtle-lsp could not start its SHACL validation thread: {e}"
            ));
            self.shacl_gone();
        }
    }

    fn shacl_outcome(&mut self, o: shacl_validate::Outcome) {
        if !self.shacl.is_current(&o) {
            return;
        }
        self.timers.clear(Timer::ShaclProgress);
        if self.progress.is_shown(Work::Shacl) {
            let summary = shacl::violation_summary(&o.diagnostics);
            self.progress.end(&self.client, Work::Shacl, summary);
        }
        if self.shacl.set_results(o.diagnostics) {
            self.knowledge_changed();
        }
    }
}

/// A server driven by its host instead of its own loop, for a host with no
/// threads and no stdio (the browser build, web/engine). The host hands over
/// messages and the server takes one step over them, returning what it sent;
/// the host calls [`Embedded::tick`] when [`Embedded::due_in_ms`] says a timer
/// is due. The workspace has no folders: it is the documents the host opens,
/// indexed from the start. SHACL validation and imports run inline, and no
/// import is loaded.
pub struct Embedded {
    server: Server,
    inbox: crossbeam_channel::Sender<Message>,
    outbox: wire::Outbox,
    ended: bool,
}

impl Embedded {
    /// A server for `initialize` params `params`, with what it sent: the
    /// initialize response first (to request id 0).
    pub fn start(params: Value) -> Result<(Embedded, Vec<Message>), String> {
        let (sender, outbox) = wire::memory();
        let (mut server, result, _roots) =
            Server::configure(sender, params, Spawn::Inline).map_err(|e| e.to_string())?;
        let (inbox, inbox_rx) = unbounded();
        server.inbox = inbox_rx;
        server
            .client
            .respond(RequestId::from(0), result)
            .map_err(|e| e.to_string())?;
        server.start();
        server.merge_index(Ok(Vec::new()));
        let mut embedded = Embedded {
            server,
            inbox,
            outbox,
            ended: false,
        };
        let sent = embedded.sent();
        Ok((embedded, sent))
    }

    /// Hand over `messages` and take one step; what the server sent.
    pub fn handle(&mut self, messages: Vec<Message>) -> Vec<Message> {
        if self.ended {
            return Vec::new();
        }
        for m in messages {
            let _ = self.inbox.send(m);
        }
        let first = self.server.inbox.try_recv().ok();
        self.step(first)
    }

    /// Run the timers that are due, then a step; what the server sent.
    pub fn tick(&mut self) -> Vec<Message> {
        if self.ended {
            return Vec::new();
        }
        self.server.fire_due();
        self.step(None)
    }

    /// Milliseconds until the next timer, if one is pending.
    pub fn due_in_ms(&self) -> Option<f64> {
        let due = self.server.next_due()?;
        Some(due.saturating_duration_since(Instant::now()).as_secs_f64() * 1000.0)
    }

    fn step(&mut self, first: Option<Message>) -> Vec<Message> {
        match self.server.step(first) {
            Ok(Some(_)) | Err(_) => self.ended = true,
            Ok(None) => {}
        }
        self.sent()
    }

    fn sent(&mut self) -> Vec<Message> {
        self.outbox.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{PANIC_IN_ANALYSIS, PANIC_IN_FACTS, path_to_uri};
    use serde_json::json;

    const TTL: &str = "@prefix ex: <http://e.org/> .\nex:A a ex:B .\n";

    /// Silence panic messages while `f` runs: the panics are the point. The
    /// guard is shared with the other tests that silence panics, and
    /// restores the hook even when `f` panics.
    fn quietly<T>(f: impl FnOnce() -> T) -> T {
        let _quiet = crate::workspace::tests::QuietPanics::new();
        f()
    }

    fn embedded() -> Embedded {
        Embedded::start(json!({"processId": null, "rootUri": null, "capabilities": {}}))
            .unwrap()
            .0
    }

    fn notify(method: &str, params: Value) -> Message {
        Message::Notification(Notification::new(method.into(), params))
    }

    fn open(e: &mut Embedded, uri: &str, text: &str) -> Vec<Message> {
        e.handle(vec![notify(
            "textDocument/didOpen",
            json!({"textDocument": {"uri": uri, "languageId": "turtle", "version": 1, "text": text}}),
        )])
    }

    fn ask(e: &mut Embedded, id: i32, method: &str, params: Value) -> Value {
        let sent = e.handle(vec![Message::Request(Request::new(
            id.into(),
            method.into(),
            params,
        ))]);
        sent.into_iter()
            .find_map(|m| match m {
                Message::Response(r) if r.id == id.into() => Some(match r.response_result {
                    Err(e) => json!({"error": e.message}),
                    Ok(v) => v,
                }),
                _ => None,
            })
            .expect("a response")
    }

    /// The messages of the diagnostics published for `uri`.
    fn published(sent: &[Message], uri: &str) -> Option<Vec<String>> {
        sent.iter().rev().find_map(|m| match m {
            Message::Notification(n) if n.method == "textDocument/publishDiagnostics" => {
                (n.params["uri"] == uri).then(|| {
                    n.params["diagnostics"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|d| d["message"].as_str().unwrap().to_string())
                        .collect()
                })
            }
            _ => None,
        })
    }

    fn symbols(e: &mut Embedded, id: i32, uri: &str) -> usize {
        let r = ask(
            e,
            id,
            "textDocument/documentSymbol",
            json!({"textDocument": {"uri": uri}}),
        );
        r.as_array().unwrap_or_else(|| panic!("{r}")).len()
    }

    #[test]
    fn a_panic_after_a_batch_does_not_end_the_session_or_skip_the_phases_after_it() {
        let mut e = embedded();
        e.server.fault = true;
        let sent = quietly(|| open(&mut e, "file:///w/a.ttl", "ex:s ex:p ex:o .\n"));
        assert!(!e.ended);
        let shown = published(&sent, "file:///w/a.ttl").expect("diagnostics still went out");
        assert!(shown.iter().any(|m| m.contains("ex")), "{shown:?}");
        assert!(!e.server.fault, "the fault fired");
        symbols(&mut e, 2, "file:///w/a.ttl");
    }

    #[test]
    fn a_document_whose_analysis_panics_shows_an_internal_error_and_the_server_goes_on() {
        for marker in [PANIC_IN_ANALYSIS, PANIC_IN_FACTS] {
            let mut e = embedded();
            let (bad, good) = ("file:///w/bad.ttl", "file:///w/good.ttl");
            let sent = quietly(|| {
                open(&mut e, good, TTL);
                open(&mut e, bad, &format!("# {marker}\n{TTL}"))
            });
            assert!(!e.ended, "{marker}");
            let shown = published(&sent, bad).expect("the bad document gets its diagnostics");
            assert!(
                shown
                    .iter()
                    .any(|m| m.contains("internal error") && m.contains("report")),
                "{marker}: {shown:?}"
            );
            // Every other document and request still works.
            assert!(symbols(&mut e, 2, good) > 0, "{marker}");
            let sent = quietly(|| {
                e.handle(vec![notify(
                    "textDocument/didChange",
                    json!({"textDocument": {"uri": good, "version": 2},
                           "contentChanges": [{"text": format!("# {marker}\n")}]}),
                )])
            });
            let shown = published(&sent, good).unwrap();
            assert!(
                shown.iter().any(|m| m.contains("internal error")),
                "{marker}: {shown:?}"
            );
            // The fix clears it.
            let sent = e.handle(vec![notify(
                "textDocument/didChange",
                json!({"textDocument": {"uri": good, "version": 3}, "contentChanges": [{"text": TTL}]}),
            )]);
            assert!(
                published(&sent, good)
                    .unwrap()
                    .iter()
                    .all(|m| !m.contains("internal error"))
            );
        }
    }

    #[test]
    fn a_pulled_report_for_a_document_whose_analysis_panics_has_the_error_too() {
        let mut e = Embedded::start(json!({
            "capabilities": {"textDocument": {"diagnostic": {}}, "workspace": {"diagnostics": {"refreshSupport": true}}},
        }))
        .unwrap()
        .0;
        let uri = "file:///w/bad.ttl";
        quietly(|| {
            open(&mut e, uri, &format!("# {PANIC_IN_ANALYSIS}\n{TTL}"));
            let r = ask(
                &mut e,
                2,
                "textDocument/diagnostic",
                json!({"textDocument": {"uri": uri}}),
            );
            assert!(r.to_string().contains("internal error"), "{r}");
        });
    }

    #[test]
    fn a_failed_initial_index_is_reported_and_the_server_works_without_it() {
        let (here, host) = wire::memory();
        let (mut server, _, _) = Server::configure(
            here,
            json!({"capabilities": {"window": {"workDoneProgress": true}}}),
            Spawn::Thread,
        )
        .unwrap();
        assert!(!server.index.merged());
        server.index_arrived(Fed::Got(Err(
            "turtle-lsp internal error in index: boom".into()
        )));
        assert!(server.index.merged());
        let sent: Vec<Message> = host.try_iter().collect();
        let warned = sent.iter().any(|m| {
            matches!(m,
            Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("boom")
                && n.params["message"].as_str().unwrap().contains("not indexed"))
        });
        assert!(warned, "{sent:?}");
        // A thread that ended without a word is a failure too, not 0 files.
        let (here, host) = wire::memory();
        let (mut server, _, _) =
            Server::configure(here, json!({"capabilities": {}}), Spawn::Thread).unwrap();
        server.index_arrived(Fed::Gone);
        assert!(server.index.merged());
        let sent: Vec<Message> = host.try_iter().collect();
        assert!(
            sent.iter()
                .any(|m| matches!(m, Message::Notification(n) if n.method == "window/showMessage"))
        );
    }

    #[test]
    fn a_change_on_disk_before_the_initial_index_lands_is_not_undone_by_it() {
        let dir = std::env::temp_dir().join(format!("turtle-lsp-premerge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("folder")).unwrap();
        let dir = crate::workspace::real(&dir);
        let (kept, gone, in_folder) = (
            dir.join("kept.ttl"),
            dir.join("gone.ttl"),
            dir.join("folder/f.ttl"),
        );
        for f in [&kept, &gone, &in_folder] {
            std::fs::write(f, TTL).unwrap();
        }
        let (here, _host) = wire::memory();
        let root = path_to_uri(&dir).unwrap();
        let (mut server, _, roots) = Server::configure(
            here,
            json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
            Spawn::Thread,
        )
        .unwrap();
        server.ws.expect_initial_index();
        let mut reader = Workspace::new(roots.clone(), Default::default());
        let files = reader.parse_paths(Workspace::scan_roots(&roots));
        assert_eq!(files.len(), 3);
        // The disk changes while the index is on its way.
        std::fs::remove_file(&gone).unwrap();
        std::fs::remove_dir_all(dir.join("folder")).unwrap();
        let event = |p: &std::path::Path| FileEvent {
            uri: path_to_uri(p).unwrap().parse().unwrap(),
            typ: FileChangeType::DELETED,
        };
        server.watched_files_changed(vec![event(&gone), event(&dir.join("folder"))]);
        server.index_arrived(Fed::Got(Ok(files)));
        let indexed = |p: &std::path::Path| server.ws.is_indexed(&path_to_uri(p).unwrap());
        assert!(indexed(&kept));
        assert!(!indexed(&gone), "a deleted file came back");
        assert!(!indexed(&in_folder), "a file of a deleted folder came back");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A notification of many changed files indexes them in one parallel pass
    /// (it parsed them one at a time, a pass each), and tells the SPARQL
    /// index of the queries among them.
    #[test]
    fn a_notification_of_many_changes_is_indexed_in_one_pass() {
        let dir = std::env::temp_dir().join(format!("turtle-lsp-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let dir = crate::workspace::real(&dir);
        let (here, _host) = wire::memory();
        let root = path_to_uri(&dir).unwrap();
        let (mut server, _, _) = Server::configure(
            here,
            json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
            Spawn::Thread,
        )
        .unwrap();
        let event = |p: &std::path::Path, typ| FileEvent {
            uri: path_to_uri(p).unwrap().parse().unwrap(),
            typ,
        };
        let first = dir.join("first.rq");
        std::fs::write(&first, "SELECT * WHERE { ?s ?p ?o }\n").unwrap();
        assert_eq!(server.ws.sparql_documents().len(), 1);
        let files: Vec<std::path::PathBuf> =
            (0..120).map(|i| dir.join(format!("f{i}.ttl"))).collect();
        for f in &files {
            std::fs::write(f, TTL).unwrap();
        }
        let second = dir.join("second.rq");
        std::fs::write(&second, "SELECT * WHERE { ?s ?p ?o }\n").unwrap();
        std::fs::remove_file(&first).unwrap();
        let mut changes: Vec<FileEvent> = files
            .iter()
            .map(|f| event(f, FileChangeType::CHANGED))
            .collect();
        changes.push(event(&files[0], FileChangeType::CHANGED));
        changes.push(event(&second, FileChangeType::CREATED));
        changes.push(event(&first, FileChangeType::DELETED));
        let before = crate::workspace::INDEX_PASSES.with(|c| c.get());
        server.watched_files_changed(changes);
        assert_eq!(crate::workspace::INDEX_PASSES.with(|c| c.get()) - before, 1);
        assert_eq!(server.ws.indexed_count(), 120);
        let queries = server.ws.sparql_documents();
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].uri, path_to_uri(&second).unwrap());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shutdown_waits_for_exit_and_says_how_it_ended() {
        let ended = |messages: Vec<Message>, wait_ms: u64, drop_inbox: bool| {
            let (here, _host) = wire::memory();
            let (mut server, _, _) =
                Server::configure(here, json!({"capabilities": {}}), Spawn::Thread).unwrap();
            let (tx, rx) = unbounded();
            server.inbox = rx;
            for m in messages {
                tx.send(m).unwrap();
            }
            if drop_inbox {
                drop(tx);
            } else {
                // Keep the sender alive: only the clock ends the wait.
                std::mem::forget(tx);
            }
            let req = Request::new(1.into(), "shutdown".into(), Value::Null);
            server
                .shutdown(req, std::iter::empty(), Duration::from_millis(wait_ms))
                .unwrap()
        };
        let exit = || notify("exit", Value::Null);
        assert_eq!(ended(vec![exit()], 5000, false), Ended::Exit);
        assert_eq!(ended(vec![], 50, false), Ended::ShutdownTimedOut);
        assert_eq!(ended(vec![], 5000, true), Ended::ClientGone);
        // Requests meanwhile are refused, and exit still ends it.
        let hover = Message::Request(Request::new(
            2.into(),
            "textDocument/hover".into(),
            Value::Null,
        ));
        assert_eq!(ended(vec![hover, exit()], 5000, false), Ended::Exit);
        assert!(Ended::Exit.clean());
        for not in [
            Ended::ExitWithoutShutdown,
            Ended::ClientGone,
            Ended::ShutdownTimedOut,
        ] {
            assert!(!not.clean());
        }
    }

    #[test]
    fn after_a_shutdown_timeout_the_reader_is_not_joined() {
        // The reader of a client that sent `shutdown` and went quiet blocks
        // on stdin for good: joining it would keep the process alive.
        let join = |ended| {
            let mut joined = false;
            let clean = end_session(ended, || {
                joined = true;
                Ok::<(), std::io::Error>(())
            })
            .unwrap();
            (joined, clean)
        };
        assert_eq!(join(Ended::ShutdownTimedOut), (false, false));
        assert_eq!(join(Ended::Exit), (true, true));
        assert_eq!(join(Ended::ExitWithoutShutdown), (true, false));
        assert_eq!(join(Ended::ClientGone), (true, false));
    }

    /// A SHACL thread that cannot start ends the validation going on with
    /// "Validation stopped" and says why, instead of panicking; the next
    /// validation starts one.
    #[test]
    fn a_shacl_thread_that_cannot_start_stops_the_validation_and_says_why() {
        let (here, host) = wire::memory();
        let (mut server, _, _) = Server::configure(
            here,
            json!({"capabilities": {"window": {"workDoneProgress": true}}}),
            Spawn::Thread,
        )
        .unwrap();
        server.merge_index(Ok(Vec::new()));
        let shapes = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://e.org/> .\nex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n";
        let _ = server
            .ws
            .open_doc("file:///w/shapes.ttl".into(), shapes.into(), 1);
        // A validation going on, its progress shown.
        server
            .progress
            .begin(&mut server.client, Work::Shacl, "Validating");
        let token = host
            .try_iter()
            .find_map(|m| match m {
                Message::Request(r) if r.method == "window/workDoneProgress/create" => Some(r.id),
                _ => None,
            })
            .expect("a progress token asked for");
        server.progress.accepted(&server.client, &token, true);
        let _ = host.try_iter().count();

        worker::failing_starts(|| server.validate_shacl());
        let sent: Vec<Message> = host.try_iter().collect();
        let warned = sent.iter().any(|m| {
            matches!(m, Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("could not start its SHACL validation thread"))
        });
        assert!(warned, "{sent:?}");
        let stopped = sent.iter().any(|m| {
            matches!(m, Message::Notification(n) if n.method == "$/progress"
                && n.params["value"]["kind"] == "end"
                && n.params["value"]["message"] == "Validation stopped")
        });
        assert!(stopped, "{sent:?}");
        assert!(!server.progress.is_shown(Work::Shacl));
        assert!(!server.timers.is_set(Timer::ShaclProgress));

        // The next validation starts the thread and gets its answer.
        server.validate_shacl();
        let outcome = server
            .shacl
            .rx()
            .recv_timeout(Duration::from_secs(10))
            .expect("an outcome from the thread");
        assert!(server.shacl.is_current(&outcome));
    }

    /// Each answer is the text of the `Value` the same result makes (what
    /// lsp-server wrote): the result itself, not wrapped or reordered.
    #[test]
    fn an_answer_is_the_text_of_its_result_as_a_value() {
        let mut e = embedded();
        let uri = "file:///w/a.ttl";
        let text = "@prefix ex: <http://e.org/> .\nex:A a ex:B ;\n  ex:p \"x\u{e9}\" .\nex:B ex:q ex:A .\n";
        open(&mut e, uri, text);
        let doc = json!({"textDocument": {"uri": uri}});
        let pos = Position::new(1, 1);
        let at = json!({"textDocument": {"uri": uri}, "position": pos});
        let options = FormattingOptions {
            tab_size: 2,
            insert_spaces: true,
            ..Default::default()
        };
        let range = Range::new(Position::new(0, 0), Position::new(9, 0));
        let completion: CompletionParams = serde_json::from_value(
            json!({"textDocument": {"uri": uri}, "position": {"line": 3, "character": 4}}),
        )
        .unwrap();
        let value = |v: serde_json::Result<Value>| v.unwrap().to_string();
        let mut s = e.server.session();
        let cases = [
            (
                "textDocument/documentSymbol",
                doc.clone(),
                value(serde_json::to_value(
                    s.document_symbols(uri).map(DocumentSymbolResponse::Nested),
                )),
            ),
            (
                "textDocument/hover",
                at.clone(),
                value(serde_json::to_value(s.hover(uri, pos))),
            ),
            (
                "textDocument/definition",
                at.clone(),
                value(serde_json::to_value(
                    s.definition(uri, pos).map(GotoDefinitionResponse::Array),
                )),
            ),
            (
                "textDocument/references",
                json!({"textDocument": {"uri": uri}, "position": pos, "context": {"includeDeclaration": true}}),
                value(serde_json::to_value(s.references(uri, pos, true))),
            ),
            (
                "textDocument/foldingRange",
                doc.clone(),
                value(serde_json::to_value(s.folding_ranges(uri))),
            ),
            (
                "textDocument/semanticTokens/full",
                doc.clone(),
                value(serde_json::to_value(s.semantic_tokens_full(uri))),
            ),
            (
                "textDocument/formatting",
                json!({"textDocument": {"uri": uri}, "options": options}),
                value(serde_json::to_value(s.formatting(uri, &options))),
            ),
            (
                "textDocument/inlayHint",
                json!({"textDocument": {"uri": uri}, "range": range}),
                value(serde_json::to_value(s.inlay_hints(uri, range))),
            ),
            (
                "textDocument/completion",
                serde_json::to_value(&completion).unwrap(),
                value(serde_json::to_value(s.completion(&completion))),
            ),
        ];
        for (method, params, expected) in cases {
            let req = Request::new(1.into(), method.into(), params);
            let got = e.server.request(req).unwrap().unwrap();
            assert_eq!(got, expected, "{method}");
        }
    }

    /// The `initialize` result is the text the `Value` it was built as
    /// gave: lsp-types' result with `typeHierarchyProvider` set in it.
    #[test]
    fn the_initialize_result_is_the_text_its_value_gave() {
        for enc in [Encoding::Utf8, Encoding::Utf16] {
            for pull in [false, true] {
                let result = InitializeResult {
                    capabilities: capabilities(enc, pull),
                    server_info: Some(ServerInfo {
                        name: "turtle-lsp".into(),
                        version: Some(crate::VERSION.into()),
                    }),
                };
                let mut value = serde_json::to_value(result).unwrap();
                value["capabilities"]["typeHierarchyProvider"] = Value::Bool(true);
                let ours = wire::encode(&Initialized::new(capabilities(enc, pull))).unwrap();
                assert_eq!(ours, value.to_string());
            }
        }
    }

    #[test]
    fn guard_turns_a_panic_into_an_error() {
        let r: Result<(), String> = quietly(|| guard("hover", || panic!("boom")));
        let msg = r.unwrap_err();
        assert!(msg.contains("hover") && msg.contains("boom"), "{msg}");
        assert_eq!(guard("x", || 7), Ok(7));
    }
}

#[cfg(test)]
mod review_tests {
    use super::*;
    use crate::workspace::path_to_uri;
    use serde_json::json;

    /// A fresh folder below the temp dir (canonical), removed when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir()
                .join(format!("turtle-lsp-review-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(crate::workspace::real(&d))
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A server (not started) for the workspace folder `root`, with the
    /// client's connection end, which must outlive it.
    fn server_at(root: &std::path::Path, init: Value) -> (Server, wire::Outbox) {
        let (here, host) = wire::memory();
        let root = path_to_uri(root).unwrap();
        let (server, _, _) = Server::configure(
            here,
            json!({
                "capabilities": {},
                "workspaceFolders": [{"uri": root, "name": "w"}],
                "initializationOptions": init,
            }),
            Spawn::Thread,
        )
        .unwrap();
        (server, host)
    }

    fn configuration(settings: Value) -> Notification {
        Notification::new(
            "workspace/didChangeConfiguration".into(),
            json!({ "settings": settings }),
        )
    }

    fn event(p: &std::path::Path, typ: FileChangeType) -> FileEvent {
        FileEvent {
            uri: path_to_uri(p).unwrap().parse().unwrap(),
            typ,
        }
    }

    fn first_subject(server: &Server, path: &std::path::Path) -> String {
        let uri = path_to_uri(path).unwrap();
        match &server.ws.indexed_analysis(&uri).unwrap().parsed.triples[0].s {
            crate::syntax::Node::Iri(i) => i.as_str().to_string(),
            other => panic!("{other:?}"),
        }
    }

    /// The settings are the `turtle` section of the notification's
    /// `settings`: the shape the VS Code extension sends.
    #[test]
    fn a_configuration_change_is_read_from_its_turtle_section() {
        let dir = Dir::new("shape");
        let (mut s, _client) = server_at(&dir.0, Value::Null);
        s.notification(configuration(json!({"format": {"lineWidth": 80}})));
        assert_eq!(
            s.layers.settings().format_line_width,
            100,
            "no turtle section"
        );
        s.notification(configuration(
            json!({"turtle": {"format": {"lineWidth": 80}}}),
        ));
        assert_eq!(s.layers.settings().format_line_width, 80);
    }

    /// A key the user removed is not kept from `initializationOptions`: the
    /// client's latest section replaces its start-up snapshot, and the key
    /// comes from turtle-lsp.json again (or the default).
    #[test]
    fn a_configuration_section_replaces_the_initialization_options() {
        let dir = Dir::new("layers");
        let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
        assert_eq!(s.layers.settings().format_line_width, 80);
        s.notification(configuration(
            json!({"turtle": {"codeLens": {"references": false}}}),
        ));
        assert_eq!(s.layers.settings().format_line_width, 100);
        assert!(!s.layers.settings().code_lens_references);

        let dir = Dir::new("layers-file");
        std::fs::write(
            dir.0.join(SETTINGS_FILE),
            r#"{"format": {"lineWidth": 90}}"#,
        )
        .unwrap();
        let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
        assert_eq!(s.layers.settings().format_line_width, 80);
        s.notification(configuration(json!({"turtle": {}})));
        assert_eq!(s.layers.settings().format_line_width, 90);
        // A client that only says "something changed" keeps what it sent.
        let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
        s.notification(configuration(Value::Null));
        assert_eq!(s.layers.settings().format_line_width, 80);
    }

    /// A notification that changes a prefixes.ttl and a data file it
    /// serves parses the data file once, with the new prefixes: not with the
    /// old ones and then again in the background.
    #[test]
    fn prefixes_and_a_data_file_changed_together_parse_the_data_once_with_the_new_prefixes() {
        let dir = Dir::new("prefixes");
        let prefixes = dir.0.join("prefixes.ttl");
        let data = dir.0.join("data.ttl");
        std::fs::write(&prefixes, "@prefix ex: <http://old.example/> .\n").unwrap();
        std::fs::write(&data, "ex:a ex:p ex:o .\n").unwrap();
        let (mut s, _client) = server_at(&dir.0, Value::Null);
        s.watched_files_changed(vec![
            event(&data, FileChangeType::CREATED),
            event(&prefixes, FileChangeType::CREATED),
        ]);
        assert_eq!(first_subject(&s, &data), "http://old.example/a");

        std::fs::write(&prefixes, "@prefix ex: <http://newer.example/> .\n").unwrap();
        std::fs::write(&data, "ex:b ex:p ex:o .\n").unwrap();
        s.watched_files_changed(vec![
            event(&data, FileChangeType::CHANGED),
            event(&prefixes, FileChangeType::CHANGED),
        ]);
        assert_eq!(first_subject(&s, &data), "http://newer.example/b");
        // Nothing was left to parse again in the background.
        assert!(
            s.index
                .reparsed()
                .recv_timeout(Duration::from_millis(500))
                .is_err()
        );
    }

    /// A workspace-diagnostics thread that cannot start turns them off with
    /// a warning, instead of panicking; a settings change turns them on again.
    #[test]
    fn a_workspace_diagnostics_thread_that_cannot_start_turns_them_off_with_a_warning() {
        let dir = Dir::new("wsdiag-start");
        let file = dir.0.join("a.ttl");
        std::fs::write(&file, "ex:a ex:p ex:o .\n").unwrap();
        let (mut s, host) = server_at(&dir.0, json!({"diagnostics": {"workspace": true}}));
        let roots = s.ws.roots().to_vec();
        let files = Workspace::new(roots.clone(), Default::default())
            .parse_paths(Workspace::scan_roots(&roots));
        s.merge_index(Ok(files));
        assert!(s.wsdiag.due().is_some(), "a pass is due");
        let _ = host.try_iter().count();

        worker::failing_starts(|| s.wsdiag_start());
        let sent: Vec<Message> = host.try_iter().collect();
        let warned = sent.iter().any(|m| {
            matches!(m, Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("could not start its workspace diagnostics thread"))
        });
        assert!(warned, "{sent:?}");
        assert!(s.wsdiag.due().is_none(), "nothing is due any more");
        s.wsdiag.invalidate_all();
        assert!(s.wsdiag.due().is_none(), "they are off");

        // A settings change turns them on again, and the pass runs.
        s.notification(configuration(json!({"turtle": {
            "diagnostics": {"workspace": true},
            "format": {"lineWidth": 80},
        }})));
        assert!(s.wsdiag.due().is_some());
        s.wsdiag_start();
        let chunk = s
            .wsdiag
            .results()
            .recv_timeout(Duration::from_secs(10))
            .expect("a pass's results");
        let _ = host.try_iter().count();
        s.wsdiag_chunk(Some(chunk));
        let uri = path_to_uri(&file).unwrap();
        let published = host.try_iter().any(|m| {
            matches!(m, Message::Notification(n) if n.method == "textDocument/publishDiagnostics"
                && n.params["uri"] == uri.as_str())
        });
        assert!(published, "the closed file's diagnostics went out");
    }

    /// Catalogs, imported RDF/XML files and JSON-LD context files count
    /// wherever the client reports them, also in folders the index skips.
    #[test]
    fn import_sources_and_context_files_in_skipped_folders_are_acted_on() {
        let dir = Dir::new("skipped");
        std::fs::create_dir_all(dir.0.join(".ctx")).unwrap();
        let ctx = dir.0.join(".ctx").join("ctx.jsonld");
        std::fs::write(&ctx, r#"{"@context": {"name": "http://ex.org/name"}}"#).unwrap();
        let (mut s, _client) = server_at(&dir.0, Value::Null);
        let doc = path_to_uri(&dir.0.join("a.jsonld")).unwrap();
        s.ws.open_doc(
            doc.clone(),
            r#"{"@context": ".ctx/ctx.jsonld", "@id": "http://ex.org/a", "name": "A"}"#.into(),
            1,
        );
        assert_eq!(s.ws.analysis(&doc).unwrap().parsed.triples.len(), 1);
        std::fs::write(&ctx, r#"{"@context": {"name": null}}"#).unwrap();
        s.watched_files_changed(vec![event(&ctx, FileChangeType::CHANGED)]);
        assert_eq!(s.ws.analysis(&doc).unwrap().parsed.triples.len(), 0);

        for source in [
            dir.0.join(".ontologies").join("catalog-v001.xml"),
            dir.0.join("venv").join("onto.owl"),
            dir.0.join("node_modules").join("x.rdf"),
        ] {
            let before = crate::imports::RETRIES.with(|c| c.get());
            s.watched_files_changed(vec![event(&source, FileChangeType::CHANGED)]);
            assert_eq!(
                crate::imports::RETRIES.with(|c| c.get()) - before,
                1,
                "{source:?}"
            );
        }
    }
}
