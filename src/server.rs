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
//! start through `worker::spawn_guarded` and their answers come back through
//! `worker::Feed`; the timers are keyed in `timers::Timers`.
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
//! Every request is routed by document kind: RDF documents to `features`,
//! SPARQL documents to `sparql::features`; neither runs on the other.

use crate::analysis::{Analysis, Encoding};
use crate::clock::Instant;
use crate::diagnose::{self, DiagnosticInputs, Doc};
use crate::features::hierarchy;
use crate::features::{
    completion, diagnostics, embedded, format, navigation, quickfix, refactor, shacl_validate,
};
use crate::ide;
use crate::settings::{Layer, SETTINGS_FILE, Settings, read_settings_file};
use crate::sparql::features as sparql_features;
use crate::workspace::{
    Change, Dirty, DocKind, Effects, Workspace, is_import_source, normalize_uri, uri_to_path,
};
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::Sender;
use crossbeam_channel::{Receiver, never, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
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

mod run_query;
mod timers;
pub(crate) mod worker;
mod workspace_diagnostics;

use timers::{Timer, Timers};
use worker::{Fed, Feed};

/// The files the client is asked to report changes of: RDF documents, SPARQL
/// queries, JSON-LD context files (`.json` too), imports sources and the
/// settings file (a `.json` file). It is the glob of every row of the
/// file-kind table ([`crate::workspace::kinds::ROWS`]), which a test checks.
pub const WATCHED_FILES: &str = "**/{*.ttl,*.turtle,*.trig,*.nt,*.nq,*.jsonld,*.json,*.rdf,*.owl,*.rq,*.sparql,*.ru,catalog-v001.xml}";

/// Pause after a described-set change before other documents are refreshed.
const DESCRIBED_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

/// How long a client that has sent `shutdown` is waited for to send `exit`.
const EXIT_WAIT: Duration = Duration::from_secs(30);

/// How long an embedded host (no threads) waits for one SHACL validation.
const INLINE_SHACL_LIMIT: Duration = Duration::from_secs(10);

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

/// A SHACL run shows progress only once it has lasted this long.
const SHACL_PROGRESS_DELAY: Duration = Duration::from_millis(200);

pub struct Server {
    conn: Connection,
    /// Every message read so far. lsp-server hands over messages through a
    /// rendezvous channel, so without this queue a batch would never hold
    /// more than one message.
    inbox: Receiver<Message>,
    ws: Workspace,
    enc: Encoding,
    dirty: FxHashSet<String>,
    /// How the client spelled an open document's URI, when that differs
    /// from the key (`%7E` for `~`, `c%3A` for `C:`): diagnostics go back
    /// under the client's spelling.
    spelling: FxHashMap<String, String>,
    /// The initial workspace index: the files parsed, or why it failed.
    index: Feed<Result<Vec<(String, Analysis)>, String>>,
    /// Indexed files parsed again after prefixes changed. This server holds
    /// the sender, so the channel never disconnects.
    #[cfg(not(target_arch = "wasm32"))]
    reindex_tx: Sender<Result<Vec<crate::workspace::Reparsed>, String>>,
    #[cfg(not(target_arch = "wasm32"))]
    reindex_rx: Receiver<Result<Vec<crate::workspace::Reparsed>, String>>,
    /// How long `shutdown` waits for `exit`.
    exit_wait: Duration,
    /// The initial workspace index has been merged.
    indexed: bool,
    watch_dynamic: bool,
    /// The client resolves a code action's edit lazily (`codeAction/resolve`).
    resolve_edits: bool,
    next_id: i32,
    /// `turtle-lsp.json` at the first workspace root (Null when absent or
    /// invalid), `initializationOptions` and the last `turtle` configuration
    /// section; `settings` is the defaults overridden by these, in that order.
    /// The first configuration section the client sends (in
    /// `didChangeConfiguration` or a `workspace/configuration` answer)
    /// replaces `initializationOptions`: the client's latest view of its
    /// settings supersedes its start-up snapshot, so a key the user removed
    /// falls back to `turtle-lsp.json` or the default.
    file_settings: Value,
    /// The warning about `turtle-lsp.json` at startup (unusable, or keys it may
    /// not set), shown once started.
    file_settings_error: Option<String>,
    init_options: Value,
    config: Value,
    settings: Settings,
    /// The client can answer `workspace/configuration`.
    config_pull: bool,
    /// The pending `workspace/configuration` request.
    config_request: Option<RequestId>,
    /// The client pulls diagnostics; nothing is pushed to it.
    pull: bool,
    refresh_support: bool,
    /// Bumped whenever knowledge outside a document changes (prefixes.ttl,
    /// the disk index, settings). Part of every pulled report's result id.
    generation: u64,
    /// A pulled report went out since the last `workspace/diagnostic/refresh`.
    reported: bool,
    /// The generation moved after a report went out: refresh at end of batch.
    refresh_needed: bool,
    /// The knowledge index's described generation last acted on.
    described_seen: u64,
    /// What waits for a time to come: acting on a described-set change (see
    /// `check_described`), validating after typing stops (`shacl.onType`;
    /// each edit pushes it back), showing a long SHACL run's progress.
    timers: Timers,
    /// `workspace/{inlayHint,semanticTokens}/refresh`, those the client supports.
    display_refresh: Vec<&'static str>,
    /// Knowledge changed since the last display refresh.
    display_stale: bool,
    imports: crate::imports::Imports,
    /// JSON-LD contexts fetched in the background (with `imports.network`),
    /// by URL, once each has arrived or failed.
    contexts: Feed<String>,
    /// SHACL validation thread, started when the workspace first has shapes.
    shacl: Option<shacl_validate::Worker>,
    shacl_feed: Feed<shacl_validate::Outcome>,
    /// The latest validation job; older results are dropped.
    shacl_generation: u64,
    /// The latest validation results per document.
    shacl_diagnostics: shacl_validate::ByUri,
    /// The client declared `window.workDoneProgress`.
    work_done_progress: bool,
    /// Tokens of the progress shown, per kind of work.
    index_progress: Option<String>,
    imports_progress: Option<String>,
    shacl_progress: Option<String>,
    /// Progress whose create request is unanswered, by that request's id.
    progress_pending: FxHashMap<RequestId, PendingProgress>,
    /// Tokens whose `begin` went out and whose `end` has not.
    progress_live: FxHashSet<String>,
    /// Embedded in a host without threads (see [`Embedded`]): SHACL and
    /// imports run inline, and the host drives the timers.
    inline: bool,
    /// Diagnostics of the files that are not open (`diagnostics.workspace`).
    wsdiag: workspace_diagnostics::State,
    /// The "Run query" runs going on.
    queries: run_query::State,
    /// The client declared `window.showDocument.support`.
    show_document: bool,
    /// The request being handled is answered later (a query run).
    answer_later: bool,
    /// Tests: panic once after the next batch.
    #[cfg(test)]
    fault: bool,
}

/// A progress waiting for the client to accept its token.
struct PendingProgress {
    token: String,
    title: String,
    /// The end message, when the work ended before the client answered.
    ended: Option<String>,
}

impl Server {
    /// A server for the client's `initialize` params (`raw`), not started,
    /// with the initialize result to answer and the workspace roots. It reads
    /// no messages and indexes nothing: the caller wires both.
    #[allow(clippy::type_complexity)]
    fn configure(
        conn: Connection,
        raw: Value,
        inline: bool,
    ) -> Result<(Server, Value, Vec<PathBuf>), Box<dyn std::error::Error + Send + Sync>> {
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
        let (file_settings, file_settings_error) = if inline {
            (Value::Null, None)
        } else {
            read_file_settings(roots.first())
        };
        let init_options = params.initialization_options.clone().unwrap_or_default();
        let settings = Settings::from_layers([
            (Layer::File, &file_settings),
            (Layer::Client, &init_options),
        ]);
        // Embedded, every file is open.
        let wsdiag = workspace_diagnostics::State::new(settings.diagnostics_workspace && !inline);
        let mut ws = Workspace::new(roots.clone(), settings.prefixes.clone());
        // Without watchers (see `register_watchers`), nothing reports new files.
        ws.set_rescan_sparql(!watch_dynamic);
        let contexts = use_network(&mut ws, settings.imports_network, inline);
        #[cfg(not(target_arch = "wasm32"))]
        let (reindex_tx, reindex_rx) = unbounded();

        let result = InitializeResult {
            capabilities: capabilities(enc, pull),
            server_info: Some(ServerInfo {
                name: "turtle-lsp".into(),
                version: Some(crate::VERSION.into()),
            }),
        };
        let mut result = serde_json::to_value(result)?;
        // lsp-types 0.97 has no field for it.
        result["capabilities"]["typeHierarchyProvider"] = Value::Bool(true);

        let server = Server {
            conn,
            inbox: never(),
            ws,
            enc,
            dirty: FxHashSet::default(),
            spelling: FxHashMap::default(),
            index: Feed::default(),
            #[cfg(not(target_arch = "wasm32"))]
            reindex_tx,
            #[cfg(not(target_arch = "wasm32"))]
            reindex_rx,
            exit_wait: EXIT_WAIT,
            indexed: false,
            watch_dynamic,
            resolve_edits,
            next_id: 0,
            file_settings,
            file_settings_error,
            init_options,
            config: Value::Null,
            settings,
            config_pull,
            config_request: None,
            pull,
            refresh_support,
            generation: 0,
            reported: false,
            refresh_needed: false,
            described_seen: 0,
            timers: Timers::default(),
            display_refresh,
            display_stale: false,
            imports: if inline {
                crate::imports::Imports::new_inline()
            } else {
                crate::imports::Imports::new()
            },
            contexts,
            shacl: None,
            shacl_feed: Feed::default(),
            shacl_generation: 0,
            shacl_diagnostics: Default::default(),
            work_done_progress,
            index_progress: None,
            imports_progress: None,
            shacl_progress: None,
            progress_pending: FxHashMap::default(),
            progress_live: FxHashSet::default(),
            wsdiag,
            queries: Default::default(),
            show_document,
            answer_later: false,
            inline,
            #[cfg(test)]
            fault: false,
        };
        Ok((server, result, roots))
    }
}

/// Load JSON-LD contexts with the network when `network` is on: in the
/// background, so the main loop never waits for a fetch (inline when
/// embedded, where there are no threads). The receiver names each context
/// fetched.
fn use_network(ws: &mut Workspace, network: bool, inline: bool) -> Feed<String> {
    #[cfg(not(target_arch = "wasm32"))]
    if network && !inline {
        let (loader, fetched) =
            crate::jsonld::loader::DesktopLoader::in_background(crate::imports::import_cache_dir());
        ws.set_context_loader(Arc::new(loader));
        return Feed::open(fetched);
    }
    let _ = inline;
    ws.set_network(network);
    Feed::default()
}

/// The settings layer from `turtle-lsp.json` at `root`, and the warning to
/// show for it, if any: Null when there is no root, no file, or a bad file;
/// the warning names the keys of a good file that it may not set.
fn read_file_settings(root: Option<&PathBuf>) -> (Value, Option<String>) {
    match root.map(|r| read_settings_file(r)) {
        Some(Ok(Some(v))) => {
            let (_, ignored) = Settings::layered([(Layer::File, &v)]);
            let warning = ignored
                .message(SETTINGS_FILE)
                .map(|m| format!("turtle-lsp: {m}"));
            (v, warning)
        }
        Some(Err(e)) => (
            Value::Null,
            Some(format!("turtle-lsp: {e}; its settings are ignored")),
        ),
        _ => (Value::Null, None),
    }
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
    let (conn, io) = Connection::stdio();
    let (id, raw) = conn.initialize_start()?;

    let (mut server, result, roots) = Server::configure(conn, raw, false)?;
    server.conn.initialize_finish(id, result)?;

    // Index the workspace in the background. What changes on disk meanwhile
    // is remembered, so that the index does not undo it when it lands.
    server.ws.expect_initial_index();
    let (tx, index_rx) = unbounded();
    let prefixes = server.ws.settings_prefixes().clone();
    // The same context loader: a context is fetched once, in the background.
    let contexts = server.ws.shared_contexts();
    let failed = tx.clone();
    let started = worker::spawn_guarded(
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
    server.index = Feed::open(index_rx);

    let (inbox_tx, inbox) = unbounded();
    let incoming = server.conn.receiver.clone();
    std::thread::spawn(move || {
        for msg in incoming {
            if inbox_tx.send(msg).is_err() {
                break;
            }
        }
    });
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
                recv(self.reindex_rx) -> done => {
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
                recv(self.shacl_feed.rx()) -> got => {
                    match self.shacl_feed.take(got) {
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
    fn index_arrived(&mut self, got: Fed<Result<Vec<(String, Analysis)>, String>>) {
        self.merge_index(match got {
            Fed::Got(files) => files,
            Fed::Gone => Err("turtle-lsp internal error: the indexing thread ended".into()),
        });
    }

    /// The validation thread is gone.
    #[cfg(not(target_arch = "wasm32"))]
    fn shacl_gone(&mut self) {
        self.shacl = None;
        self.timers.clear(Timer::ShaclProgress);
        let token = self.shacl_progress.take();
        self.progress_end(token, "Validation stopped".into());
    }

    /// Before the first message: register file watchers, show indexing.
    fn start(&mut self) {
        self.register_watchers();
        if let Some(e) = self.file_settings_error.take() {
            self.show_warning(e);
        }
        if !self.indexed {
            self.index_progress = self.progress_begin("Indexing workspace");
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
                    if self.inline {
                        // The host ends the session; nothing waits for `exit`.
                        self.conn.sender.send(Response::new_ok(req.id, ()).into())?;
                        return Ok(Some(Ended::Exit));
                    }
                    return self.shutdown(req, rest.into_iter()).map(Some);
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
                    self.conn.sender.send(Message::Response(resp))?;
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
                    s.shacl_progress = s.progress_begin("Validating SHACL shapes");
                }
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
        if g != self.described_seen {
            self.timers
                .set(Timer::Described, Instant::now() + DESCRIBED_DEBOUNCE);
        }
    }

    /// The pause after a described-set change has passed.
    fn described_settled(&mut self) {
        let g = self.ws.knowledge().described_generation();
        if g != self.described_seen {
            self.described_seen = g;
            self.knowledge_changed();
        }
    }

    /// Answer `shutdown`, then wait for `exit`; other requests are refused.
    fn shutdown(
        &self,
        req: Request,
        rest: impl Iterator<Item = Message>,
    ) -> Result<Ended, Box<dyn std::error::Error + Send + Sync>> {
        self.queries.cancel_all();
        self.conn.sender.send(Response::new_ok(req.id, ()).into())?;
        let mut rest = rest;
        loop {
            let msg = match rest.next() {
                Some(m) => m,
                None => match self.inbox.recv_timeout(self.exit_wait) {
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
                    self.conn.sender.send(resp.into())?;
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
                if std::mem::take(&mut self.answer_later) && result.is_ok() {
                    return Ok(());
                }
                let resp = match result {
                    Ok(v) => Response::new_ok(id, v),
                    Err((code, message)) => Response::new_err(id, code as i32, message),
                };
                self.conn.sender.send(Message::Response(resp))?;
            }
            Message::Notification(n) => {
                let method = n.method.clone();
                let _ = guard(&method, || self.notification(n));
            }
            Message::Response(r) => {
                if let Some(p) = self.progress_pending.remove(&r.id) {
                    self.progress_accepted(p, r.response_result.is_ok());
                }
                if self.config_request.as_ref() == Some(&r.id) {
                    self.config_request = None;
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
                    if self.settings.shacl_on_type {
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
                    if !self.wsdiag_closed(&uri) && !self.pull {
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
                    None if self.config_pull => self.request_config(),
                    None => {}
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
            _ => {}
        }
    }

    /// Embedded, the open documents are the whole workspace (there is no
    /// disk to index): opening or closing one changes the data SHACL
    /// validates, so validate once the host pauses, as after typing.
    fn workspace_changed_inline(&mut self) {
        if self.inline {
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
        if !self.inline {
            let (tx, failed) = (self.reindex_tx.clone(), self.reindex_tx.clone());
            let started = worker::spawn_guarded(
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
            Err(msg) => self.show_warning(msg),
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
        if !self.indexed {
            return;
        }
        let roots = self.ws.roots().to_vec();
        let network = self.settings.imports_network;
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
        match (self.imports.busy(), self.imports_progress.is_some()) {
            (true, false) => {
                self.imports_progress = self.progress_begin("Resolving owl:imports");
            }
            (false, true) => {
                let n = self.imports.unresolved().len();
                let msg = match n {
                    0 => "All imports resolved".to_string(),
                    1 => "1 import unresolved".to_string(),
                    n => format!("{n} imports unresolved"),
                };
                let token = self.imports_progress.take();
                self.progress_end(token, msg);
            }
            _ => {}
        }
    }

    /// Ask the client for a progress token for `title`, when the client
    /// supports progress. `begin` goes out once the client accepts the token
    /// (see `progress_accepted`); the main loop never waits for that.
    fn progress_begin(&mut self, title: &str) -> Option<String> {
        if !self.work_done_progress {
            return None;
        }
        let token = format!("turtle-progress-{}", self.next_id + 1);
        let id = self.send_request(
            lsp_types::request::WorkDoneProgressCreate::METHOD,
            WorkDoneProgressCreateParams {
                token: NumberOrString::String(token.clone()),
            },
        );
        self.progress_pending.insert(
            id,
            PendingProgress {
                token: token.clone(),
                title: title.into(),
                ended: None,
            },
        );
        Some(token)
    }

    /// The client answered a create request: begin (and end, if the work is
    /// already over). A refused token is dropped.
    fn progress_accepted(&mut self, p: PendingProgress, ok: bool) {
        if !ok {
            return;
        }
        self.send_progress(
            &p.token,
            WorkDoneProgress::Begin(WorkDoneProgressBegin {
                title: p.title,
                cancellable: Some(false),
                ..Default::default()
            }),
        );
        match p.ended {
            Some(message) => self.send_end(&p.token, message),
            None => {
                self.progress_live.insert(p.token);
            }
        }
    }

    fn progress_end(&mut self, token: Option<String>, message: String) {
        let Some(token) = token else { return };
        if self.progress_live.remove(&token) {
            self.send_end(&token, message);
        } else if let Some(p) = self
            .progress_pending
            .values_mut()
            .find(|p| p.token == token)
        {
            p.ended = Some(message);
        }
    }

    fn send_end(&self, token: &str, message: String) {
        self.send_progress(
            token,
            WorkDoneProgress::End(WorkDoneProgressEnd {
                message: Some(message),
            }),
        );
    }

    fn send_progress(&self, token: &str, value: WorkDoneProgress) {
        let params = ProgressParams {
            token: NumberOrString::String(token.into()),
            value: ProgressParamsValue::WorkDone(value),
        };
        let n = Notification::new(lsp_types::notification::Progress::METHOD.into(), params);
        let _ = self.conn.sender.send(Message::Notification(n));
    }

    fn request_config(&mut self) {
        let params = ConfigurationParams {
            items: vec![ConfigurationItem {
                scope_uri: None,
                section: Some("turtle".into()),
            }],
        };
        let id = self.send_request(lsp_types::request::WorkspaceConfiguration::METHOD, params);
        self.config_request = Some(id);
    }

    /// The client's `turtle` configuration section changed. It replaces
    /// `initializationOptions` as well as the previous section (see
    /// [`Server::file_settings`]).
    fn set_config(&mut self, section: Value) {
        if section.is_null() {
            return;
        }
        self.init_options = Value::Null;
        self.config = section;
        self.relayer_settings();
    }

    /// `turtle-lsp.json` changed on disk: read it again. An invalid file
    /// warns and counts as absent.
    fn reload_settings_file(&mut self) {
        let (file, error) = read_file_settings(self.ws.roots().first());
        self.file_settings = file;
        if let Some(e) = error {
            self.show_warning(e);
        }
        self.relayer_settings();
    }

    fn show_warning(&self, message: String) {
        let params = ShowMessageParams {
            typ: MessageType::WARNING,
            message,
        };
        let n = Notification::new(lsp_types::notification::ShowMessage::METHOD.into(), params);
        let _ = self.conn.sender.send(n.into());
    }

    /// Recompute the settings from their layers and act on any change.
    fn relayer_settings(&mut self) {
        let new = Settings::from_layers([
            (Layer::File, &self.file_settings),
            (Layer::Client, &self.init_options),
            (Layer::Client, &self.config),
        ]);
        if new == self.settings {
            return;
        }
        // Any setting can change every document's diagnostics, and SHACL's.
        let mut fx = Effects {
            shacl: true,
            ..Effects::knowledge()
        };
        if new.prefixes != self.settings.prefixes {
            // The background index was built with the old prefixes.
            self.ensure_indexed();
            fx.add(self.ws.set_settings_prefixes(new.prefixes.clone()));
        }
        if new.imports_network != self.settings.imports_network {
            // Indexed JSON-LD files are parsed again with the new loader.
            self.ensure_indexed();
            self.contexts = use_network(&mut self.ws, new.imports_network, self.inline);
        }
        self.settings = new;
        self.wsdiag_setting();
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
            // Pulled reports handed out so far are stale.
            self.generation += 1;
            if self.reported {
                self.refresh_needed = true;
            }
            // Labels and term kinds shown in open documents may come from elsewhere.
            self.display_stale = true;
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
        if std::mem::take(&mut self.display_stale) {
            for m in self.display_refresh.clone() {
                self.send_request(m, ());
            }
        }
        if !std::mem::take(&mut self.refresh_needed) {
            return;
        }
        self.reported = false;
        if self.refresh_support {
            self.send_request(lsp_types::request::WorkspaceDiagnosticRefresh::METHOD, ());
        }
    }

    /// The initial index arrived (or failed, with the reason). Either way
    /// the server goes on: documents that are open are analyzed as ever.
    fn merge_index(&mut self, files: Result<Vec<(String, Analysis)>, String>) {
        let token = self.index_progress.take();
        let files = match files {
            Ok(files) => {
                self.progress_end(
                    token,
                    match files.len() {
                        1 => "1 file".to_string(),
                        n => format!("{n} files"),
                    },
                );
                files
            }
            Err(msg) => {
                self.progress_end(token, "Indexing failed".into());
                self.show_warning(format!(
                    "{msg}. The workspace's files are not indexed; the documents you open still work."
                ));
                Vec::new()
            }
        };
        self.ws.merge_initial_index(files);
        self.indexed = true;
        // The sender is gone now; a disconnected channel is always ready, which
        // would wake the main loop forever.
        self.index.close();
        self.apply(Effects {
            shacl: true,
            ..Effects::knowledge()
        });
    }

    /// Answers that look across the workspace (definition, references, rename,
    /// ...) must not come from a half-built index: a rename sent right after
    /// start-up would silently miss files. Wait for the initial index once.
    fn ensure_indexed(&mut self) {
        if !self.indexed {
            let got = if self.index.is_open() {
                self.index.rx().recv()
            } else {
                Err(crossbeam_channel::RecvError)
            };
            let got = self.index.take(got);
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
        let enc = self.enc;
        let sparql = self.ws.is_sparql(uri);
        guard("diagnostics", || {
            let status = |iri: &str| self.imports.statuses().get(iri);
            let inputs = DiagnosticInputs {
                import_status: &status,
                shacl: self.shacl_diagnostics.get(uri).map_or(&[], Vec::as_slice),
                indexed: self.indexed,
                cached_import,
                ..DiagnosticInputs::new(&self.settings)
            };
            if sparql {
                let q = self.ws.sparql(uri);
                return q.map(|q| {
                    diagnose::document(Doc::Sparql(&q), self.ws.knowledge(), enc, &inputs)
                });
            }
            let a = self.ws.analysis(uri);
            a.map(|a| diagnose::document(Doc::Rdf(&a), self.ws.knowledge(), enc, &inputs))
        })
        // A cached copy of an import is read-only: nothing to fix there.
        .map(|d| d.or_else(|| cached_import.then(Vec::new)))
    }

    fn publish_dirty(&mut self) {
        let dirty: Vec<String> = self.dirty.drain().collect();
        if self.pull {
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
        // The described set is part of the id: other documents' edits that
        // change it may not have bumped the generation yet (debounced).
        let described = self.ws.knowledge().described_generation();
        let mut result_id = Some(format!("{version}:{}:{described}", self.generation));
        self.reported = true;
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
        let n = Notification::new(
            lsp_types::notification::PublishDiagnostics::METHOD.into(),
            params,
        );
        let _ = self.conn.sender.send(Message::Notification(n));
    }

    /// Send a request to the client; the answer is matched by the returned id.
    fn send_request(&mut self, method: &str, params: impl serde::Serialize) -> RequestId {
        self.next_id += 1;
        let id = RequestId::from(format!("turtle-{}", self.next_id));
        let req = Request::new(id.clone(), method.into(), params);
        let _ = self.conn.sender.send(Message::Request(req));
        id
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
        self.send_request(lsp_types::request::RegisterCapability::METHOD, params);
    }

    /// The editor session over this server's workspace, for one request.
    fn session(&mut self) -> ide::Session<'_> {
        ide::Session::new(&mut self.ws, self.enc, &self.settings, self.resolve_edits)
    }

    fn request(&mut self, req: Request) -> Result<Value, (ErrorCode, String)> {
        use lsp_types::request::*;
        let enc = self.enc;
        let bad = |e: serde_json::Error| (ErrorCode::InvalidParams, e.to_string());
        let ok = |v: Value| Ok(v);
        macro_rules! params {
            ($t:ty) => {
                serde_json::from_value::<$t>(req.params.clone()).map_err(bad)?
            };
        }
        // An RDF document's analysis; a SPARQL document answers `null`.
        macro_rules! doc {
            ($uri:expr) => {
                match self.session().doc($uri.as_str()) {
                    Some(ide::Doc::Rdf(a)) => a,
                    Some(ide::Doc::Sparql(_)) | None => return ok(Value::Null),
                }
            };
        }
        // The document by kind.
        macro_rules! open {
            ($uri:expr) => {
                match self.session().doc($uri.as_str()) {
                    Some(doc) => doc,
                    None => return ok(Value::Null),
                }
            };
        }
        let json =
            |v: serde_json::Result<Value>| v.map_err(|e| (ErrorCode::InternalError, e.to_string()));

        match req.method.as_str() {
            Completion::METHOD => {
                let p = params!(CompletionParams);
                let a = match open!(p.text_document_position.text_document.uri) {
                    ide::Doc::Rdf(a) => a,
                    ide::Doc::Sparql(q) => {
                        return json(serde_json::to_value(sparql_features::completion(
                            &q,
                            self.ws.knowledge(),
                            enc,
                            &p,
                        )));
                    }
                };
                // `?` and `$` start variables, which RDF documents have none
                // of outside the SPARQL queries of SHACL shapes.
                let trigger = p
                    .context
                    .as_ref()
                    .and_then(|c| c.trigger_character.as_deref());
                if matches!(trigger, Some("?" | "$"))
                    && !a
                        .offset(enc, p.text_document_position.position)
                        .is_some_and(|o| embedded::in_query(&a, o))
                {
                    return ok(Value::Null);
                }
                json(serde_json::to_value(completion::complete(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position.position,
                )))
            }
            ResolveCompletionItem::METHOD => {
                let item = params!(CompletionItem);
                let k = self.ws.knowledge();
                let sparql = item.data.as_ref().is_some_and(|d| d["sparql"] == true);
                json(serde_json::to_value(if sparql {
                    sparql_features::resolve_completion(item, k)
                } else {
                    completion::resolve(item, k)
                }))
            }
            HoverRequest::METHOD => {
                let p = params!(HoverParams);
                let at = p.text_document_position_params;
                json(serde_json::to_value(
                    self.session()
                        .hover(at.text_document.uri.as_str(), at.position),
                ))
            }
            GotoDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let at = p.text_document_position_params;
                let locs = self
                    .session()
                    .definition(at.text_document.uri.as_str(), at.position);
                json(serde_json::to_value(
                    locs.map(GotoDefinitionResponse::Array),
                ))
            }
            GotoTypeDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let at = p.text_document_position_params;
                let locs = self
                    .session()
                    .type_definition(at.text_document.uri.as_str(), at.position);
                json(serde_json::to_value(
                    locs.map(GotoDefinitionResponse::Array),
                ))
            }
            GotoImplementation::METHOD => {
                let p = params!(GotoDefinitionParams);
                let at = p.text_document_position_params;
                let locs = self
                    .session()
                    .implementations(at.text_document.uri.as_str(), at.position);
                json(serde_json::to_value(
                    locs.map(GotoDefinitionResponse::Array),
                ))
            }
            References::METHOD => {
                let p = params!(ReferenceParams);
                let at = p.text_document_position;
                json(serde_json::to_value(self.session().references(
                    at.text_document.uri.as_str(),
                    at.position,
                    p.context.include_declaration,
                )))
            }
            DocumentHighlightRequest::METHOD => {
                let p = params!(DocumentHighlightParams);
                let at = p.text_document_position_params;
                json(serde_json::to_value(
                    self.session()
                        .highlights(at.text_document.uri.as_str(), at.position),
                ))
            }
            PrepareRenameRequest::METHOD => {
                let p = params!(TextDocumentPositionParams);
                match self
                    .session()
                    .prepare_rename(p.text_document.uri.as_str(), p.position)
                {
                    Ok(r) => json(serde_json::to_value(r)),
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
                    Ok(edit) => json(serde_json::to_value(edit)),
                    Err(msg) => Err((ErrorCode::InvalidParams, msg)),
                }
            }
            DocumentSymbolRequest::METHOD => {
                let p = params!(DocumentSymbolParams);
                let syms = self
                    .session()
                    .document_symbols(p.text_document.uri.as_str());
                json(serde_json::to_value(
                    syms.map(DocumentSymbolResponse::Nested),
                ))
            }
            WorkspaceSymbolRequest::METHOD => {
                let p = params!(WorkspaceSymbolParams);
                json(serde_json::to_value(WorkspaceSymbolResponse::Flat(
                    self.session().workspace_symbols(&p.query),
                )))
            }
            FoldingRangeRequest::METHOD => {
                let p = params!(FoldingRangeParams);
                json(serde_json::to_value(
                    self.session().folding_ranges(p.text_document.uri.as_str()),
                ))
            }
            DocumentDiagnosticRequest::METHOD => {
                let p = params!(DocumentDiagnosticParams);
                json(serde_json::to_value(self.pull_diagnostics(p)))
            }
            CodeActionRequest::METHOD => {
                let p = params!(CodeActionParams);
                let a = match open!(p.text_document.uri) {
                    ide::Doc::Rdf(a) => a,
                    ide::Doc::Sparql(q) => {
                        return json(serde_json::to_value(sparql_features::code_actions(
                            &q,
                            self.ws.knowledge(),
                            enc,
                            &p,
                            self.resolve_edits,
                        )));
                    }
                };
                let only = p.context.only.as_deref();
                let mut actions: Vec<_> = quickfix::code_actions(&a, self.ws.knowledge(), enc, &p)
                    .into_iter()
                    .filter(|c| match c {
                        CodeActionOrCommand::CodeAction(c) => {
                            c.kind.as_ref().is_none_or(|k| refactor::wanted(only, k))
                        }
                        CodeActionOrCommand::Command(_) => true,
                    })
                    .collect();
                actions.extend(refactor::code_actions(&a, enc, &p, self.resolve_edits));
                actions.extend(crate::imports::fetch_actions(&a, enc, &p));
                json(serde_json::to_value(actions))
            }
            ExecuteCommand::METHOD => {
                match crate::commands::parse(params!(ExecuteCommandParams))? {
                    // The job starts when this batch ends; its answer
                    // republishes diagnostics like any other resolution pass.
                    crate::commands::Command::FetchImport { iri } => {
                        self.imports.fetch(&iri);
                        ok(Value::Null)
                    }
                    // Answered when the query is done (see `run_query`).
                    crate::commands::Command::RunQuery { uri } => {
                        match self.run_query(req.id.clone(), &uri)? {
                            Some(v) => ok(v),
                            None => {
                                self.answer_later = true;
                                ok(Value::Null)
                            }
                        }
                    }
                }
            }
            CodeActionResolveRequest::METHOD => {
                let action = params!(CodeAction);
                let Some(uri) = action
                    .data
                    .as_ref()
                    .and_then(|d| d["uri"].as_str())
                    .map(normalize_uri)
                else {
                    return json(serde_json::to_value(action));
                };
                if let Some(q) = self.ws.sparql(&uri) {
                    return json(serde_json::to_value(sparql_features::resolve_code_action(
                        &q, enc, action,
                    )));
                }
                let Some(a) = self.ws.analysis(&uri) else {
                    return json(serde_json::to_value(action));
                };
                json(serde_json::to_value(refactor::resolve(&a, enc, action)))
            }
            Formatting::METHOD => {
                let p = params!(DocumentFormattingParams);
                let opts =
                    format::Options::from_lsp(&p.options, self.settings.format_line_width as usize);
                json(serde_json::to_value(match open!(p.text_document.uri) {
                    ide::Doc::Rdf(a) => format::formatting(&a, enc, &opts),
                    ide::Doc::Sparql(q) => sparql_features::formatting(&q, enc, &opts),
                }))
            }
            TypeHierarchyPrepare::METHOD => {
                let p = params!(TypeHierarchyPrepareParams);
                let a = doc!(p.text_document_position_params.text_document.uri);
                json(serde_json::to_value(hierarchy::prepare(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position_params.position,
                )))
            }
            TypeHierarchySupertypes::METHOD | TypeHierarchySubtypes::METHOD => {
                let item = params!(TypeHierarchySupertypesParams).item;
                let mut s = self.session();
                json(serde_json::to_value(
                    if req.method == TypeHierarchySupertypes::METHOD {
                        s.supertypes(&item)
                    } else {
                        s.subtypes(&item)
                    },
                ))
            }
            SemanticTokensFullRequest::METHOD => {
                let p = params!(SemanticTokensParams);
                json(serde_json::to_value(
                    self.session()
                        .semantic_tokens_full(p.text_document.uri.as_str()),
                ))
            }
            SemanticTokensRangeRequest::METHOD => {
                let p = params!(SemanticTokensRangeParams);
                json(serde_json::to_value(self.session().semantic_tokens_range(
                    p.text_document.uri.as_str(),
                    p.range,
                )))
            }
            InlayHintRequest::METHOD => {
                let p = params!(InlayHintParams);
                json(serde_json::to_value(
                    self.session()
                        .inlay_hints(p.text_document.uri.as_str(), p.range),
                ))
            }
            CodeLensRequest::METHOD => {
                let p = params!(CodeLensParams);
                json(serde_json::to_value(
                    self.session().code_lenses(p.text_document.uri.as_str()),
                ))
            }
            CodeLensResolve::METHOD => {
                let lens = params!(CodeLens);
                json(serde_json::to_value(self.session().resolve_code_lens(lens)))
            }
            _ => Err((
                ErrorCode::MethodNotFound,
                format!("unhandled method {}", req.method),
            )),
        }
    }

    /// Start validating the workspace in the background (once indexed, when
    /// enabled and some workspace document has shapes); otherwise clear the
    /// results shown.
    fn validate_shacl(&mut self) {
        if !self.indexed {
            return;
        }
        self.shacl_generation += 1;
        let docs: Vec<_> = self
            .ws
            .knowledge()
            .docs()
            .map(|(_, s, a)| (s, a.clone()))
            .collect();
        let wanted = self.settings.shacl_enabled
            && docs.iter().any(|(s, a)| {
                *s != crate::knowledge::Source::External && shacl_validate::has_shapes(a)
            });
        if !wanted {
            if let Some(w) = &self.shacl {
                w.cancel(self.shacl_generation);
            }
            self.timers.clear(Timer::ShaclProgress);
            let token = self.shacl_progress.take();
            self.progress_end(token, "Validation off".into());
            if !self.shacl_diagnostics.is_empty() {
                self.shacl_diagnostics.clear();
                self.knowledge_changed();
            }
            return;
        }
        let job = shacl_validate::Job {
            generation: self.shacl_generation,
            docs,
            max_triples: self.settings.shacl_max_triples,
            enc: self.enc,
            unresolved: self.imports.unresolved(),
            soften: self.settings.imports_unresolved == crate::settings::UnresolvedImports::Soften,
        };
        if self.inline {
            // Nothing else runs meanwhile, so the run is held to a time
            // limit of its own, besides the budget every validation has.
            let deadline = Instant::now() + INLINE_SHACL_LIMIT;
            let diagnostics = shacl_validate::run(&job, &move || Instant::now() >= deadline)
                .unwrap_or_else(|| shacl_stopped(&job));
            self.shacl_outcome(shacl_validate::Outcome {
                generation: job.generation,
                diagnostics,
            });
            return;
        }
        if self.shacl.is_none() {
            let (worker, rx) = shacl_validate::Worker::start();
            self.shacl = Some(worker);
            self.shacl_feed = Feed::open(rx);
        }
        // A superseded run counts as the same validation going on.
        if self.work_done_progress
            && self.shacl_progress.is_none()
            && !self.timers.is_set(Timer::ShaclProgress)
        {
            self.timers
                .set(Timer::ShaclProgress, Instant::now() + SHACL_PROGRESS_DELAY);
        }
        if let Some(w) = &self.shacl {
            w.submit(job);
        }
    }

    fn shacl_outcome(&mut self, o: shacl_validate::Outcome) {
        if o.generation != self.shacl_generation {
            return;
        }
        self.timers.clear(Timer::ShaclProgress);
        if let Some(token) = self.shacl_progress.take() {
            self.progress_end(Some(token), violation_summary(&o.diagnostics));
        }
        if o.diagnostics == self.shacl_diagnostics {
            return;
        }
        self.shacl_diagnostics = o.diagnostics;
        self.knowledge_changed();
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
    outbox: Receiver<Message>,
    ended: bool,
}

impl Embedded {
    /// A server for `initialize` params `params`, with what it sent: the
    /// initialize response first (to request id 0).
    pub fn start(params: Value) -> Result<(Embedded, Vec<Message>), String> {
        let (here, host) = Connection::memory();
        let (mut server, result, _roots) =
            Server::configure(here, params, true).map_err(|e| e.to_string())?;
        let (inbox, inbox_rx) = unbounded();
        server.inbox = inbox_rx;
        let init = Response::new_ok(RequestId::from(0), result);
        server
            .conn
            .sender
            .send(init.into())
            .map_err(|e| e.to_string())?;
        server.start();
        server.merge_index(Ok(Vec::new()));
        let mut embedded = Embedded {
            server,
            inbox,
            outbox: host.receiver,
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

/// What a validation that hit its time limit shows: a warning on each shapes
/// file that the results are missing, not a clean bill of health.
fn shacl_stopped(job: &shacl_validate::Job) -> shacl_validate::ByUri {
    job.docs
        .iter()
        .filter(|(s, a)| *s != crate::knowledge::Source::External && shacl_validate::has_shapes(a))
        .map(|(_, a)| {
            let d = Diagnostic {
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(shacl_validate::SKIPPED_CODE.into())),
                source: Some(diagnostics::SOURCE.to_string()),
                message: format!(
                    "SHACL validation stopped after {} s, its time limit: the results are missing",
                    INLINE_SHACL_LIMIT.as_secs()
                ),
                ..Default::default()
            };
            (a.uri.clone(), vec![d])
        })
        .collect()
}

/// Counts of a validation's results by severity, e.g. "2 errors, 1 warning
/// in 2 files", or "No problems". Skip notices are not results.
fn violation_summary(by_uri: &shacl_validate::ByUri) -> String {
    let is_result = |d: &&Diagnostic| !matches!(&d.code, Some(NumberOrString::String(c)) if c == shacl_validate::SKIPPED_CODE);
    let (mut errors, mut warnings, mut info, mut files) = (0, 0, 0, 0);
    for diags in by_uri.values() {
        let mut any = false;
        for d in diags.iter().filter(is_result) {
            any = true;
            match d.severity {
                Some(DiagnosticSeverity::WARNING) => warnings += 1,
                Some(DiagnosticSeverity::INFORMATION) | Some(DiagnosticSeverity::HINT) => info += 1,
                _ => errors += 1,
            }
        }
        files += usize::from(any);
    }
    let plural = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let parts: Vec<String> = [
        plural(errors, "error", "errors"),
        plural(warnings, "warning", "warnings"),
        plural(info, "info", "info"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return "No problems".into();
    }
    let files = if files == 1 {
        "1 file".to_string()
    } else {
        format!("{files} files")
    };
    format!("{} in {files}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::guard;

    #[test]
    fn violation_summary_counts_results_and_files() {
        use super::{Diagnostic, NumberOrString, violation_summary};
        use lsp_types::DiagnosticSeverity as S;
        let d = |code: &str, severity: S| Diagnostic {
            code: Some(NumberOrString::String(code.into())),
            severity: Some(severity),
            ..Default::default()
        };
        let mut by = super::shacl_validate::ByUri::default();
        assert_eq!(violation_summary(&by), "No problems");
        by.insert("a".into(), vec![d("shacl-skipped", S::INFORMATION)]);
        assert_eq!(violation_summary(&by), "No problems");
        by.insert("b".into(), vec![d("shacl-violation", S::ERROR)]);
        assert_eq!(violation_summary(&by), "1 error in 1 file");
        by.insert(
            "c".into(),
            vec![
                d("shacl-violation", S::ERROR),
                d("shacl-violation", S::WARNING),
            ],
        );
        assert_eq!(violation_summary(&by), "2 errors, 1 warning in 2 files");
        by.insert("d".into(), vec![d("shacl-violation", S::INFORMATION)]);
        assert_eq!(
            violation_summary(&by),
            "2 errors, 1 warning, 1 info in 3 files"
        );
    }

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
        let (here, host) = Connection::memory();
        let (mut server, _, _) = Server::configure(
            here,
            json!({"capabilities": {"window": {"workDoneProgress": true}}}),
            false,
        )
        .unwrap();
        assert!(!server.indexed);
        server.index_arrived(Fed::Got(Err(
            "turtle-lsp internal error in index: boom".into()
        )));
        assert!(server.indexed);
        let sent: Vec<Message> = host.receiver.try_iter().collect();
        let warned = sent.iter().any(|m| {
            matches!(m,
            Message::Notification(n) if n.method == "window/showMessage"
                && n.params["message"].as_str().unwrap().contains("boom")
                && n.params["message"].as_str().unwrap().contains("not indexed"))
        });
        assert!(warned, "{sent:?}");
        // A thread that ended without a word is a failure too, not 0 files.
        let (here, host) = Connection::memory();
        let (mut server, _, _) =
            Server::configure(here, json!({"capabilities": {}}), false).unwrap();
        server.index_arrived(Fed::Gone);
        assert!(server.indexed);
        let sent: Vec<Message> = host.receiver.try_iter().collect();
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
        let dir = dir.canonicalize().unwrap();
        let (kept, gone, in_folder) = (
            dir.join("kept.ttl"),
            dir.join("gone.ttl"),
            dir.join("folder/f.ttl"),
        );
        for f in [&kept, &gone, &in_folder] {
            std::fs::write(f, TTL).unwrap();
        }
        let (here, _host) = Connection::memory();
        let root = path_to_uri(&dir).unwrap();
        let (mut server, _, roots) = Server::configure(
            here,
            json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
            false,
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
        let dir = dir.canonicalize().unwrap();
        let (here, _host) = Connection::memory();
        let root = path_to_uri(&dir).unwrap();
        let (mut server, _, _) = Server::configure(
            here,
            json!({"capabilities": {}, "workspaceFolders": [{"uri": root, "name": "w"}]}),
            false,
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
            let (here, _host) = Connection::memory();
            let (mut server, _, _) =
                Server::configure(here, json!({"capabilities": {}}), false).unwrap();
            server.exit_wait = Duration::from_millis(wait_ms);
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
            server.shutdown(req, std::iter::empty()).unwrap()
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

    #[test]
    fn a_validation_that_hits_its_time_limit_says_so_on_the_shapes() {
        use crate::knowledge::Source;
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://e.org/> .\nex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n";
        let a = Arc::new(Analysis::new(
            "file:///w/shapes.ttl".into(),
            text,
            Arc::default(),
            false,
        ));
        let job = shacl_validate::Job {
            generation: 1,
            docs: vec![(Source::Disk, a)],
            max_triples: 1000,
            enc: Encoding::Utf16,
            unresolved: vec![],
            soften: true,
        };
        // Cancelled at once, as a deadline already past would: no results ...
        assert!(shacl_validate::run(&job, &|| true).is_none());
        // ... and not a clean bill of health.
        let stopped = shacl_stopped(&job);
        let d = &stopped["file:///w/shapes.ttl"];
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(d[0].message.contains("stopped") && d[0].message.contains("time limit"));
        assert!(violation_summary(&stopped) == "No problems");
        // Uncancelled it validates.
        assert!(shacl_validate::run(&job, &|| false).is_some());
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
            Dir(d.canonicalize().unwrap())
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A server (not started) for the workspace folder `root`, with the
    /// client's connection end, which must outlive it.
    fn server_at(root: &std::path::Path, init: Value) -> (Server, Connection) {
        let (here, host) = Connection::memory();
        let root = path_to_uri(root).unwrap();
        let (server, _, _) = Server::configure(
            here,
            json!({
                "capabilities": {},
                "workspaceFolders": [{"uri": root, "name": "w"}],
                "initializationOptions": init,
            }),
            false,
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
        assert_eq!(s.settings.format_line_width, 100, "no turtle section");
        s.notification(configuration(
            json!({"turtle": {"format": {"lineWidth": 80}}}),
        ));
        assert_eq!(s.settings.format_line_width, 80);
    }

    /// A key the user removed is not kept from `initializationOptions`: the
    /// client's latest section replaces its start-up snapshot, and the key
    /// comes from turtle-lsp.json again (or the default).
    #[test]
    fn a_configuration_section_replaces_the_initialization_options() {
        let dir = Dir::new("layers");
        let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
        assert_eq!(s.settings.format_line_width, 80);
        s.notification(configuration(
            json!({"turtle": {"codeLens": {"references": false}}}),
        ));
        assert_eq!(s.settings.format_line_width, 100);
        assert!(!s.settings.code_lens_references);

        let dir = Dir::new("layers-file");
        std::fs::write(
            dir.0.join(SETTINGS_FILE),
            r#"{"format": {"lineWidth": 90}}"#,
        )
        .unwrap();
        let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
        assert_eq!(s.settings.format_line_width, 80);
        s.notification(configuration(json!({"turtle": {}})));
        assert_eq!(s.settings.format_line_width, 90);
        // A client that only says "something changed" keeps what it sent.
        let (mut s, _client) = server_at(&dir.0, json!({"format": {"lineWidth": 80}}));
        s.notification(configuration(Value::Null));
        assert_eq!(s.settings.format_line_width, 80);
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
            s.reindex_rx
                .recv_timeout(Duration::from_millis(500))
                .is_err()
        );
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
