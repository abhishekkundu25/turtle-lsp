//! SHACL validation scheduling: the validation thread, the latest job's
//! generation and the results shown.
//!
//! Each validation is a job with a generation; a newer job supersedes the
//! one going on, and the outcome of a superseded job is dropped. The thread
//! starts when the workspace first has shapes. Embedded (no threads), a job
//! runs at once with a time limit of its own ([`run_inline`]).

#[cfg(not(target_arch = "wasm32"))]
use super::worker::Fed;
use super::worker::Feed;
use crate::analysis::{Analysis, Encoding};
use crate::clock::Instant;
use crate::features::{diagnostics, shacl_validate};
use crate::knowledge::Source;
use crate::settings::{Settings, UnresolvedImports};
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{Receiver, RecvError};
use lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString};
use std::sync::Arc;
use std::time::Duration;

/// How long an embedded host (no threads) waits for one SHACL validation.
const INLINE_SHACL_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(super) struct Shacl {
    /// The validation thread, started when the workspace first has shapes.
    worker: Option<shacl_validate::Worker>,
    feed: Feed<shacl_validate::Outcome>,
    /// The latest validation job; older results are dropped.
    generation: u64,
    /// The latest validation results per document.
    diagnostics: shacl_validate::ByUri,
}

impl Shacl {
    /// The latest validation results per document.
    pub(super) fn diagnostics(&self) -> &shacl_validate::ByUri {
        &self.diagnostics
    }

    /// The thread's outcomes, for the main loop to select on.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn rx(&self) -> Receiver<shacl_validate::Outcome> {
        self.feed.rx()
    }

    /// What a receive on [`Shacl::rx`] found.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn take(
        &mut self,
        got: Result<shacl_validate::Outcome, RecvError>,
    ) -> Fed<shacl_validate::Outcome> {
        self.feed.take(got)
    }

    /// A new validation of `docs` (every source of the knowledge index), which
    /// supersedes the one going on: the job when `settings` enable it and a
    /// workspace document has shapes. Otherwise None, and the run going on is
    /// cancelled. `unresolved` gives the imports not loaded.
    pub(super) fn job(
        &mut self,
        docs: Vec<(Source, Arc<Analysis>)>,
        settings: &Settings,
        enc: Encoding,
        unresolved: impl FnOnce() -> Vec<String>,
    ) -> Option<shacl_validate::Job> {
        self.generation += 1;
        let wanted = settings.shacl_enabled
            && docs
                .iter()
                .any(|(s, a)| *s != Source::External && shacl_validate::has_shapes(a));
        if !wanted {
            if let Some(w) = &self.worker {
                w.cancel(self.generation);
            }
            return None;
        }
        Some(shacl_validate::Job {
            generation: self.generation,
            docs,
            max_triples: settings.shacl_max_triples,
            enc,
            unresolved: unresolved(),
            soften: settings.imports_unresolved == UnresolvedImports::Soften,
        })
    }

    /// Validate `job` on the thread, started first if it is not running.
    pub(super) fn submit(&mut self, job: shacl_validate::Job) {
        let worker = self.worker.get_or_insert_with(|| {
            let (worker, rx) = shacl_validate::Worker::start();
            self.feed = Feed::open(rx);
            worker
        });
        worker.submit(job);
    }

    /// `o` is the outcome of the latest job, not of a superseded one.
    pub(super) fn is_current(&self, o: &shacl_validate::Outcome) -> bool {
        o.generation == self.generation
    }

    /// Show `diagnostics` as the latest results: whether they differ from
    /// those shown.
    pub(super) fn set_results(&mut self, diagnostics: shacl_validate::ByUri) -> bool {
        if diagnostics == self.diagnostics {
            return false;
        }
        self.diagnostics = diagnostics;
        true
    }

    /// Clear the results shown: whether there were any.
    pub(super) fn clear(&mut self) -> bool {
        if self.diagnostics.is_empty() {
            return false;
        }
        self.diagnostics.clear();
        true
    }

    /// The validation thread is gone; the next job starts another.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn gone(&mut self) {
        self.worker = None;
    }
}

/// Validate `job` here and now. Nothing else runs meanwhile, so the run is
/// held to a time limit of its own, besides the budget every validation has.
pub(super) fn run_inline(job: &shacl_validate::Job) -> shacl_validate::Outcome {
    let deadline = Instant::now() + INLINE_SHACL_LIMIT;
    let diagnostics = shacl_validate::run(job, &move || Instant::now() >= deadline)
        .unwrap_or_else(|| shacl_stopped(job));
    shacl_validate::Outcome {
        generation: job.generation,
        diagnostics,
    }
}

/// What a validation that hit its time limit shows: a warning on each shapes
/// file that the results are missing, not a clean bill of health.
fn shacl_stopped(job: &shacl_validate::Job) -> shacl_validate::ByUri {
    job.docs
        .iter()
        .filter(|(s, a)| *s != Source::External && shacl_validate::has_shapes(a))
        .map(|(_, a)| {
            let d = Diagnostic {
                severity: Some(DiagnosticSeverity::WARNING),
                code: Some(NumberOrString::String(shacl_validate::SKIPPED_CODE.into())),
                source: Some(diagnostics::SOURCE.to_string()),
                message: format!(
                    "SHACL validation stopped after {} s, its time limit: the results are missing",
                    INLINE_SHACL_LIMIT.as_secs()
                ),
                ..Default::default()
            };
            (a.uri.clone(), vec![d])
        })
        .collect()
}

/// Counts of a validation's results by severity, e.g. "2 errors, 1 warning
/// in 2 files", or "No problems". Skip notices are not results.
pub(super) fn violation_summary(by_uri: &shacl_validate::ByUri) -> String {
    let is_result = |d: &&Diagnostic| !matches!(&d.code, Some(NumberOrString::String(c)) if c == shacl_validate::SKIPPED_CODE);
    let (mut errors, mut warnings, mut info, mut files) = (0, 0, 0, 0);
    for diags in by_uri.values() {
        let mut any = false;
        for d in diags.iter().filter(is_result) {
            any = true;
            match d.severity {
                Some(DiagnosticSeverity::WARNING) => warnings += 1,
                Some(DiagnosticSeverity::INFORMATION) | Some(DiagnosticSeverity::HINT) => info += 1,
                _ => errors += 1,
            }
        }
        files += usize::from(any);
    }
    let plural = |n: usize, one: &str, many: &str| match n {
        0 => None,
        1 => Some(format!("1 {one}")),
        n => Some(format!("{n} {many}")),
    };
    let parts: Vec<String> = [
        plural(errors, "error", "errors"),
        plural(warnings, "warning", "warnings"),
        plural(info, "info", "info"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if parts.is_empty() {
        return "No problems".into();
    }
    let files = if files == 1 {
        "1 file".to_string()
    } else {
        format!("{files} files")
    };
    format!("{} in {files}", parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn violation_summary_counts_results_and_files() {
        use lsp_types::DiagnosticSeverity as S;
        let d = |code: &str, severity: S| Diagnostic {
            code: Some(NumberOrString::String(code.into())),
            severity: Some(severity),
            ..Default::default()
        };
        let mut by = shacl_validate::ByUri::default();
        assert_eq!(violation_summary(&by), "No problems");
        by.insert("a".into(), vec![d("shacl-skipped", S::INFORMATION)]);
        assert_eq!(violation_summary(&by), "No problems");
        by.insert("b".into(), vec![d("shacl-violation", S::ERROR)]);
        assert_eq!(violation_summary(&by), "1 error in 1 file");
        by.insert(
            "c".into(),
            vec![
                d("shacl-violation", S::ERROR),
                d("shacl-violation", S::WARNING),
            ],
        );
        assert_eq!(violation_summary(&by), "2 errors, 1 warning in 2 files");
        by.insert("d".into(), vec![d("shacl-violation", S::INFORMATION)]);
        assert_eq!(
            violation_summary(&by),
            "2 errors, 1 warning, 1 info in 3 files"
        );
    }

    #[test]
    fn a_validation_that_hits_its_time_limit_says_so_on_the_shapes() {
        let text = "@prefix sh: <http://www.w3.org/ns/shacl#> .\n@prefix ex: <http://e.org/> .\nex:S a sh:NodeShape ; sh:targetNode ex:n ; sh:property [ sh:path ex:p ; sh:minCount 1 ] .\n";
        let a = Arc::new(Analysis::new(
            "file:///w/shapes.ttl".into(),
            text,
            Arc::default(),
            false,
        ));
        let job = shacl_validate::Job {
            generation: 1,
            docs: vec![(Source::Disk, a)],
            max_triples: 1000,
            enc: Encoding::Utf16,
            unresolved: vec![],
            soften: true,
        };
        // Cancelled at once, as a deadline already past would: no results ...
        assert!(shacl_validate::run(&job, &|| true).is_none());
        // ... and not a clean bill of health.
        let stopped = shacl_stopped(&job);
        let d = &stopped["file:///w/shapes.ttl"];
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].severity, Some(DiagnosticSeverity::WARNING));
        assert!(d[0].message.contains("stopped") && d[0].message.contains("time limit"));
        assert!(violation_summary(&stopped) == "No problems");
        // Uncancelled it validates.
        assert!(shacl_validate::run(&job, &|| false).is_some());
    }
}
