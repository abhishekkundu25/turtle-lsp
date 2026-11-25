//! `turtle-lsp format`: format files in place, or say what would change;
//! standard input goes to stdout. With `--format json`, stdout is one line:
//! `{"version": 1, "files": [{"path", "status", "reason"?}], "summary":
//! {"changed", "notFormatted", "filesChecked"}}`, a file's `status` being
//! `formatted`, `would-format` or `not-formatted` (with its `reason`).
//! Unchanged files are counted, not listed. No formatter writes RDF/XML,
//! OWL Functional syntax or OWL/XML: such a file found in a folder is passed
//! over without a word, neither listed nor counted, and leaves the exit
//! status as it is; one named on the command line is not formatted, and
//! says so ("not formatted: RDF/XML", "not formatted: OWL Functional
//! syntax", "not formatted: OWL/XML").

use super::args::FormatArgs;
use super::diff;
use super::files::{self, Context};
use crate::batch::{self, Columns, Disk, FormatOutcome, Options};
use serde_json::{Value, json};
use std::io::Write;

/// Runs the formatter; the exit code.
pub(crate) fn run(a: FormatArgs) -> Result<i32, String> {
    let cwd = files::current_dir()?;
    let targets = files::Targets::read(&cwd, &a.paths, a.stdin_filename.as_deref())?;
    let root = targets.root(a.root.as_deref(), &cwd)?;
    let settings = files::settings(&root, &Value::Null);
    let ws = files::load_lenient(root, cwd, targets, Context::None)?;
    // Standard input is printed, formatted or as it was; never written.
    let stdin = a.stdin_filename.is_some();
    let batch: Vec<batch::BatchFile> = ws.files.iter().map(files::File::batch).collect();
    let disk = Disk {
        roots: vec![ws.root.clone()],
        network: false,
    };
    let opts = Options {
        shacl: false,
        columns: Columns::CodePoints,
        fixes: false,
        verdict: true,
        // The formatter's verdict is all `format` shows.
        diagnostics: false,
    };
    let report = batch::analyse_on_disk(&batch, &settings, &opts, &disk);

    let dry = a.check || a.diff;
    let (mut changed, mut skipped, mut unwritten) = (0, ws.unread.len(), 0);
    // The files `--format json` lists.
    let mut listed: Vec<Value> = Vec::new();
    for u in &ws.unread {
        eprintln!("{}: not formatted: {}", u.shown, u.message);
        listed.push(not_formatted(&u.shown, &u.message));
    }
    // Files left out of the run (warned about already) are skipped files.
    if ws.incomplete {
        skipped += 1;
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    // RDF/XML, OWL Functional and OWL/XML files found in a folder: passed over
    // without a word.
    let mut passed_over = 0;
    for (file, fr) in ws.files.iter().zip(report.files) {
        let Some(verdict) = fr.format else { continue };
        let written = match verdict.outcome {
            FormatOutcome::Refused(_)
                if matches!(fr.kind, "rdfxml" | "ofn" | "owx") && !file.named =>
            {
                passed_over += 1;
                continue;
            }
            FormatOutcome::Unchanged if stdin && !dry => out.write_all(file.text.as_bytes()),
            FormatOutcome::Unchanged => continue,
            FormatOutcome::Refused(reason) => {
                eprintln!("{}: not formatted: {reason}", file.shown);
                listed.push(not_formatted(&file.shown, &reason));
                skipped += 1;
                continue;
            }
            FormatOutcome::Changed(text) => {
                let shown = &file.shown;
                if !dry && !stdin {
                    match files::write_back(&file.path, file.stamp, &text) {
                        Ok(()) => {
                            changed += 1;
                            listed.push(json!({ "path": shown, "status": "formatted" }));
                        }
                        Err(e) => {
                            eprintln!("{shown}: not formatted: {e}");
                            listed.push(not_formatted(shown, &e));
                            unwritten += 1;
                        }
                    }
                    continue;
                }
                changed += 1;
                if a.json {
                    listed.push(json!({ "path": shown, "status": "would-format" }));
                    continue;
                }
                if a.diff {
                    // Like git: `a/abs/x.ttl`, not `a//abs/x.ttl`.
                    let label = shown.trim_start_matches('/');
                    let d = diff::unified(
                        &file.text,
                        &text,
                        &format!("a/{label}"),
                        &format!("b/{label}"),
                    );
                    out.write_all(d.as_bytes())
                } else if !stdin {
                    writeln!(out, "{shown}")
                } else if dry {
                    Ok(())
                } else {
                    out.write_all(text.as_bytes())
                }
            }
        };
        to_stdout(written)?;
    }
    if a.json {
        listed.sort_by(|x, y| x["path"].as_str().cmp(&y["path"].as_str()));
        let refused = listed
            .iter()
            .filter(|f| f["status"] == "not-formatted")
            .count();
        let report = json!({
            "version": 1,
            "files": listed,
            "summary": {
                "changed": changed,
                "notFormatted": refused,
                "filesChecked": ws.files.len() + ws.unread.len() - passed_over,
            },
        });
        to_stdout(writeln!(out, "{report}"))?;
    }
    // Standard input's text is the output: losing its end is a failure.
    to_stdout(out.flush())?;
    let s = |n: usize| if n == 1 { "" } else { "s" };
    if changed > 0 && !stdin {
        let verb = if dry {
            "would be formatted"
        } else {
            "formatted"
        };
        eprintln!("{changed} file{} {verb}", s(changed));
    }
    if unwritten > 0 {
        eprintln!("{unwritten} file{} could not be written", s(unwritten));
    }
    Ok(if skipped > 0 || unwritten > 0 {
        2
    } else if dry && changed > 0 {
        1
    } else {
        0
    })
}

/// A file `--format json` lists as not formatted, and why.
fn not_formatted(path: &str, reason: &str) -> Value {
    json!({ "path": path, "status": "not-formatted", "reason": reason })
}

/// A write to stdout; a reader that stopped early (`| head`) is not a failure.
fn to_stdout(written: std::io::Result<()>) -> Result<(), String> {
    match written {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => {
            Err(format!("cannot write to stdout: {e}"))
        }
        _ => Ok(()),
    }
}
