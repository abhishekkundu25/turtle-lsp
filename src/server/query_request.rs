//! `turtle/query`: run a SPARQL query given as text over the workspace and
//! answer its results as JSON, for another extension or tool, not for a
//! person: nothing is shown and no file is written.
//!
//! The request's [`Params`] are the query text and, optionally, the
//! document URIs to query (`files`; default every RDF document of the
//! workspace, open ones with their unsaved edits, imports excluded), a
//! `base` for relative IRIs, and limits: `timeoutSeconds`, which can only
//! lower `query.timeoutSeconds` (a higher value, or `0`, is lowered to the
//! setting), and `maxRows` (default and `0`: no limit).
//! `query.maxBufferedRows` applies as always.
//!
//! The dataset is [`query::run`]'s, as "▶ Run query" has it: the default
//! graph is the union of every triple of every document; there are no named
//! graphs.
//!
//! The answer is one of
//!
//! - `{"kind": "select", "head": {"vars": [...]}, "results": {"bindings":
//!   [...]}, "omitted": n, "elapsedMs": ms}`: SPARQL 1.1 Query Results JSON
//!   with full IRIs, `omitted` the rows past `maxRows`;
//! - `{"kind": "ask", "head": {}, "boolean": b, "elapsedMs": ms}`;
//! - `{"kind": "construct", "triples": [{"subject", "predicate",
//!   "object"}], "omitted": n, "elapsedMs": ms}` for CONSTRUCT and DESCRIBE,
//!   the terms in the results JSON's term form.
//!
//! A query that is not SPARQL, an UPDATE, or one of `files` that is not a
//! workspace document is `InvalidParams`; a query stopped at a limit or by
//! the evaluator is `RequestFailed`, with the reason as the message. The
//! query runs on a thread of its own ([`Spawn::Thread`]), which answers the
//! request; at most [`MAX_RUNNING`] run at a time, and one more is refused
//! with `RequestFailed`. A `$/cancelRequest` for a pending request stops its
//! run, answered `RequestCanceled`. Embedded ([`Spawn::Inline`]), it runs
//! inline, with no cap and no cancellation.

use super::Server;
use super::worker::Spawn;
use crate::analysis::Analysis;
use crate::clock::Instant;
use crate::knowledge::Source;
use crate::query::render::json_term;
use crate::query::{self, Answer, Limits, QueryError};
use crate::syntax::PrefixMap;
use lsp_server::{ErrorCode, RequestId};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How many `turtle/query` requests run at a time.
pub(crate) const MAX_RUNNING: usize = 4;

/// The params of `turtle/query`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Params {
    /// The SPARQL query (SELECT, ASK, CONSTRUCT or DESCRIBE).
    query: String,
    /// The documents to query, by URI; omitted: every RDF document.
    files: Option<Vec<String>>,
    /// The base IRI for relative IRIs in the query.
    base: Option<String>,
    /// Lowers `query.timeoutSeconds`; never raises it.
    timeout_seconds: Option<f64>,
    /// `0` or omitted: no limit.
    max_rows: Option<u64>,
}

/// The runs going on, by request id: each one's cancellation flag. Shared
/// with the threads, which remove their own run when it ends.
#[derive(Clone, Default)]
pub(crate) struct State {
    running: Arc<Mutex<FxHashMap<RequestId, Arc<AtomicBool>>>>,
}

impl State {
    /// The runs going on.
    fn count(&self) -> usize {
        self.running.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    fn insert(&self, id: RequestId, cancel: Arc<AtomicBool>) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, cancel);
    }

    fn remove(&self, id: &RequestId) -> Option<Arc<AtomicBool>> {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
    }

    /// Stop the run of request `id`, if one goes on. Its thread answers
    /// `RequestCanceled` when the evaluator stops.
    pub(crate) fn cancel(&self, id: &RequestId) {
        if let Some(flag) = self.remove(id) {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// Stop every run (the server is going away).
    pub(crate) fn cancel_all(&self) {
        for flag in self
            .running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
        {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        // The last owner: the threads hold the map, not the state.
        if Arc::strong_count(&self.running) == 1 {
            self.cancel_all();
        }
    }
}

/// One run, as handed to its thread.
struct Job {
    text: String,
    base: Option<String>,
    prefixes: Arc<PrefixMap>,
    docs: Vec<Arc<Analysis>>,
    limits: Limits,
}

impl Server {
    /// `turtle/query` asked by request `id`: Some(answer) when answered now,
    /// None when the answer comes from the thread.
    pub(super) fn query_request(
        &mut self,
        id: RequestId,
        p: Params,
    ) -> Result<Option<Value>, (ErrorCode, String)> {
        let invalid = |m: String| (ErrorCode::InvalidParams, m);
        let setting = self.layers.settings().query_timeout;
        let timeout = match p.timeout_seconds {
            None => setting,
            Some(s) if s.is_finite() && s >= 0.0 => {
                let asked = Duration::try_from_secs_f64(s)
                    .map_err(|e| invalid(format!("timeoutSeconds: {e}")))?;
                // Only lower the setting: `0` and a higher value are the setting.
                match setting {
                    Some(t) if asked.is_zero() || asked > t => Some(t),
                    Some(_) => Some(asked),
                    None => (!asked.is_zero()).then_some(asked),
                }
            }
            Some(s) => {
                return Err(invalid(format!(
                    "timeoutSeconds: {s} is not a number of seconds"
                )));
            }
        };
        let max_rows = match p.max_rows {
            None | Some(0) => None,
            Some(n) => {
                Some(usize::try_from(n).map_err(|_| invalid(format!("maxRows: {n} is too many")))?)
            }
        };
        // The documents asked for, each once, in the order given.
        let wanted: Option<Vec<String>> = p.files.map(|v| {
            let mut seen = FxHashSet::default();
            v.iter()
                .map(|u| crate::workspace::normalize_uri(u))
                .filter(|u| seen.insert(u.clone()))
                .collect()
        });
        let docs: Vec<Arc<Analysis>> = self
            .ws
            .knowledge()
            .sources()
            .filter(|(_, source, _)| *source != Source::External)
            .filter(|(_, _, a)| {
                wanted
                    .as_ref()
                    .is_none_or(|w| w.iter().any(|u| *u == a.uri))
            })
            .map(|(_, _, a)| a.clone())
            .collect();
        if let Some(w) = &wanted {
            let missing: Vec<&str> = w
                .iter()
                .filter(|u| !docs.iter().any(|a| a.uri == **u))
                .map(String::as_str)
                .collect();
            if !missing.is_empty() {
                return Err(invalid(format!(
                    "not a workspace document: {}",
                    missing.join(", ")
                )));
            }
        }
        // The prefixes the query may use undeclared: the project's, as a
        // query file beside the first of `files` gets them, else one at the
        // first workspace root.
        let at = wanted
            .as_ref()
            .and_then(|w| w.first().cloned())
            .or_else(|| {
                self.ws
                    .roots()
                    .first()
                    .and_then(|r| crate::workspace::path_to_uri(r))
            })
            .unwrap_or_default();
        let prefixes = self.ws.external_prefixes_for(&at);
        let job = Job {
            text: p.query,
            base: p.base,
            prefixes,
            docs,
            limits: Limits {
                timeout,
                cancel: None,
                max_rows,
                max_buffered_rows: self.layers.settings().query_max_buffered_rows,
            },
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.spawn == Spawn::Thread {
            return self.query_on_thread(id, job);
        }
        execute(&job).map(Some)
    }

    /// The rest of [`Server::query_request`] on a thread, which answers the
    /// request itself and removes its run from [`State`].
    #[cfg(not(target_arch = "wasm32"))]
    fn query_on_thread(
        &mut self,
        id: RequestId,
        mut job: Job,
    ) -> Result<Option<Value>, (ErrorCode, String)> {
        use super::wire::Out;
        use lsp_server::Response;
        if self.query_requests.count() >= MAX_RUNNING {
            return Err((
                ErrorCode::RequestFailed,
                format!("{MAX_RUNNING} queries are running: try again when one has answered"),
            ));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        job.limits.cancel = Some(cancel.clone());
        let state = self.query_requests.clone();
        state.insert(id.clone(), cancel);
        let out = self.client.sender();
        let done_id = id.clone();
        let started = self.spawn.run(
            "query request",
            super::WORKER_STACK,
            move || {
                execute(&job).and_then(|v| {
                    super::wire::encode(&v).map_err(|e| (ErrorCode::InternalError, e.to_string()))
                })
            },
            move |answer| {
                state.remove(&done_id);
                let msg = match answer.unwrap_or_else(|e| Err((ErrorCode::InternalError, e))) {
                    Ok(json) => Out::Result { id: done_id, json },
                    Err((code, message)) => {
                        Out::Message(Response::new_err(done_id, code as i32, message).into())
                    }
                };
                let _ = out.send(msg);
            },
        );
        if let Err(e) = started {
            self.query_requests.remove(&id);
            return Err((
                ErrorCode::InternalError,
                format!("turtle-lsp could not start a thread for the query: {e}"),
            ));
        }
        Ok(None)
    }
}

/// Run `job`; its answer as JSON, or the error to answer with.
fn execute(job: &Job) -> Result<Value, (ErrorCode, String)> {
    let start = Instant::now();
    let docs: Vec<&Analysis> = job.docs.iter().map(|a| &**a).collect();
    let outcome = query::run(
        &docs,
        &job.text,
        job.base.as_deref(),
        &job.prefixes,
        &job.limits,
    );
    let elapsed = Instant::now().saturating_duration_since(start).as_millis();
    match outcome {
        Ok(a) => {
            let mut v = answer_json(&a);
            if let Value::Object(m) = &mut v {
                m.insert("elapsedMs".into(), (elapsed as u64).into());
            }
            Ok(v)
        }
        Err(QueryError::Cancelled) => Err((ErrorCode::RequestCanceled, "cancelled".into())),
        Err(
            e
            @ (QueryError::Syntax { .. } | QueryError::Unsupported(_) | QueryError::TooComplex(_)),
        ) => Err((ErrorCode::InvalidParams, e.to_string())),
        Err(e) => Err((ErrorCode::RequestFailed, e.to_string())),
    }
}

/// The answer as JSON, with its `kind` and, for rows and triples, the
/// count `omitted` past `maxRows`.
fn answer_json(a: &Answer) -> Value {
    match a {
        Answer::Boolean(b) => json!({ "kind": "ask", "head": {}, "boolean": b }),
        Answer::Solutions(s) => {
            let bindings: Vec<Value> = s
                .rows
                .iter()
                .map(|row| {
                    let mut m = serde_json::Map::new();
                    for (var, term) in s.variables.iter().zip(row) {
                        if let Some(t) = term {
                            m.insert(var.clone(), json_term(t));
                        }
                    }
                    Value::Object(m)
                })
                .collect();
            json!({
                "kind": "select",
                "head": { "vars": s.variables },
                "results": { "bindings": bindings },
                "omitted": s.omitted,
            })
        }
        Answer::Triples(t) => {
            let triples: Vec<Value> = t
                .iter()
                .map(|t| {
                    json!({
                        "subject": json_term(&t.subject.clone().into()),
                        "predicate": json_term(&t.predicate.clone().into()),
                        "object": json_term(&t.object),
                    })
                })
                .collect();
            json!({ "kind": "construct", "triples": triples, "omitted": t.omitted })
        }
    }
}
