//! LSP main loop.
//!
//! Single-threaded state, with two kinds of background work:
//! - the initial workspace index is built on a separate thread (in parallel via
//!   rayon) and merged when it arrives, so `initialize` answers immediately;
//! - diagnostics are published only after the incoming queue is drained, so a
//!   burst of keystrokes costs one analysis, not one per keystroke.

use crate::analysis::{Analysis, Encoding};
use crate::features::{completion, diagnostics, hover, navigation, symbols};
use crate::workspace::{
    PREFIX_FILE, Workspace, is_turtle, normalize_uri, path_to_uri, uri_to_path,
};
use crossbeam_channel::{Receiver, select, unbounded};
use lsp_server::{Connection, ErrorCode, Message, Notification, Request, RequestId, Response};
use lsp_types::notification::Notification as _;
use lsp_types::request::Request as _;
use lsp_types::*;
use rustc_hash::FxHashSet;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Server {
    conn: Connection,
    ws: Workspace,
    enc: Encoding,
    dirty: FxHashSet<String>,
    index_rx: Receiver<Vec<(String, Analysis)>>,
    /// The initial workspace index has been merged.
    indexed: bool,
    watch_dynamic: bool,
    next_id: i32,
}

pub fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (conn, io) = Connection::stdio();
    let (id, params) = conn.initialize_start()?;
    let params: InitializeParams = serde_json::from_value(params)?;

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
    let mut ws = Workspace::new(roots.clone(), Default::default());
    if let Some(Value::Object(p)) = params
        .initialization_options
        .as_ref()
        .and_then(|o| o.get("prefixes"))
    {
        for (k, v) in p {
            if let Some(v) = v.as_str() {
                ws.settings_prefixes
                    .insert(k.trim_end_matches(':').to_string(), v.to_string());
            }
        }
    }

    let result = InitializeResult {
        capabilities: capabilities(enc),
        server_info: Some(ServerInfo {
            name: "turtle-lsp".into(),
            version: Some(env!("CARGO_PKG_VERSION").into()),
        }),
    };
    conn.initialize_finish(id, serde_json::to_value(result)?)?;

    // Index the workspace in the background.
    let (tx, index_rx) = unbounded();
    let settings = ws.settings_prefixes.clone();
    std::thread::spawn(move || {
        let mut tmp = Workspace::new(roots.clone(), settings);
        let files = Workspace::scan_roots(&roots);
        tmp.index_paths(files);
        let out: Vec<(String, Analysis)> = tmp
            .disk
            .into_iter()
            .filter_map(|(u, a)| Arc::try_unwrap(a).ok().map(|a| (u, a)))
            .collect();
        let _ = tx.send(out);
    });

    let mut server = Server {
        conn,
        ws,
        enc,
        dirty: FxHashSet::default(),
        index_rx,
        indexed: false,
        watch_dynamic,
        next_id: 0,
    };
    server.main_loop()?;
    io.join()?;
    Ok(())
}

fn capabilities(enc: Encoding) -> ServerCapabilities {
    ServerCapabilities {
        position_encoding: Some(match enc {
            Encoding::Utf8 => PositionEncodingKind::UTF8,
            Encoding::Utf16 => PositionEncodingKind::UTF16,
        }),
        text_document_sync: Some(TextDocumentSyncCapability::Kind(
            TextDocumentSyncKind::INCREMENTAL,
        )),
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
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX]),
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

impl Server {
    fn main_loop(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.register_watchers();
        loop {
            let msg = select! {
                recv(self.conn.receiver) -> m => match m { Ok(m) => m, Err(_) => return Ok(()) },
                recv(self.index_rx) -> files => {
                    self.merge_index(files.ok());
                    continue;
                }
            };
            if self.handle(msg)? {
                return Ok(());
            }
            // Drain whatever else is queued before doing the expensive part.
            while let Ok(msg) = self.conn.receiver.try_recv() {
                if self.handle(msg)? {
                    return Ok(());
                }
            }
            self.publish_dirty();
        }
    }

    /// Returns true on exit.
    fn handle(&mut self, msg: Message) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        match msg {
            Message::Request(req) => {
                if self.conn.handle_shutdown(&req)? {
                    return Ok(true);
                }
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
            Message::Response(_) => {}
        }
        Ok(false)
    }

    fn notification(&mut self, n: Notification) {
        use lsp_types::notification::*;
        match n.method.as_str() {
            DidOpenTextDocument::METHOD => {
                if let Some(p) = parse::<DidOpenTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    self.ws
                        .open_doc(uri.clone(), p.text_document.text, p.text_document.version);
                    self.after_edit(&uri);
                }
            }
            DidChangeTextDocument::METHOD => {
                if let Some(p) = parse::<DidChangeTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    self.ws
                        .change_doc(&uri, p.text_document.version, p.content_changes, self.enc);
                    self.after_edit(&uri);
                }
            }
            DidCloseTextDocument::METHOD => {
                if let Some(p) = parse::<DidCloseTextDocumentParams>(n.params) {
                    let uri = normalize_uri(p.text_document.uri.as_str());
                    self.ws.close_doc(&uri);
                    self.dirty.remove(&uri);
                    // Clear diagnostics for the closed buffer.
                    self.send_diagnostics(&uri, vec![], None);
                }
            }
            DidChangeWatchedFiles::METHOD => {
                if let Some(p) = parse::<DidChangeWatchedFilesParams>(n.params) {
                    let mut prefixes_changed = false;
                    for ch in p.changes {
                        let Some(path) = uri_to_path(ch.uri.as_str()) else {
                            continue;
                        };
                        if !is_turtle(&path) {
                            continue;
                        }
                        prefixes_changed |= path.file_name().is_some_and(|f| f == PREFIX_FILE);
                        if !self.ws.open.contains_key(&normalize_uri(ch.uri.as_str())) {
                            self.ws.reindex_disk_file(&path);
                        }
                    }
                    if prefixes_changed {
                        self.ws.invalidate_prefixes();
                        self.dirty.extend(self.ws.open.keys().cloned());
                    }
                }
            }
            _ => {}
        }
    }

    fn merge_index(&mut self, files: Option<Vec<(String, Analysis)>>) {
        for (uri, a) in files.into_iter().flatten() {
            self.ws.disk.entry(uri).or_insert_with(|| Arc::new(a));
        }
        self.indexed = true;
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
            self.ws.invalidate_prefixes();
            self.dirty.extend(self.ws.open.keys().cloned());
        }
        self.dirty.insert(uri.to_string());
    }

    fn publish_dirty(&mut self) {
        let dirty: Vec<String> = self.dirty.drain().collect();
        for uri in dirty {
            let version = self.ws.open.get(&uri).map(|d| d.version);
            let enc = self.enc;
            let diags = guard("diagnostics", || {
                self.ws
                    .analysis(&uri)
                    .map(|a| diagnostics::diagnostics(&a, enc))
            });
            match diags {
                Ok(Some(d)) => self.send_diagnostics(&uri, d, version),
                Ok(None) => {}
                // Tell the user instead of silently showing nothing.
                Err(msg) => {
                    self.send_diagnostics(&uri, vec![internal_error_diagnostic(&msg)], version)
                }
            }
        }
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

    fn register_watchers(&mut self) {
        if !self.watch_dynamic {
            return;
        }
        let opts = DidChangeWatchedFilesRegistrationOptions {
            watchers: vec![FileSystemWatcher {
                glob_pattern: GlobPattern::String("**/*.{ttl,turtle}".into()),
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
        self.next_id += 1;
        let req = Request::new(
            RequestId::from(format!("turtle-{}", self.next_id)),
            lsp_types::request::RegisterCapability::METHOD.into(),
            params,
        );
        let _ = self.conn.sender.send(Message::Request(req));
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
                let all = self.ws.all();
                let text = a.text.clone();
                json(serde_json::to_value(completion::complete(
                    &a,
                    &text,
                    &all,
                    enc,
                    p.text_document_position.position,
                )))
            }
            ResolveCompletionItem::METHOD => {
                let item = params!(CompletionItem);
                let all = self.ws.all();
                json(serde_json::to_value(completion::resolve(item, &all)))
            }
            HoverRequest::METHOD => {
                let p = params!(HoverParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                let all = self.ws.all();
                json(serde_json::to_value(hover::hover(
                    &a,
                    &all,
                    enc,
                    p.text_document_position_params.position,
                )))
            }
            GotoDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let (u, a) = doc!(p.text_document_position_params.text_document.uri);
                let all = self.ws.all();
                let prefix_file = self.prefix_file_for(&u);
                let locs = navigation::definition(
                    &a,
                    &all,
                    enc,
                    p.text_document_position_params.position,
                    prefix_file,
                );
                json(serde_json::to_value(GotoDefinitionResponse::Array(locs)))
            }
            GotoTypeDefinition::METHOD => {
                let p = params!(GotoDefinitionParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                let all = self.ws.all();
                let locs = navigation::type_definition(
                    &a,
                    &all,
                    enc,
                    p.text_document_position_params.position,
                );
                json(serde_json::to_value(GotoDefinitionResponse::Array(locs)))
            }
            GotoImplementation::METHOD => {
                let p = params!(GotoDefinitionParams);
                let (_, a) = doc!(p.text_document_position_params.text_document.uri);
                let all = self.ws.all();
                let locs = navigation::implementations(
                    &a,
                    &all,
                    enc,
                    p.text_document_position_params.position,
                );
                json(serde_json::to_value(GotoDefinitionResponse::Array(locs)))
            }
            References::METHOD => {
                let p = params!(ReferenceParams);
                let (_, a) = doc!(p.text_document_position.text_document.uri);
                let all = self.ws.all();
                json(serde_json::to_value(navigation::references(
                    &a,
                    &all,
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
                let all = self.ws.all();
                match navigation::rename(
                    &a,
                    &all,
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
                let all = self.ws.all();
                json(serde_json::to_value(WorkspaceSymbolResponse::Flat(
                    symbols::workspace_symbols(&all, enc, &p.query),
                )))
            }
            FoldingRangeRequest::METHOD => {
                let p = params!(FoldingRangeParams);
                let (_, a) = doc!(p.text_document.uri);
                json(serde_json::to_value(symbols::folding_ranges(&a)))
            }
            CodeActionRequest::METHOD => {
                let p = params!(CodeActionParams);
                let (_, a) = doc!(p.text_document.uri);
                let text = a.text.clone();
                json(serde_json::to_value(diagnostics::code_actions(
                    &a, &text, enc, &p,
                )))
            }
            _ => Err((
                ErrorCode::MethodNotFound,
                format!("unhandled method {}", req.method),
            )),
        }
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
