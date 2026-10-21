//! `turtle-lsp query`: run a SPARQL query over the workspace's files.
//!
//! What each answer prints, by `--format`:
//!
//! | answer               | table (default) | json            | csv, tsv | turtle    |
//! |----------------------|-----------------|-----------------|----------|-----------|
//! | SELECT               | aligned table   | SPARQL JSON     | yes      | error     |
//! | ASK                  | `true`/`false`  | SPARQL JSON     | error    | error     |
//! | CONSTRUCT, DESCRIBE  | error           | error           | error    | (default) |
//!
//! A graph prints as Turtle when no format is given. Exit codes: 0 the query
//! ran (an ASK answered true), 1 an ASK answered false, 2 an error.

use super::args::{QueryArgs, QueryFormat, QuerySource};
use super::files::{self, Context};
use crate::batch::{self, BatchFile, Disk};
use crate::query::{Answer, Limits, QueryError, render};
use serde_json::Value;
use std::io::Write;

/// The name an inline (`-e`) query goes by, in messages and as its file name
/// (in the current folder: its base IRI and where its prefixes.ttl is found).
const INLINE: &str = "-e";

/// Runs the query; the exit code.
pub fn run(a: QueryArgs) -> Result<i32, String> {
    let cwd = files::current_dir()?;
    let root = files::find_root(a.root.as_deref(), &cwd)?;
    let settings = files::settings(&root, &Value::Null);
    let q = match &a.query {
        QuerySource::File(p) => {
            let path = files::real(&cwd.join(p));
            let shown = p.to_string_lossy().replace('\\', "/");
            let text = std::fs::read_to_string(&path).map_err(|e| format!("{shown}: {e}"))?;
            BatchFile::at_path(&path, &shown, text, false)
        }
        QuerySource::Inline(text) => {
            BatchFile::at_path(&cwd.join(INLINE), INLINE, text.clone(), false)
        }
    }
    .ok_or("the query has no file URI")?;
    let ws = files::load(root, cwd, &a.paths, Context::None)?;
    let data: Vec<BatchFile> = ws.files.iter().map(files::File::batch).collect();
    let disk = Disk {
        roots: vec![ws.root.clone()],
        network: settings.imports_network,
    };
    let limits = Limits {
        timeout: a.timeout,
        cancel: None,
    };
    let outcome = batch::query_on_disk(&data, &q, &settings, &limits, &disk);
    let answer = match outcome.answer {
        Ok(answer) => answer,
        Err(QueryError::Syntax {
            message,
            line,
            column,
            ..
        }) => {
            eprintln!("{}:{line}:{column}: {message}", q.name);
            return Ok(2);
        }
        Err(QueryError::Timeout) => {
            let secs = a.timeout.map_or(0.0, |t| t.as_secs_f64());
            return Err(format!(
                "the query was stopped at the time limit ({secs} s; see --timeout)"
            ));
        }
        Err(e) => return Err(e.to_string()),
    };
    let (text, code) = render(&answer, a.format, a.limit, &outcome.prefixes)?;
    let mut out = std::io::stdout().lock();
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => {}
        // A reader that stopped early (`| head`) is not a failure.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => return Err(format!("cannot write to stdout: {e}")),
    }
    Ok(code)
}

/// The text to print and the exit code, or why `format` does not fit the
/// answer.
fn render(
    answer: &Answer,
    format: Option<QueryFormat>,
    limit: Option<usize>,
    prefixes: &crate::syntax::PrefixMap,
) -> Result<(String, i32), String> {
    let misfit = |what: &str, fits: &str| {
        let f = format.map_or("table", QueryFormat::name);
        Err(format!("--format {f} does not fit {what}; use {fits}"))
    };
    match answer {
        Answer::Solutions(s) => match format.unwrap_or(QueryFormat::Table) {
            QueryFormat::Table => Ok((render::table(s, prefixes, limit), 0)),
            QueryFormat::Json => Ok((render::json(answer).unwrap_or_default(), 0)),
            QueryFormat::Csv => Ok((render::csv(s), 0)),
            QueryFormat::Tsv => Ok((render::tsv(s), 0)),
            QueryFormat::Turtle => misfit("a SELECT query", "table, json, csv or tsv"),
        },
        Answer::Boolean(b) => {
            let code = if *b { 0 } else { 1 };
            match format.unwrap_or(QueryFormat::Table) {
                QueryFormat::Table => Ok((format!("{b}\n"), code)),
                QueryFormat::Json => Ok((render::json(answer).unwrap_or_default(), code)),
                _ => misfit("an ASK query", "table or json"),
            }
        }
        Answer::Triples(t) => match format.unwrap_or(QueryFormat::Turtle) {
            QueryFormat::Turtle => Ok((render::turtle(t, prefixes), 0)),
            _ => misfit("a CONSTRUCT or DESCRIBE query", "turtle"),
        },
    }
}
