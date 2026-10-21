//! The operations the toolbox asks for. `analyse` and `fix_all` run
//! [`turtle_lsp::batch`] on the input, every file named below
//! `file:///work/`; each call builds the workspace anew, as the input is a
//! handful of files and analysis is fast. `query` runs a SPARQL file over the
//! others with [`batch::query`]. The `lsp` operations drive the editor's
//! language server.

use lsp_server::Message;
use lsp_types::NumberOrString;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::time::Duration;
use turtle_lsp::batch::{self, BatchFile, Columns, FormatOutcome};
use turtle_lsp::clock::Instant;
use turtle_lsp::query::{Answer, Limits, QueryError, render};
use turtle_lsp::server::Embedded;
use turtle_lsp::settings::Settings;

pub fn handle(request: &str) -> String {
    let req: Value = match serde_json::from_str(request) {
        Ok(v) => v,
        Err(e) => return error(&format!("the request is not JSON: {e}")),
    };
    let files = || -> Result<Vec<BatchFile>, String> {
        let list = req["files"].as_array().ok_or("the request has no files")?;
        list.iter()
            .map(|f| match (f["name"].as_str(), f["text"].as_str()) {
                (Some(name), Some(text)) => Ok(BatchFile::named(
                    name,
                    text.to_string(),
                    f["context"].as_bool().unwrap_or(false),
                )),
                _ => Err("each file needs a name and a text".to_string()),
            })
            .collect()
    };
    let answer = match req["op"].as_str().unwrap_or("") {
        "version" => Ok(json!({ "version": turtle_lsp::VERSION })),
        "analyse" => files().map(|f| analyse(&f)),
        "fix_all" => files().map(fix_all),
        "query" => files().and_then(|f| query(&f, &req)),
        "lsp_start" => lsp_start(req["params"].clone()),
        "lsp" => lsp(&req["messages"]),
        "lsp_tick" => lsp_tick(),
        op => Err(format!("unknown operation '{op}'")),
    };
    match answer {
        Ok(v) => json!({ "ok": v }).to_string(),
        Err(e) => error(&e),
    }
}

thread_local! {
    /// The editor's language server (see turtle_lsp::server::Embedded).
    static SERVER: RefCell<Option<Embedded>> = const { RefCell::new(None) };
}

/// A new language server for `initialize` params; its initialize response.
fn lsp_start(params: Value) -> Result<Value, String> {
    let (server, sent) = Embedded::start(params)?;
    let due = server.due_in_ms();
    SERVER.with(|s| *s.borrow_mut() = Some(server));
    Ok(json!({ "messages": sent, "due": due }))
}

/// Hand LSP messages to the language server; what it sent, and when its
/// next timer is due (milliseconds, or null).
fn lsp(messages: &Value) -> Result<Value, String> {
    let messages: Vec<Message> =
        serde_json::from_value(messages.clone()).map_err(|e| format!("bad LSP messages: {e}"))?;
    with_server(|s| s.handle(messages))
}

fn lsp_tick() -> Result<Value, String> {
    with_server(Embedded::tick)
}

fn with_server(f: impl FnOnce(&mut Embedded) -> Vec<Message>) -> Result<Value, String> {
    SERVER.with(|s| {
        let mut s = s.borrow_mut();
        let server = s
            .as_mut()
            .ok_or("the language server is not started (lsp_start)")?;
        let sent = f(server);
        Ok(json!({ "messages": sent, "due": server.due_in_ms() }))
    })
}

pub fn error(message: &str) -> String {
    json!({ "error": message }).to_string()
}

/// Positions in UTF-16 units, as JavaScript strings count them.
fn options() -> batch::Options {
    batch::Options {
        shacl: true,
        columns: Columns::Utf16,
    }
}

fn analyse(files: &[BatchFile]) -> Value {
    let report = batch::analyse(files, &Settings::default(), &options());
    let out: Vec<Value> = report
        .files
        .iter()
        .map(|f| {
            let diagnostics: Vec<Value> = f
                .problems
                .iter()
                .map(|p| {
                    let d = &p.diagnostic;
                    let code = match &d.code {
                        Some(NumberOrString::String(s)) => json!(s),
                        Some(NumberOrString::Number(n)) => json!(n.to_string()),
                        None => Value::Null,
                    };
                    let fixes: Vec<Value> = p.fixes.iter().map(fix_json).collect();
                    json!({
                        "range": d.range,
                        "severity": d.severity,
                        "code": code,
                        "message": d.message,
                        "fixes": fixes,
                    })
                })
                .collect();
            json!({
                "name": f.name,
                "kind": f.kind,
                "diagnostics": diagnostics,
                "format": f.format.as_ref().map_or(Value::Null, format_json),
            })
        })
        .collect();
    json!({ "files": out, "hasShapes": report.has_shapes })
}

fn fix_json(fix: &batch::Fix) -> Value {
    let edits: serde_json::Map<String, Value> = fix
        .edits
        .iter()
        .map(|(name, list)| (name.clone(), json!(list)))
        .collect();
    json!({ "title": fix.title, "preferred": fix.preferred, "edits": edits })
}

fn format_json(f: &batch::Formatted) -> Value {
    let guarantee = f.guarantee;
    match &f.outcome {
        FormatOutcome::Unchanged => json!({ "status": "unchanged", "guarantee": guarantee }),
        FormatOutcome::Changed(text) => {
            json!({ "status": "changed", "guarantee": guarantee, "text": text })
        }
        FormatOutcome::Refused(reason) => {
            json!({ "status": "refused", "guarantee": guarantee, "reason": reason })
        }
    }
}

/// Apply every preferred fix (see [`batch::fix_all`]), term renames included.
fn fix_all(files: Vec<BatchFile>) -> Value {
    let outcome = batch::fix_all(&files, &Settings::default(), true);
    let files: Vec<Value> = outcome
        .files
        .iter()
        .map(|f| json!({ "name": f.name, "text": f.text }))
        .collect();
    json!({ "files": files, "applied": outcome.applied })
}

/// Run the file named `req.query` (one of `files`) over the others, at most
/// `req.timeout` seconds (default 30) and showing at most `req.maxRows` rows
/// or triples (default 1000). The answer is one of
/// `{kind: "select", variables, rows, total}` (each row a list of display
/// strings, null when unbound), `{kind: "ask", boolean}`,
/// `{kind: "construct", turtle, shown, total}` (the first `shown` triples) or
/// `{kind: "error", message, line, column}` (line and column 1-based, for a
/// syntax error only, else null); each with `elapsedMs`.
fn query(files: &[BatchFile], req: &Value) -> Result<Value, String> {
    let name = req["query"].as_str().ok_or("the request has no query")?;
    let q = files
        .iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("no file is named '{name}'"))?;
    let seconds = req["timeout"].as_f64().unwrap_or(30.0);
    let max_rows = req["maxRows"].as_u64().map_or(1000, |n| n as usize);
    let limits = Limits {
        timeout: (seconds > 0.0).then(|| Duration::from_secs_f64(seconds)),
        ..Default::default()
    };
    let start = Instant::now();
    let outcome = batch::query(files, q, &Settings::default(), &limits);
    let p = &outcome.prefixes;
    let mut out = match &outcome.answer {
        Ok(Answer::Solutions(s)) => {
            let rows: Vec<Vec<Option<String>>> = s
                .rows
                .iter()
                .take(max_rows)
                .map(|r| {
                    r.iter()
                        .map(|t| t.as_ref().map(|t| render::term(t, p)))
                        .collect()
                })
                .collect();
            json!({ "kind": "select", "variables": s.variables, "rows": rows, "total": s.rows.len() })
        }
        Ok(Answer::Boolean(b)) => json!({ "kind": "ask", "boolean": b }),
        Ok(Answer::Triples(t)) => {
            let shown = t.len().min(max_rows);
            json!({
                "kind": "construct",
                "turtle": render::turtle(&t[..shown], p),
                "shown": shown,
                "total": t.len(),
            })
        }
        Err(QueryError::Syntax {
            message,
            line,
            column,
            ..
        }) => json!({ "kind": "error", "message": message, "line": line, "column": column }),
        Err(e) => {
            json!({ "kind": "error", "message": e.to_string(), "line": null, "column": null })
        }
    };
    let ms = Instant::now()
        .saturating_duration_since(start)
        .as_secs_f64()
        * 1000.0;
    out["elapsedMs"] = json!(ms);
    Ok(out)
}
