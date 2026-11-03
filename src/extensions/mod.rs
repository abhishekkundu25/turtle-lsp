//! The requests beyond the LSP standard, each in a file of its own:
//! `turtle/graph` ([`graph`]). The server hands every method [`handles`] to
//! [`request`], and lists them under `experimental` ([`capabilities`]).
//! "▶ Run query" ([`run_query`]) is two commands and a state the server drives.

pub(crate) mod graph;
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
    method == graph::METHOD
}

/// The `experimental` capability: every extension request, by method.
pub(crate) fn capabilities() -> serde_json::Value {
    serde_json::json!({ graph::METHOD: true })
}

/// Answer the extension request `req`; `out` is for a thread that answers it.
pub(crate) fn request(req: Request, spawn: Spawn, out: Sender<Out>, s: &mut Session) -> Answer {
    graph::request(req, spawn, out, s)
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
