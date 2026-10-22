//! `turtle-lsp format`: format files in place, or say what would change.

use super::args::FormatArgs;
use super::diff;
use super::files::{self, Context};
use crate::batch::{self, Columns, Disk, FormatOutcome, Options};
use serde_json::Value;
use std::io::Write;

/// Runs the formatter; the exit code.
pub fn run(a: FormatArgs) -> Result<i32, String> {
    let cwd = files::current_dir()?;
    let root = files::find_root(a.root.as_deref(), &cwd)?;
    let settings = files::settings(&root, &Value::Null);
    let ws = files::load_lenient(root, cwd, &a.paths, Context::None)?;
    let batch: Vec<batch::BatchFile> = ws.files.iter().map(files::File::batch).collect();
    let disk = Disk {
        roots: vec![ws.root.clone()],
        network: false,
    };
    let opts = Options {
        shacl: false,
        columns: Columns::CodePoints,
        fixes: false,
    };
    let report = batch::analyse_on_disk(&batch, &settings, &opts, &disk);

    let dry = a.check || a.diff;
    let (mut changed, mut skipped, mut unwritten) = (0, ws.unread.len(), 0);
    for u in &ws.unread {
        eprintln!("{}: not formatted: {}", u.shown, u.message);
    }
    // Files left out of the run (warned about already) are skipped files.
    if ws.incomplete {
        skipped += 1;
    }
    let mut out = std::io::BufWriter::new(std::io::stdout().lock());
    for (file, fr) in ws.files.iter().zip(report.files) {
        let Some(verdict) = fr.format else { continue };
        match verdict.outcome {
            FormatOutcome::Unchanged => {}
            FormatOutcome::Refused(reason) => {
                eprintln!("{}: not formatted: {reason}", file.shown);
                skipped += 1;
            }
            FormatOutcome::Changed(text) => {
                let shown = &file.shown;
                if !dry {
                    match files::write_back(&file.path, file.stamp, &text) {
                        Ok(()) => changed += 1,
                        Err(e) => {
                            eprintln!("{shown}: not formatted: {e}");
                            unwritten += 1;
                        }
                    }
                    continue;
                }
                changed += 1;
                let written = if a.diff {
                    // Like git: `a/abs/x.ttl`, not `a//abs/x.ttl`.
                    let label = shown.trim_start_matches('/');
                    let d = diff::unified(
                        &file.text,
                        &text,
                        &format!("a/{label}"),
                        &format!("b/{label}"),
                    );
                    out.write_all(d.as_bytes())
                } else {
                    writeln!(out, "{shown}")
                };
                match written {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
                    Err(e) => return Err(format!("cannot write to stdout: {e}")),
                }
            }
        }
    }
    let _ = out.flush();
    let s = |n: usize| if n == 1 { "" } else { "s" };
    if changed > 0 {
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
