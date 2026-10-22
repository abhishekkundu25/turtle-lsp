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
//! The query runs on a thread of its own (never the main loop), with
//! `query.timeoutSeconds` and `query.maxRows`. The thread renders the
//! results as Markdown and writes them to
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

use super::Server;
use super::client::Client;
use crate::analysis::Analysis;
use crate::clock::Instant;
use crate::knowledge::Source;
use crate::query::{self, Answer, Limits, QueryError};
use crate::syntax::PrefixMap;
use crate::workspace::{is_sparql_path, path_to_uri, uri_to_path};
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_server::{ErrorCode, RequestId, Response};
use lsp_types::request::Request as _;
use lsp_types::{MessageType, ShowDocumentParams, Uri};
use rustc_hash::FxHashMap;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// The runs going on, by document URI, and the channel their results come
/// back on.
pub struct State {
    /// The client declared `window.showDocument.support`.
    show_document: bool,
    done_tx: Sender<Done>,
    done_rx: Receiver<Done>,
    next_id: u64,
    running: FxHashMap<String, Running>,
    /// The results file stem of each document run so far.
    stems: FxHashMap<String, String>,
}

struct Running {
    id: u64,
    cancel: Arc<AtomicBool>,
    request: RequestId,
}

/// A finished run, for the main loop.
pub struct Done {
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
    /// The results file written, or why it was not.
    file: Result<PathBuf, String>,
}

/// One run, as handed to its thread.
struct Job {
    name: String,
    text: String,
    base: String,
    prefixes: Arc<PrefixMap>,
    docs: Vec<Arc<Analysis>>,
    timeout: Option<Duration>,
    max_rows: Option<usize>,
    max_buffered_rows: Option<usize>,
    cancel: Arc<AtomicBool>,
    out: Option<PathBuf>,
}

impl State {
    /// Runs for a client that can show a document (`show_document`) or not.
    pub fn new(show_document: bool) -> State {
        let (done_tx, done_rx) = unbounded();
        State {
            show_document,
            done_tx,
            done_rx,
            next_id: 0,
            running: FxHashMap::default(),
            stems: FxHashMap::default(),
        }
    }

    /// Finished runs, for the main loop.
    pub fn results(&self) -> Receiver<Done> {
        self.done_rx.clone()
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
    pub fn cancel_all(&self) {
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

impl Server {
    /// `turtle.runQuery` for `uri`, asked by request `id`: Some(answer) when
    /// answered now, None when the answer comes with the result.
    pub(super) fn run_query(
        &mut self,
        id: RequestId,
        uri: &str,
    ) -> Result<Option<Value>, (ErrorCode, String)> {
        let uri = crate::workspace::normalize_uri(uri);
        let (text, base, prefixes) = match self.ws.sparql(&uri) {
            Some(q) => (q.text().to_owned(), q.base().to_owned(), q.external.clone()),
            None => {
                let path = uri_to_path(&uri)
                    .filter(|p| is_sparql_path(p) && !self.ws.is_open(&uri))
                    .ok_or_else(|| {
                        (
                            ErrorCode::InvalidParams,
                            format!("{uri} is not a SPARQL document"),
                        )
                    })?;
                let text = std::fs::read_to_string(&path)
                    .map_err(|e| (ErrorCode::InvalidParams, format!("{}: {e}", path.display())))?;
                (text, uri.clone(), self.ws.external_prefixes_for(&uri))
            }
        };
        let docs: Vec<Arc<Analysis>> = self
            .ws
            .knowledge()
            .sources()
            .filter(|(_, s, _)| *s != Source::External)
            .map(|(_, _, a)| a.clone())
            .collect();
        let cancel = Arc::new(AtomicBool::new(false));
        let job = Job {
            name: file_name(&uri),
            text,
            base,
            prefixes,
            docs,
            timeout: self.layers.settings().query_timeout,
            max_rows: self.layers.settings().query_max_rows,
            max_buffered_rows: self.layers.settings().query_max_buffered_rows,
            cancel: cancel.clone(),
            out: if self.inline { None } else { results_dir() },
        };
        if self.inline {
            let report = execute(&job, &self.queries.stem_for(&uri));
            return Ok(Some(self.queries.deliver(
                &mut self.client,
                &job.name,
                report,
            )));
        }
        // A newer run of the same document replaces the one going on.
        if let Some(old) = self.queries.running.remove(&uri) {
            old.cancel.store(true, Ordering::Relaxed);
            let resp = Response::new_ok(old.request, "cancelled: a newer run replaced it");
            let _ = self.client.send(resp);
        }
        self.queries.next_id += 1;
        let run_id = self.queries.next_id;
        self.queries.running.insert(
            uri.clone(),
            Running {
                id: run_id,
                cancel,
                request: id,
            },
        );
        let out = self.queries.done_tx.clone();
        let stem = self.queries.stem_for(&uri);
        std::thread::Builder::new()
            .name("run-query".into())
            .stack_size(16 << 20)
            .spawn(move || {
                let report =
                    super::guard("run query", || execute(&job, &stem)).unwrap_or_else(|msg| {
                        Report {
                            summary: msg,
                            failed: true,
                            file: Err("not written".into()),
                        }
                    });
                let _ = out.send(Done {
                    id: run_id,
                    uri,
                    report,
                });
            })
            .map_err(|e| (ErrorCode::InternalError, e.to_string()))?;
        Ok(None)
    }

    /// A run finished: show it and answer its request (unless it was
    /// replaced or cancelled meanwhile).
    pub(super) fn query_done(&mut self, done: Done) {
        if self.queries.running.get(&done.uri).map(|r| r.id) != Some(done.id) {
            return;
        }
        let Some(run) = self.queries.running.remove(&done.uri) else {
            return;
        };
        let name = file_name(&done.uri);
        let summary = self.queries.deliver(&mut self.client, &name, done.report);
        let _ = self.client.send(Response::new_ok(run.request, summary));
    }

    /// A `$/cancelRequest` for `id`: when it is a pending run, stop it and
    /// answer `RequestCanceled`.
    pub(super) fn query_cancel_request(&mut self, id: &RequestId) {
        let Some(uri) = self
            .queries
            .running
            .iter()
            .find(|(_, r)| r.request == *id)
            .map(|(u, _)| u.clone())
        else {
            return;
        };
        if let Some(run) = self.queries.running.remove(&uri) {
            run.cancel.store(true, Ordering::Relaxed);
            let resp = Response::new_err(
                run.request,
                ErrorCode::RequestCanceled as i32,
                "request cancelled".into(),
            );
            let _ = self.client.send(resp);
        }
    }
}

impl State {
    /// Open the results file, or show the summary; the summary.
    fn deliver(&self, client: &mut Client, name: &str, report: Report) -> Value {
        let file = report
            .file
            .as_ref()
            .ok()
            .and_then(|p| path_to_uri(p))
            .and_then(|u| u.parse::<Uri>().ok());
        match file {
            Some(uri) if self.show_document => {
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

/// Run `job`, render the results and write them to `<job.out>/<stem>.md`.
fn execute(job: &Job, stem: &str) -> Report {
    let start = Instant::now();
    let docs: Vec<&Analysis> = job.docs.iter().map(|a| &**a).collect();
    let limits = Limits {
        timeout: job.timeout,
        cancel: Some(job.cancel.clone()),
        max_rows: job.max_rows,
        max_buffered_rows: job.max_buffered_rows,
    };
    let outcome = query::run(&docs, &job.text, Some(&job.base), &job.prefixes, &limits);
    let elapsed = Instant::now().saturating_duration_since(start);
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
            file: Err("cancelled".into()),
        };
    }
    let file = match &job.out {
        Some(dir) => {
            let md = document(job, &outcome, elapsed);
            let path = dir.join(format!("{stem}.md"));
            std::fs::create_dir_all(dir)
                .and_then(|()| std::fs::write(&path, md))
                .map(|()| path)
                .map_err(|e| e.to_string())
        }
        None => Err("no cache directory".into()),
    };
    Report {
        summary,
        failed,
        file,
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
/// over how many files, then the results (or the error).
fn document(job: &Job, outcome: &Result<Answer, QueryError>, elapsed: Duration) -> String {
    let files = job.docs.len();
    let mut out = format!(
        "# {}\n\nRan in {} over {files} file{}.\n\n",
        job.name,
        duration(elapsed),
        if files == 1 { "" } else { "s" }
    );
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
        let mut state = State::new(false);
        assert_eq!(state.stem_for("file:///a/Same.rq"), "Same");
        assert_eq!(state.stem_for("file:///b/same.rq"), "same-2");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_millis(34)), "34 ms");
        assert_eq!(duration(Duration::from_millis(1234)), "1.2 s");
        assert_eq!(duration(Duration::from_secs(30)), "30 s");
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
            cancel: Arc::new(AtomicBool::new(false)),
            out: Some(dir.clone()),
        };
        let r = execute(&job("SELECT ?s WHERE { ?s ex:p ?o } ORDER BY ?o"), "q");
        assert!(r.summary.starts_with("2 rows in "), "{}", r.summary);
        let md = std::fs::read_to_string(r.file.unwrap()).unwrap();
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
        let md = std::fs::read_to_string(r.file.unwrap()).unwrap();
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
        let md = std::fs::read_to_string(r.file.unwrap()).unwrap();
        assert!(md.contains("## Error\n\nSyntax error at 1:"), "{md}");

        let cancelled = job("ASK {}");
        cancelled.cancel.store(true, Ordering::Relaxed);
        let r = execute(&cancelled, "q");
        assert_eq!(r.summary, "cancelled");
        assert!(r.file.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
