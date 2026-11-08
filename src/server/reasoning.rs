//! Reasoning scheduling (`turtle.reasoning.rules`): the reasoning thread,
//! the latest job's generation and the findings shown.
//!
//! The state the runs write and hovers read is the server's
//! `reasoning` field ([`crate::reasoning::Shared`]); this group holds only
//! what drives the runs. Each run is a job with a generation; a run cannot
//! be stopped halfway, so a superseded one finishes and its outcome is
//! dropped. An edit supersedes the run going on at once, though the next
//! one starts only once typing pauses; the server marks the current run's
//! result as the one hover reads (`Shared::show`). The thread starts with the first job. Embedded (no threads), a
//! job runs at once ([`crate::reasoning::run_inline`]).
//!
//! With reasoning off the server holds no state and never calls this
//! group: no thread, no job.

#[cfg(not(target_arch = "wasm32"))]
use super::worker::Fed;
use super::worker::Feed;
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::reasoning::{ByUri, Job, Outcome, SKIPPED_CODE, Shared, Worker};
use crate::settings::Settings;
#[cfg(not(target_arch = "wasm32"))]
use crossbeam_channel::{Receiver, RecvError};
use std::sync::Arc;

#[derive(Default)]
pub(super) struct Runs {
    /// The reasoning thread, writing the server's current state; started
    /// with the first job after reasoning is turned on.
    worker: Option<Worker>,
    feed: Feed<Outcome>,
    /// The latest job; older outcomes are dropped.
    generation: u64,
    /// The latest findings per document.
    diagnostics: ByUri,
    /// The client shows related information (see [`Job::related`]).
    related: bool,
}

impl Runs {
    /// Runs for a client that shows related information (`related`) or not.
    pub(super) fn new(related: bool) -> Runs {
        Runs {
            related,
            ..Runs::default()
        }
    }

    /// The latest findings per document.
    pub(super) fn diagnostics(&self) -> &ByUri {
        &self.diagnostics
    }

    /// The thread's outcomes, for the main loop to select on.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn rx(&self) -> Receiver<Outcome> {
        self.feed.rx()
    }

    /// What a receive on [`Runs::rx`] found.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn take(&mut self, got: Result<Outcome, RecvError>) -> Fed<Outcome> {
        self.feed.take(got)
    }

    /// A new run over `docs` (every source of the knowledge index), which
    /// supersedes the one going on.
    pub(super) fn job(
        &mut self,
        docs: Vec<(Source, Arc<Analysis>)>,
        settings: &Settings,
        enc: Encoding,
    ) -> Job {
        self.generation += 1;
        Job {
            generation: self.generation,
            docs,
            rules: settings.reasoning,
            max_triples: settings.reasoning_max_triples,
            enc,
            related: self.related,
            findings: settings.reasoning_diagnostics,
            project: Default::default(),
        }
    }

    /// Run `job` on the thread, started first, writing `shared`, if it is
    /// not running. Err when the thread cannot be started: the job is
    /// dropped, and the next one tries again.
    pub(super) fn submit(&mut self, job: Job, shared: &Arc<Shared>) -> std::io::Result<()> {
        if self.worker.is_none() {
            let (worker, rx) = Worker::start(Arc::clone(shared))?;
            self.worker = Some(worker);
            self.feed = Feed::open(rx);
        }
        if let Some(worker) = &self.worker {
            worker.submit(job);
        }
        Ok(())
    }

    /// The workspace changed and the next job comes later (once typing
    /// pauses): the job going on is superseded, its outcome dropped.
    pub(super) fn invalidate(&mut self) {
        self.generation += 1;
    }

    /// The latest job's generation.
    #[cfg(test)]
    pub(super) fn generation(&self) -> u64 {
        self.generation
    }

    /// `o` is the outcome of the latest job, not of a superseded one.
    pub(super) fn is_current(&self, o: &Outcome) -> bool {
        o.generation == self.generation
    }

    /// Show `diagnostics` as the latest findings: whether they differ from
    /// those shown.
    pub(super) fn set_results(&mut self, diagnostics: ByUri) -> bool {
        if diagnostics == self.diagnostics {
            return false;
        }
        self.diagnostics = diagnostics;
        true
    }

    /// Reasoning was turned off: the thread ends once its run is done, its
    /// outcome is not listened for, and the findings are cleared. Whether
    /// there were any.
    pub(super) fn stop(&mut self) -> bool {
        self.generation += 1;
        self.gone();
        self.feed.close();
        let shown = !self.diagnostics.is_empty();
        self.diagnostics.clear();
        shown
    }

    /// The reasoning thread is gone, or could not start; the next job
    /// starts another.
    pub(super) fn gone(&mut self) {
        self.worker = None;
    }
}

/// What a run's progress ends with: the counts of its findings, or that it
/// was skipped.
pub(super) fn summary(by_uri: &ByUri) -> String {
    let skipped = by_uri.values().flatten().any(
        |d| matches!(&d.code, Some(lsp_types::NumberOrString::String(c)) if c == SKIPPED_CODE),
    );
    if skipped {
        return "Skipped: more triples than turtle.reasoning.maxTriples".into();
    }
    super::shacl::violation_summary(by_uri, SKIPPED_CODE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::worker::Spawn;
    use crossbeam_channel::RecvTimeoutError;
    use std::time::Duration;

    fn doc(text: &str) -> (Source, Arc<Analysis>) {
        let a = Analysis::new("file:///w/a.ttl".into(), text, Arc::default(), false);
        (Source::Open, Arc::new(a))
    }

    const DISJOINT: &str = "@prefix ex: <http://example.org/> .\n\
        @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
        ex:Cat owl:disjointWith ex:Dog .\n\
        ex:tom a ex:Cat , ex:Dog .\n";

    fn settings() -> Settings {
        Settings {
            reasoning: crate::settings::Reasoning::Default,
            reasoning_diagnostics: true,
            ..Settings::default()
        }
    }

    #[test]
    fn only_the_latest_job_is_current_and_stop_ends_the_thread() {
        let shared = Shared::when_on(&settings(), Spawn::Thread).unwrap();
        let mut runs = Runs::default();
        let first = runs.job(vec![doc(DISJOINT)], &settings(), Encoding::Utf16);
        let second = runs.job(vec![doc(DISJOINT)], &settings(), Encoding::Utf16);
        runs.submit(first, &shared).unwrap();
        runs.submit(second, &shared).unwrap();
        let rx = runs.rx();
        // The thread may take both, or only the newest.
        let mut current = None;
        while current.is_none() {
            let o = rx.recv_timeout(Duration::from_secs(30)).unwrap();
            if runs.is_current(&o) {
                current = Some(o);
            }
        }
        let o = current.unwrap();
        assert!(o.diagnostics["file:///w/a.ttl"].iter().any(|d| {
            d.code == Some(lsp_types::NumberOrString::String("turbowl/cax-dw".into()))
        }));
        assert!(runs.set_results(o.diagnostics));
        assert!(!runs.diagnostics().is_empty());
        // Off: the findings go, and the thread with them.
        assert!(runs.stop());
        assert!(runs.diagnostics().is_empty() && !runs.stop());
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(10)),
            Err(RecvTimeoutError::Disconnected)
        ));
        assert!(matches!(
            runs.rx().recv_timeout(Duration::from_millis(10)),
            Err(RecvTimeoutError::Timeout)
        ));
    }
}
