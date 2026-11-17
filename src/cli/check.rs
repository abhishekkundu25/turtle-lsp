//! `turtle-lsp check`: report what the editor would show.

use super::args::CheckArgs;
use super::files::{self, Context};
use super::output::{self, CheckedFile, Format, Summary};
use crate::batch::{self, Columns, Disk, Options, Problem};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Range};
use serde_json::json;
use std::collections::HashMap;
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

/// A file in the report: its problems, as shown and sorted.
struct Checked<'a> {
    shown: &'a str,
    root_relative: Option<&'a str>,
    uri: &'a str,
    problems: Vec<Problem>,
}

/// Runs the check; the exit code.
pub(crate) fn run(a: CheckArgs) -> Result<i32, String> {
    let cwd = files::current_dir()?;
    let targets = files::Targets::read(&cwd, &a.paths, a.stdin_filename.as_deref())?;
    let root = targets.root(a.root.as_deref(), &cwd)?;
    let mut flags = serde_json::Map::new();
    if a.no_shacl {
        flags.insert("shacl.enabled".into(), json!(false));
    }
    if a.network {
        flags.insert("imports.network".into(), json!(true));
    }
    let settings = files::settings_strict(&root, &flags.into())?;
    let ws = files::load_lenient(root, cwd, targets, Context::Workspace)?;
    let disk = Disk {
        roots: vec![ws.root.clone()],
        network: settings.imports_network,
    };
    let mut batch: Vec<batch::BatchFile> = ws.files.iter().map(files::File::batch).collect();
    // Something kept the run from covering what it was asked to: exit 2.
    let mut failed_run = ws.incomplete || !ws.unread.is_empty();

    if a.fix {
        let outcome = batch::fix_all_on_disk(&batch, &settings, a.unsafe_fixes, &disk);
        let index: HashMap<&str, usize> = batch
            .iter()
            .enumerate()
            .map(|(i, f)| (f.name.as_str(), i))
            .collect();
        let (mut changed, mut unwritten) = (0, 0);
        let mut new_texts = Vec::new();
        for fixed in outcome.files {
            let Some(&i) = index.get(fixed.name.as_str()) else {
                continue;
            };
            if batch[i].text == fixed.text {
                continue;
            }
            let file = &ws.files[i];
            match files::write_back(&file.path, file.stamp, &fixed.text) {
                Ok(()) => {
                    changed += 1;
                    new_texts.push((i, fixed.text));
                }
                Err(e) => {
                    eprintln!("{}: not fixed: {e}", file.shown);
                    unwritten += 1;
                }
            }
        }
        for (i, text) in new_texts {
            batch[i].text = text;
        }
        let s = |n: usize| if n == 1 { "" } else { "s" };
        if unwritten > 0 {
            failed_run = true;
            eprintln!(
                "Fixed {changed} file{}; {unwritten} file{} could not be written",
                s(changed),
                s(unwritten)
            );
        } else if outcome.applied > 0 {
            eprintln!(
                "Applied {} fix{} in {changed} file{}",
                outcome.applied,
                if outcome.applied == 1 { "" } else { "es" },
                s(changed)
            );
        }
    }

    let format = a.format.unwrap_or(Format::Concise);
    // Only the machine-readable reports carry fixes; the others, and the
    // statistics, never show them. `--fix` finds its own.
    let shows_fixes = !a.statistics && matches!(format, Format::Json | Format::Sarif);
    let opts = Options {
        shacl: !a.no_shacl,
        columns: Columns::CodePoints,
        fixes: shows_fixes,
        // `check` never shows the formatter's verdict.
        verdict: false,
        diagnostics: true,
    };
    let report = batch::analyse_on_disk(&batch, &settings, &opts, &disk);

    let keep = |c: &str| {
        (a.select.is_empty() || a.select.iter().any(|s| s == c)) && !a.ignore.iter().any(|s| s == c)
    };
    // Every reported file and every context file with a skipped SHACL run,
    // and the reported files that could not be read.
    let mut checked: Vec<Checked> = Vec::new();
    for (file, fr) in ws.files.iter().zip(report.files) {
        let mut problems: Vec<Problem> = fr
            .problems
            .into_iter()
            .filter(|p| file.reported || code(p) == SHACL_SKIPPED)
            .filter(|p| keep(code(p)))
            .map(|mut p| {
                if code(&p) == SHACL_SKIPPED {
                    p.diagnostic.severity = Some(DiagnosticSeverity::WARNING);
                }
                p
            })
            // A skipped SHACL run says the check is incomplete: never hidden.
            .filter(|p| severity(p) <= a.min_severity || code(p) == SHACL_SKIPPED)
            .collect();
        if !file.reported && problems.is_empty() {
            continue;
        }
        problems.sort_by(|x, y| {
            let (r, s) = (x.diagnostic.range.start, y.diagnostic.range.start);
            (r.line, r.character, code(x)).cmp(&(s.line, s.character, code(y)))
        });
        checked.push(Checked {
            shown: &file.shown,
            root_relative: file.root_relative.as_deref(),
            uri: &file.uri,
            problems,
        });
    }
    for u in &ws.unread {
        let problems = if keep(u.code) {
            vec![Problem {
                diagnostic: Diagnostic {
                    range: Range::default(),
                    severity: Some(DiagnosticSeverity::ERROR),
                    code: Some(NumberOrString::String(u.code.to_string())),
                    source: Some("turtle-lsp".to_string()),
                    message: format!("The file could not be checked: {}", u.message),
                    ..Default::default()
                },
                fixes: Vec::new(),
            }]
        } else {
            Vec::new()
        };
        checked.push(Checked {
            shown: &u.shown,
            root_relative: u.root_relative.as_deref(),
            uri: &u.uri,
            problems,
        });
    }
    checked.sort_by(|x, y| x.shown.cmp(y.shown));

    let mut summary = Summary {
        files_checked: ws.files.iter().filter(|f| f.reported).count() + ws.unread.len(),
        ..Summary::default()
    };
    for c in &checked {
        if !c.problems.is_empty() {
            summary.files_with_problems += 1;
        }
        for p in &c.problems {
            match p.diagnostic.severity {
                Some(DiagnosticSeverity::WARNING) => summary.warnings += 1,
                Some(DiagnosticSeverity::INFORMATION) => summary.infos += 1,
                Some(DiagnosticSeverity::HINT) => summary.hints += 1,
                _ => summary.errors += 1,
            }
        }
    }

    // The statistics count every problem; neither cap applies to them. The
    // default cap is for a reader's screen or context, not for a file.
    let (per_file, max_problems) = if a.statistics {
        (None, None)
    } else {
        let total = match a.max_problems {
            Some(0) => None,
            Some(n) => Some(n),
            None if format == Format::Concise && a.output_file.is_none() => {
                Some(super::args::DEFAULT_MAX_PROBLEMS)
            }
            None => None,
        };
        (a.max_diagnostics, total)
    };
    let (kept, not_shown) = budget(&checked, per_file, max_problems);
    let shown: Vec<CheckedFile> = checked
        .iter()
        .enumerate()
        .map(|(i, c)| CheckedFile {
            path: c.shown,
            root_relative: c.root_relative,
            uri: c.uri,
            problems: kept.as_ref().map_or(&c.problems[..], |k| &k[i]),
        })
        .collect();
    summary.not_shown = not_shown.count;

    let write = |out: &mut dyn Write| -> std::io::Result<()> {
        if a.statistics {
            output::statistics(&shown, out)?;
        } else {
            output::render(format, &shown, &summary, out)?;
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
    if let Some(line) = output::not_shown_line(&not_shown) {
        eprintln!("{line}");
    }

    let failed = summary.errors > 0 || a.max_warnings.is_some_and(|m| summary.warnings > m);
    Ok(if failed_run {
        2
    } else if failed && !a.exit_zero {
        1
    } else {
        0
    })
}

/// A problem's severity as `--min-severity` names it; none is an error.
fn severity(p: &Problem) -> super::args::Severity {
    use super::args::Severity;
    match p.diagnostic.severity {
        Some(DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(DiagnosticSeverity::INFORMATION) => Severity::Info,
        Some(DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

/// Which problems the report prints: in each file the `per_file` most
/// severe, then over the run the `total` most severe, ties in report order,
/// printed in report order; and what that left out. None: no cap. Choosing
/// by severity keeps an error from hiding behind hints in earlier files.
/// `None` when no cap leaves anything out: every problem is printed as it is.
fn budget(
    checked: &[Checked],
    per_file: Option<usize>,
    total: Option<usize>,
) -> (Option<Vec<Vec<Problem>>>, output::NotShown) {
    let mut not_shown = output::NotShown::default();
    let per_file_cap = per_file.unwrap_or(usize::MAX);
    let within: usize = checked
        .iter()
        .map(|c| c.problems.len().min(per_file_cap))
        .sum();
    if within == checked.iter().map(|c| c.problems.len()).sum::<usize>()
        && within <= total.unwrap_or(usize::MAX)
    {
        return (None, not_shown);
    }
    // (severity, file, index): sorted, the most severe first, then report order.
    let mut pool = Vec::new();
    for (f, c) in checked.iter().enumerate() {
        let mut ranked: Vec<_> = c
            .problems
            .iter()
            .enumerate()
            .map(|(i, p)| (severity(p), f, i))
            .collect();
        ranked.sort_unstable();
        if let Some(n) = per_file
            && ranked.len() > n
        {
            not_shown.max_diagnostics = per_file;
            ranked.truncate(n);
        }
        pool.extend(ranked);
    }
    pool.sort_unstable();
    if let Some(n) = total
        && pool.len() > n
    {
        not_shown.max_problems = total;
        pool.truncate(n);
    }
    pool.sort_unstable_by_key(|&(_, f, i)| (f, i));
    let mut kept = vec![Vec::new(); checked.len()];
    for &(_, f, i) in &pool {
        kept[f].push(checked[f].problems[i].clone());
    }
    for c in checked {
        for p in &c.problems {
            not_shown.by_severity[severity(p) as usize] += 1;
        }
    }
    for &(s, _, _) in &pool {
        not_shown.by_severity[s as usize] -= 1;
    }
    not_shown.count = not_shown.by_severity.iter().sum();
    (Some(kept), not_shown)
}
