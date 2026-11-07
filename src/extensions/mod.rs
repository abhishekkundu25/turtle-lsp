//! The requests beyond the LSP standard, each in a file of its own:
//! `turtle/graph` ([`graph`]), `turtle/graphDiff` ([`graph_diff`]) and
//! `turtle/query` ([`query`]). The server
//! hands every method [`handles`] to [`request`], and lists them under
//! `experimental` ([`capabilities`]). "▶ Run query" ([`run_query`]) is two
//! commands and a state the server drives; so is the [`query::State`] of the
//! `turtle/query` runs going on.

pub(crate) mod graph;
pub(crate) mod graph_diff;
pub(crate) mod query;
pub(crate) mod run_query;

use crate::ide::Session;
use crate::server::wire::Out;
use crate::server::{WORKER_STACK, worker::Spawn};
use crossbeam_channel::Sender;
use lsp_server::{ErrorCode, Request, RequestId, Response};

/// A result as JSON text, or the error to answer instead.
type Json = Result<String, (ErrorCode, String)>;
/// [`Json`], or None when a thread answers the request ([`on_thread`]).
type Answer = Result<Option<String>, (ErrorCode, String)>;

pub(crate) fn handles(method: &str) -> bool {
    [graph::METHOD, graph_diff::METHOD, query::METHOD].contains(&method)
}

/// The `experimental` capability: every extension request, by method.
pub(crate) fn capabilities() -> serde_json::Value {
    serde_json::json!({ graph::METHOD: true, graph_diff::METHOD: true, query::METHOD: true })
}

/// Answer the extension request `req`. `out` is for a thread that answers
/// it; `queries` (the `turtle/query` runs going on) and `root_uris` (the
/// workspace folders as `initialize` named them) are the server's.
pub(crate) fn request(
    req: Request,
    queries: &query::State,
    root_uris: &[String],
    spawn: Spawn,
    out: Sender<Out>,
    s: &mut Session,
) -> Answer {
    match req.method.as_str() {
        query::METHOD => query::request(req, queries, root_uris, spawn, out, s),
        graph_diff::METHOD => graph_diff::request(req, spawn, out, s),
        _ => graph::request(req, spawn, out, s),
    }
}

/// Answer request `id` with what `work` returns, from a thread named `name`:
/// the main loop goes on meanwhile. Nothing is kept: a `$/cancelRequest`
/// does not stop it, and the answer comes when `work` ends.
fn on_thread(
    out: Sender<Out>,
    id: RequestId,
    name: &str,
    work: impl FnOnce() -> Json + Send + 'static,
) -> Answer {
    let deliver = move |answer: Result<Json, String>| {
        let msg = match answer.unwrap_or_else(|e| Err((ErrorCode::InternalError, e))) {
            Ok(json) => Out::Result { id, json },
            Err((code, message)) => {
                Out::Message(Response::new_err(id, code as i32, message).into())
            }
        };
        let _ = out.send(msg);
    };
    match Spawn::Thread.run(name, WORKER_STACK, work, deliver) {
        Ok(()) => Ok(None),
        Err(e) => Err((
            ErrorCode::InternalError,
            format!("turtle-lsp could not start a thread for the {name}: {e}"),
        )),
    }
}
