//! LSP main loop.
//!
//! Single-threaded state, with two kinds of background work:
//! - the initial workspace index is built on a separate thread (in parallel via
//!   rayon) and merged when it arrives, so `initialize` answers immediately;
//! - diagnostics are published only after the incoming queue is drained, so a
//!   burst of keystrokes costs one analysis, not one per keystroke.
//!
//! The queue is drained as one batch before anything is answered, so a request
//! whose `$/cancelRequest` is already queued is never computed.
//!
//! Diagnostics are pushed (`publishDiagnostics`), or pulled
//! (`textDocument/diagnostic`) when the client supports that. Pulled reports
//! carry a result id made of the document version and the knowledge
//! generation: anything outside the document that can change its diagnostics
//! (prefixes.ttl, the disk index, settings) bumps the generation.

use crate::analysis::{Analysis, Encoding};
use crate::features::hierarchy;
use crate::features::{
    completion, diagnostics, format, hover, inlay, navigation, refactor, semantic, shacl_validate,
    symbols,
};
use crate::settings::Settings;
use crate::workspace::{
    PREFIX_FILE, Workspace, is_turtle, normalize_uri, path_to_uri, uri_to_path,
};
use crossbeam_channel::{Receiver, never, select, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::*;
use rustc_hash::FxHashSet;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Files that feed `owl:imports` without being Turtle documents themselves.
fn is_import_source(path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|f| f == "catalog-v001.xml")
        || path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("rdf") || e.eq_ignore_ascii_case("owl"))
}

/// Pause after a described-set change before other documents are refreshed.
const DESCRIBED_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(300);

pub struct Server {
    conn: Connection,
    /// Every message read so far. lsp-server hands over messages through a
    /// rendezvous channel, so without this queue a batch would never hold
    /// more than one message.
    inbox: Receiver<Message>,
    ws: Workspace,
    enc: Encoding,
    dirty: FxHashSet<String>,
    index_rx: Receiver<Vec<(String, Analysis)>>,
    /// The initial workspace index has been merged.
    indexed: bool,
    watch_dynamic: bool,
    /// The client resolves a code action's edit lazily (`codeAction/resolve`).
    resolve_edits: bool,
    next_id: i32,
    /// `initializationOptions` and the last `turtle` configuration section;
    /// `settings` is the defaults overridden by these two, in that order.
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
    /// When to act on a described-set change (see `check_described`).
    described_due: Option<std::time::Instant>,
    /// `workspace/{inlayHint,semanticTokens}/refresh`, those the client supports.
    display_refresh: Vec<&'static str>,
    /// Knowledge changed since the last display refresh.
    display_stale: bool,
    imports: crate::imports::Imports,
    /// SHACL validation thread, started when the workspace first has shapes.
    shacl: Option<shacl_validate::Worker>,
    shacl_rx: Receiver<shacl_validate::Outcome>,
    /// The latest validation job; older results are dropped.
    shacl_generation: u64,
    /// The latest validation results per document.
    shacl_diagnostics: shacl_validate::ByUri,
}

/// Returns whether the session ended cleanly (`shutdown` then `exit`); the
/// process should exit with code 1 otherwise, as the LSP spec asks.
pub fn run() -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    // Workspace files are parsed on rayon threads; give them room beyond the
    // 2 MB default (parser nesting is bounded, this is defence in depth).
    let _ = rayon::ThreadPoolBuilder::new()
        .stack_size(8 << 20)
        .build_global();
    let (conn, io) = Connection::stdio();
    let (id, raw) = conn.initialize_start()?;
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
    ]
    .into_iter()
    .filter(|(k, _)| {
        raw.pointer(&format!("/capabilities/workspace/{k}/refreshSupport"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    })
    .map(|(_, m)| m)
    .collect();
    let config_pull = params
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.configuration)
        .unwrap_or(false);
    let init_options = params.initialization_options.clone().unwrap_or_default();
    let settings = Settings::from_layers([&init_options]);

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
    let ws = Workspace::new(roots.clone(), settings.prefixes.clone());

    let result = InitializeResult {
        capabilities: capabilities(enc, pull),
        server_info: Some(ServerInfo {
            name: "turtle-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    let mut result = serde_json::to_value(result)?;
    // lsp-types 0.97 has no field for it.
    result["capabilities"]["typeHierarchyProvider"] = Value::Bool(true);
    conn.initialize_finish(id, result)?;

    // Index the workspace in the background.
    let (tx, index_rx) = unbounded();
    let prefixes = ws.settings_prefixes.clone();
    std::thread::spawn(move || {
        let mut tmp = Workspace::new(roots.clone(), prefixes);
        let files = Workspace::scan_roots(&roots);
        let _ = tx.send(tmp.parse_paths(files));
    });

    let (inbox_tx, inbox) = unbounded();
    let incoming = conn.receiver.clone();
    std::thread::spawn(move || {
        for msg in incoming {
            if inbox_tx.send(msg).is_err() {
                break;
            }
        }
    });

    let mut server = Server {
        conn,
        inbox,
        ws,
        enc,
        dirty: FxHashSet::default(),
        index_rx,
        indexed: false,
        watch_dynamic,
        resolve_edits,
        next_id: 0,
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
        described_due: None,
        display_refresh,
        display_stale: false,
        imports: crate::imports::Imports::new(),
        shacl: None,
        shacl_rx: never(),
        shacl_generation: 0,
        shacl_diagnostics: Default::default(),
    };
    let clean = server.main_loop()?;
    // The writer thread ends only when every sender is gone; joining while the
    // server still holds one would keep the process alive forever after the
    // editor disappears (stdin closed without shutdown/exit).
    drop(server);
    io.join()?;
    Ok(clean)
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
            trigger_characters: Some(vec![":".into(), "@".into()]),
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
                legend: semantic::legend(),
                range: Some(true),
                full: Some(SemanticTokensFullOptions::Bool(true)),
                work_done_progress_options: Default::default(),
            },
        )),
        inlay_hint_provider: Some(OneOf::Left(true)),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![
                CodeActionKind::QUICKFIX,
                CodeActionKind::SOURCE_ORGANIZE_IMPORTS,
                CodeActionKind::REFACTOR_REWRITE,
            ]),
            resolve_provider: Some(true),
            ..Default::default()
        })),
        ..Default::default()
    }
}

/// Run `f`, turning a panic into an error message. The default panic hook has
/// already printed the details to stderr (stdout carries the protocol).
fn guard<T>(what: &str, f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| {
        let detail = e
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| e.downcast_ref::<String>().cloned())
            .unwrap_or_default();
        format!("turtle-lsp internal error in {what}: {detail}")
    })
}

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
    /// Returns true after `shutdown` + `exit`, false when the client went away
    /// or sent `exit` without `shutdown`.
    fn main_loop(&mut self) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.register_watchers();
        loop {
            let first = select! {
                recv(self.inbox) -> m => match m { Ok(m) => Some(m), Err(_) => return Ok(false) },
                recv(self.index_rx) -> files => {
                    self.merge_index(files.ok());
                    None
                }
                recv(self.described_due.map_or_else(never, crossbeam_channel::at)) -> _ => {
                    self.described_settled();
                    None
                }
                recv(self.imports.results()) -> done => {
                    if let Ok(done) = done {
                        self.imports_done(done);
                    }
                    None
                }
                recv(self.shacl_rx) -> outcome => {
                    match outcome {
                        Ok(o) => self.shacl_outcome(o),
                        // The thread is gone: a disconnected channel is always ready.
                        Err(_) => {
                            self.shacl = None;
                            self.shacl_rx = never();
                        }
                    }
                    None
                }
            };
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
                        return self.shutdown(req, rest.into_iter());
                    }
                    Message::Notification(n)
                        if n.method == lsp_types::notification::Exit::METHOD =>
                    {
                        return Ok(false);
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
            self.check_described();
            self.publish_dirty();
            self.sync_imports();
            self.refresh_if_needed();
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
            self.described_due = Some(std::time::Instant::now() + DESCRIBED_DEBOUNCE);
        }
    }

    /// The pause after a described-set change has passed.
    fn described_settled(&mut self) {
        self.described_due = None;
        let g = self.ws.knowledge().described_generation();
        if g != self.described_seen {
            self.described_seen = g;
            self.knowledge_changed();
        }
    }

    /// Answer `shutdown`, then wait for `exit`; other requests are refused.
    /// Returns whether `exit` arrived.
    fn shutdown(
        &self,
        req: Request,
        rest: impl Iterator<Item = Message>,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        self.conn.sender.send(Response::new_ok(req.id, ()).into())?;
        let mut rest = rest.map(Ok);
        let receiver = &self.inbox;
        let mut next = || {
            rest.next()
                .unwrap_or_else(|| receiver.recv_timeout(Duration::from_secs(30)))
        };
        while let Ok(msg) = next() {
            match msg {
                Message::Notification(n) if n.method == lsp_types::notification::Exit::METHOD => {
                    return Ok(true);
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
        Ok(false)
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
                    let syntax = crate::syntax::Syntax::detect(Some(&doc.language_id), &uri);
                    self.ws
                        .open_doc_as(uri.clone(), doc.text, doc.version, syntax);
                    self.after_edit(&uri);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Some(p) = parse::<DidChangeTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    self.ws
                        .change_doc(&uri, p.text_document.version, p.content_changes, self.enc);
                    // SHACL results stay until the next save (they describe the
                    // saved text, like a build-on-save checker's).
                    self.after_edit(&uri);
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
                    self.ws.close_doc(&uri);
                    self.dirty.remove(&uri);
                    // Unsaved edits to a prefixes.ttl are gone with the buffer.
                    if uri.ends_with(&format!("/{PREFIX_FILE}")) {
                        self.ws.invalidate_prefixes();
                        self.knowledge_changed();
                    }
                    // Clear diagnostics for the closed buffer.
                    if !self.pull {
                        self.send_diagnostics(&uri, vec![], None);
                    }
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
                    let mut prefixes_changed = false;
                    let mut disk_changed = false;
                    for ch in p.changes {
                        let Some(path) = uri_to_path(ch.uri.as_str()) else {
                            continue;
                        };
                        if is_import_source(&path) {
                            // A catalog or an imported RDF/XML ontology changed.
                            self.imports.retry();
                            continue;
                        }
                        if !is_turtle(&path) {
                            continue;
                        }
                        let open = self.ws.open.contains_key(&normalize_uri(ch.uri.as_str()));
                        // An open prefixes.ttl was handled on save.
                        prefixes_changed |=
                            !open && path.file_name().is_some_and(|f| f == PREFIX_FILE);
                        if !open {
                            self.ws.reindex_disk_file(&path);
                            disk_changed = true;
                        }
                    }
                    if prefixes_changed {
                        self.ws.invalidate_prefixes();
                        self.dirty.extend(self.ws.open.keys().cloned());
                    }
                    if prefixes_changed || disk_changed {
                        self.knowledge_changed();
                        self.validate_shacl();
                    }
                }
            }
            _ => {}
        }
    }

    /// A document was saved: a catalog or ontology file may have changed too.
    fn on_save(&mut self, uri: &str) {
        if uri.ends_with(&format!("/{PREFIX_FILE}")) {
            self.ws.invalidate_prefixes();
            self.knowledge_changed();
        }
        self.imports.retry();
        self.validate_shacl();
    }

    /// Start resolving `owl:imports` if they changed (never before the
    /// initial index, which may describe the imported ontologies).
    fn sync_imports(&mut self) {
        if !self.indexed {
            return;
        }
        let roots = self.ws.roots.clone();
        let network = self.settings.imports_network;
        let k = self.ws.knowledge();
        if let Some(done) = self.imports.sync(k, &roots, network) {
            self.imports_done(done);
        }
    }

    fn imports_done(&mut self, done: crate::imports::Done) {
        if self.imports.apply(done, self.ws.knowledge_mut()) {
            self.knowledge_changed();
        }
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

    /// The client's `turtle` configuration section changed.
    fn set_config(&mut self, section: Value) {
        if section.is_null() {
            return;
        }
        self.config = section;
        let new = Settings::from_layers([&self.init_options, &self.config]);
        if new == self.settings {
            return;
        }
        if new.prefixes != self.settings.prefixes {
            // The background index was built with the old prefixes.
            self.ensure_indexed();
            self.ws.settings_prefixes = new.prefixes.clone();
            self.ws.invalidate_prefixes();
        }
        self.settings = new;
        self.dirty.extend(self.ws.open.keys().cloned());
        self.knowledge_changed();
        self.validate_shacl();
    }

    /// Something outside any one document changed that can change its
    /// diagnostics: pulled reports handed out so far are stale.
    fn knowledge_changed(&mut self) {
        self.generation += 1;
        // Diagnostics read the knowledge index (add-prefix candidates, lints),
        // so every open document is due again.
        self.dirty.extend(self.ws.open.keys().cloned());
        if self.reported {
            self.refresh_needed = true;
        }
        // Labels and term kinds shown in open documents may come from elsewhere.
        self.display_stale = true;
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

    fn merge_index(&mut self, files: Option<Vec<(String, Analysis)>>) {
        self.ws.add_disk(files.unwrap_or_default(), false);
        self.indexed = true;
        // The sender is gone now; a disconnected channel is always ready, which
        // would wake the main loop forever.
        self.index_rx = never();
        self.knowledge_changed();
        self.validate_shacl();
    }

    /// Answers that look across the workspace (definition, references, rename,
    /// ...) must not come from a half-built index: a rename sent right after
    /// start-up would silently miss files. Wait for the initial index once.
    fn ensure_indexed(&mut self) {
        if !self.indexed {
            let files = self.index_rx.recv().ok();
            self.merge_index(files);
        }
    }

    fn after_edit(&mut self, uri: &str) {
        if uri.ends_with(&format!("/{PREFIX_FILE}")) {
            // Open documents follow the edit; indexed files are re-read when
            // it is saved (re-reading the workspace per keystroke stalls).
            self.ws.invalidate_open_prefixes();
            self.dirty.extend(self.ws.open.keys().cloned());
            self.knowledge_changed();
        }
        self.dirty.insert(uri.to_string());
    }

    /// Diagnostics for `uri`, or the panic message if computing them failed.
    fn diagnostics_for(&mut self, uri: &str) -> Result<Option<Vec<Diagnostic>>, String> {
        let enc = self.enc;
        let indexed = self.indexed;
        let opts = diagnostics::Options::from_settings(&self.settings);
        guard("diagnostics", || {
            self.ws.analysis(uri).map(|a| {
                let mut d = diagnostics::diagnostics_with(&a, self.ws.knowledge(), enc, &opts);
                // "Known nowhere" findings wait for the initial index.
                if !indexed {
                    d.retain(|d| !diagnostics::needs_index(d));
                }
                d.extend(self.imports.diagnostics(&a, enc));
                if let Some(shacl) = self.shacl_diagnostics.get(uri) {
                    d.extend(shacl.iter().cloned());
                }
                d
            })
        })
    }

    fn publish_dirty(&mut self) {
        let dirty: Vec<String> = self.dirty.drain().collect();
        if self.pull {
            return;
        }
        for uri in dirty {
            let version = self.ws.open.get(&uri).map(|d| d.version);
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
            .open
            .get(&uri)
            .map_or_else(|| "disk".to_string(), |d| d.version.to_string());
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
            uri: navigation::uri(uri),
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
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String(
                    "**/{*.ttl,*.turtle,*.trig,*.nt,*.rdf,*.owl,catalog-v001.xml}".into(),
                ),
                kind: None,
            }],
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
        macro_rules! doc {
            ($uri:expr) => {{
                let u = normalize_uri($uri.as_str());
                match self.ws.analysis(&u) {
                    Some(a) => (u, a),
                    None => return ok(Value::Null),
                }
            }};
        }
        let json =
            |v: serde_json::Result<Value>| v.map_err(|e| (ErrorCode::InternalError, e.to_string()));

        match req.method.as_str() {
            Completion::METHOD => {
                let p = params!(CompletionParams);
                let (_, a) = doc!(p.text_document_position.text_document.uri);
                let text = a.text.clone();
                json(serde_json::to_value(completion::complete(
                    &a,
                    &text,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position.position,
                )))
            }
            ResolveCompletionItem::METHOD => {
                let item = params!(CompletionItem);
                json(serde_json::to_value(completion::resolve(
                    item,
                    self.ws.knowledge(),
                )))
            }
            HoverRequest::METHOD => {
                let p = params!(HoverParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                json(serde_json::to_value(hover::hover(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position_params.position,
                )))
            }
            GotoDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let (u, a) = doc!(p.text_document_position_params.text_document.uri);
                let prefix_file = self.prefix_file_for(&u);
                let locs = navigation::definition(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position_params.position,
                    prefix_file,
                );
                json(serde_json::to_value(GotoDefinitionResponse::Array(locs)))
            }
            GotoTypeDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                let locs = navigation::type_definition(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position_params.position,
                );
                json(serde_json::to_value(GotoDefinitionResponse::Array(locs)))
            }
            GotoImplementation::METHOD => {
                let p = params!(GotoDefinitionParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                let locs = navigation::implementations(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position_params.position,
                );
                json(serde_json::to_value(GotoDefinitionResponse::Array(locs)))
            }
            References::METHOD => {
                let p = params!(ReferenceParams);
                let (_, a) = doc!(p.text_document_position.text_document.uri);
                json(serde_json::to_value(navigation::references(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position.position,
                    p.context.include_declaration,
                )))
            }
            DocumentHighlightRequest::METHOD => {
                let p = params!(DocumentHighlightParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                json(serde_json::to_value(navigation::highlights(
                    &a,
                    enc,
                    p.text_document_position_params.position,
                )))
            }
            PrepareRenameRequest::METHOD => {
                let p = params!(TextDocumentPositionParams);
                let (_, a) = doc!(p.text_document.uri);
                json(serde_json::to_value(navigation::prepare_rename(
                    &a, enc, p.position,
                )))
            }
            Rename::METHOD => {
                let p = params!(RenameParams);
                let (_, a) = doc!(p.text_document_position.text_document.uri);
                match navigation::rename(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position.position,
                    &p.new_name,
                ) {
                    Ok(edit) => json(serde_json::to_value(edit)),
                    Err(msg) => Err((ErrorCode::InvalidParams, msg)),
                }
            }
            DocumentSymbolRequest::METHOD => {
                let p = params!(DocumentSymbolParams);
                let (_, a) = doc!(p.text_document.uri);
                json(serde_json::to_value(DocumentSymbolResponse::Nested(
                    symbols::document_symbols(&a, &a.text, enc),
                )))
            }
            WorkspaceSymbolRequest::METHOD => {
                let p = params!(WorkspaceSymbolParams);
                json(serde_json::to_value(WorkspaceSymbolResponse::Flat(
                    symbols::workspace_symbols(self.ws.knowledge(), enc, &p.query),
                )))
            }
            FoldingRangeRequest::METHOD => {
                let p = params!(FoldingRangeParams);
                let (_, a) = doc!(p.text_document.uri);
                json(serde_json::to_value(symbols::folding_ranges(&a)))
            }
            DocumentDiagnosticRequest::METHOD => {
                let p = params!(DocumentDiagnosticParams);
                json(serde_json::to_value(self.pull_diagnostics(p)))
            }
            CodeActionRequest::METHOD => {
                let p = params!(CodeActionParams);
                let (_, a) = doc!(p.text_document.uri);
                let text = a.text.clone();
                let only = p.context.only.as_deref();
                let mut actions: Vec<_> =
                    diagnostics::code_actions(&a, self.ws.knowledge(), &text, enc, &p)
                        .into_iter()
                        .filter(|c| match c {
                            CodeActionOrCommand::CodeAction(c) => {
                                c.kind.as_ref().is_none_or(|k| refactor::wanted(only, k))
                            }
                            CodeActionOrCommand::Command(_) => true,
                        })
                        .collect();
                actions.extend(refactor::code_actions(&a, enc, &p, self.resolve_edits));
                json(serde_json::to_value(actions))
            }
            CodeActionResolveRequest::METHOD => {
                let action = params!(CodeAction);
                let uri = action.data.as_ref().and_then(|d| d["uri"].as_str());
                let Some(a) = uri.and_then(|u| self.ws.analysis(&normalize_uri(u))) else {
                    return json(serde_json::to_value(action));
                };
                json(serde_json::to_value(refactor::resolve(&a, enc, action)))
            }
            Formatting::METHOD => {
                let p = params!(DocumentFormattingParams);
                let (_, a) = doc!(p.text_document.uri);
                let opts =
                    format::Options::from_lsp(&p.options, self.settings.format_line_width as usize);
                json(serde_json::to_value(format::formatting(&a, enc, &opts)))
            }
            TypeHierarchyPrepare::METHOD => {
                let p = params!(TypeHierarchyPrepareParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                json(serde_json::to_value(hierarchy::prepare(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.text_document_position_params.position,
                )))
            }
            TypeHierarchySupertypes::METHOD | TypeHierarchySubtypes::METHOD => {
                let item = params!(TypeHierarchySupertypesParams).item;
                let asking =
                    hierarchy::asking_uri(&item).and_then(|u| self.ws.analysis(&normalize_uri(u)));
                let k = self.ws.knowledge();
                json(serde_json::to_value(
                    if req.method == TypeHierarchySupertypes::METHOD {
                        hierarchy::supertypes(&item, k, enc, asking.as_deref())
                    } else {
                        hierarchy::subtypes(&item, k, enc, asking.as_deref())
                    },
                ))
            }
            SemanticTokensFullRequest::METHOD => {
                let p = params!(SemanticTokensParams);
                let (_, a) = doc!(p.text_document.uri);
                json(serde_json::to_value(semantic::full(
                    &a,
                    self.ws.knowledge(),
                    enc,
                )))
            }
            SemanticTokensRangeRequest::METHOD => {
                let p = params!(SemanticTokensRangeParams);
                let (_, a) = doc!(p.text_document.uri);
                json(serde_json::to_value(semantic::range(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.range,
                )))
            }
            InlayHintRequest::METHOD => {
                let p = params!(InlayHintParams);
                let (_, a) = doc!(p.text_document.uri);
                let mode = self.settings.inlay_hint_labels;
                json(serde_json::to_value(inlay::inlay_hints(
                    &a,
                    self.ws.knowledge(),
                    enc,
                    p.range,
                    mode,
                )))
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
            if !self.shacl_diagnostics.is_empty() {
                self.shacl_diagnostics.clear();
                self.knowledge_changed();
            }
            return;
        }
        if self.shacl.is_none() {
            let (worker, rx) = shacl_validate::Worker::start();
            self.shacl = Some(worker);
            self.shacl_rx = rx;
        }
        if let Some(w) = &self.shacl {
            w.submit(shacl_validate::Job {
                generation: self.shacl_generation,
                docs,
                max_triples: self.settings.shacl_max_triples,
                enc: self.enc,
            });
        }
    }

    fn shacl_outcome(&mut self, o: shacl_validate::Outcome) {
        if o.generation != self.shacl_generation || o.diagnostics == self.shacl_diagnostics {
            return;
        }
        self.shacl_diagnostics = o.diagnostics;
        self.knowledge_changed();
    }

    fn prefix_file_for(&mut self, uri: &str) -> Option<Arc<Analysis>> {
        let dir = uri_to_path(uri)?.parent()?.to_path_buf();
        let file = self.ws.find_prefix_file(&dir)?;
        let file_uri = path_to_uri(&file)?;
        if let Some(a) = self.ws.analysis(&file_uri) {
            return Some(a);
        }
        let text = std::fs::read_to_string(&file).ok()?;
        Some(Arc::new(Analysis::new(
            file_uri,
            &text,
            Arc::new(Default::default()),
            false,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::guard;

    #[test]
    fn guard_turns_a_panic_into_an_error() {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let r: Result<(), String> = guard("hover", || panic!("boom"));
        std::panic::set_hook(prev);
        let msg = r.unwrap_err();
        assert!(msg.contains("hover") && msg.contains("boom"), "{msg}");
        assert_eq!(guard("x", || 7), Ok(7));
    }
}
