//! `turtle-lsp check`: report what the editor would show.

use super::args::CheckArgs;
use super::files::{self, Context};
use super::output::{self, CheckedFile, Format, Summary};
use crate::batch::{self, Columns, Disk, Options, Problem};
use lsp_types::{DiagnosticSeverity, NumberOrString};
use serde_json::json;
use std::io::Write;

/// The SHACL run was skipped: shown as a warning, so a check never passes
/// without validation unnoticed.
const SHACL_SKIPPED: &str = "shacl-skipped";

fn code(p: &Problem) -> &str {
    match &p.diagnostic.code {
        Some(NumberOrString::String(s)) => s,
        _ => "",
    }
}

/// Runs the check; the exit code.
pub fn run(a: CheckArgs) -> Result<i32, String> {
    let cwd = files::current_dir()?;
    let root = files::find_root(a.root.as_deref(), &cwd)?;
    let mut flags = serde_json::Map::new();
    if a.no_shacl {
        flags.insert("shacl.enabled".into(), json!(false));
    }
    if a.network {
        flags.insert("imports.network".into(), json!(true));
    }
    let settings = files::settings(&root, &flags.into());
    let ws = files::load(root, cwd, &a.paths, Context::Workspace)?;
    let disk = Disk {
        roots: vec![ws.root.clone()],
        network: settings.imports_network,
    };
    let mut batch: Vec<batch::BatchFile> = ws.files.iter().map(files::File::batch).collect();

    if a.fix {
        let outcome = batch::fix_all_on_disk(&batch, &settings, a.unsafe_fixes, &disk);
        let mut changed = 0;
        for fixed in outcome.files {
            let Some(i) = batch.iter().position(|f| f.name == fixed.name) else {
                continue;
            };
            if batch[i].text == fixed.text {
                continue;
            }
            std::fs::write(&ws.files[i].path, &fixed.text)
                .map_err(|e| format!("{}: {e}", fixed.name))?;
            batch[i].text = fixed.text;
            changed += 1;
        }
        if outcome.applied > 0 {
            let s = |n: usize| if n == 1 { "" } else { "s" };
            eprintln!(
                "Applied {} fix{} in {changed} file{}",
                outcome.applied,
                if outcome.applied == 1 { "" } else { "es" },
                s(changed)
            );
        }
    }

    let opts = Options {
        shacl: !a.no_shacl,
        columns: Columns::CodePoints,
    };
    let report = batch::analyse_on_disk(&batch, &settings, &opts, &disk);

    let keep = |c: &str| {
        (a.select.is_empty() || a.select.iter().any(|s| s == c)) && !a.ignore.iter().any(|s| s == c)
    };
    // (index into ws.files, its problems), for every reported file and every
    // context file with a skipped SHACL run.
    let mut checked: Vec<(usize, Vec<Problem>)> = Vec::new();
    for (i, fr) in report.files.into_iter().enumerate() {
        let reported = ws.files[i].reported;
        let mut problems: Vec<Problem> = fr
            .problems
            .into_iter()
            .filter(|p| reported || code(p) == SHACL_SKIPPED)
            .filter(|p| keep(code(p)))
            .map(|mut p| {
                if code(&p) == SHACL_SKIPPED {
                    p.diagnostic.severity = Some(DiagnosticSeverity::WARNING);
                }
                p
            })
            .collect();
        if !reported && problems.is_empty() {
            continue;
        }
        problems.sort_by(|x, y| {
            let (r, s) = (x.diagnostic.range.start, y.diagnostic.range.start);
            (r.line, r.character, code(x)).cmp(&(s.line, s.character, code(y)))
        });
        checked.push((i, problems));
    }
    checked.sort_by(|x, y| ws.files[x.0].shown.cmp(&ws.files[y.0].shown));

    let mut summary = Summary {
        files_checked: ws.files.iter().filter(|f| f.reported).count(),
        ..Summary::default()
    };
    for (_, problems) in &checked {
        if !problems.is_empty() {
            summary.files_with_problems += 1;
        }
        for p in problems {
            match p.diagnostic.severity {
                Some(DiagnosticSeverity::WARNING) => summary.warnings += 1,
                Some(DiagnosticSeverity::INFORMATION) => summary.infos += 1,
                Some(DiagnosticSeverity::HINT) => summary.hints += 1,
                _ => summary.errors += 1,
            }
        }
    }

    let limit = a.max_diagnostics.unwrap_or(usize::MAX);
    let view = |all: bool| -> Vec<CheckedFile> {
        checked
            .iter()
            .map(|(i, problems)| CheckedFile {
                path: &ws.files[*i].shown,
                root_relative: &ws.files[*i].root_relative,
                problems: if all {
                    problems
                } else {
                    &problems[..problems.len().min(limit)]
                },
            })
            .collect()
    };

    let format = a.format.unwrap_or(Format::Concise);
    let write = |out: &mut dyn Write| -> std::io::Result<()> {
        if a.statistics {
            output::statistics(&view(true), out)?;
        } else {
            output::render(format, &view(false), &summary, out)?;
        }
        out.flush()
    };
    let written = match &a.output_file {
        Some(path) => {
            let path = ws.cwd.join(path);
            let file = std::fs::File::create(&path)
                .map_err(|e| format!("--output-file {}: {e}", path.display()))?;
            write(&mut std::io::BufWriter::new(file))
        }
        None => write(&mut std::io::BufWriter::new(std::io::stdout().lock())),
    };
    match written {
        Ok(()) => {}
        // A reader that stopped early (`| head`) is not a failure.
        Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
        Err(e) => return Err(format!("cannot write the report: {e}")),
    }
    let problems = summary.errors + summary.warnings + summary.infos + summary.hints;
    if (format == Format::Concise || a.statistics || a.output_file.is_some()) && problems > 0 {
        eprintln!("{}", output::summary_line(&summary));
    }

    let failed = summary.errors > 0 || a.max_warnings.is_some_and(|m| summary.warnings > m);
    Ok(if failed && !a.exit_zero { 1 } else { 0 })
}
