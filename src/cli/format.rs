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
    let ws = files::load(root, cwd, &a.paths, Context::None)?;
    let batch: Vec<batch::BatchFile> = ws.files.iter().map(files::File::batch).collect();
    let disk = Disk {
        roots: vec![ws.root.clone()],
        network: false,
    };
    let opts = Options {
        shacl: false,
        columns: Columns::CodePoints,
    };
    let report = batch::analyse_on_disk(&batch, &settings, &opts, &disk);

    let dry = a.check || a.diff;
    let (mut changed, mut skipped) = (0, 0);
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
                changed += 1;
                let shown = &file.shown;
                let written = if a.diff {
                    let d = diff::unified(
                        &file.text,
                        &text,
                        &format!("a/{shown}"),
                        &format!("b/{shown}"),
                    );
                    out.write_all(d.as_bytes())
                } else if a.check {
                    writeln!(out, "{shown}")
                } else {
                    std::fs::write(&file.path, &text).map_err(|e| format!("{shown}: {e}"))?;
                    Ok(())
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
    Ok(if skipped > 0 {
        2
    } else if dry && changed > 0 {
        1
    } else {
        0
    })
}
