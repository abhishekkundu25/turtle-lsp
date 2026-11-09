//! The reasoning thread: one job at a time, the newest queued one first, as
//! SHACL validation's ([`crate::features::shacl_validate::Worker`]). A run
//! cannot be stopped halfway, so a superseded run finishes and the server
//! drops its outcome by generation; hover does not read the state it wrote
//! (see [`Shared::show`]), which the next run's replaces.

use super::diagnostics::{self, ByUri};
use super::state::{Reasoned, Shared};
use super::{Base, rules};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::settings::Reasoning;
use crate::syntax::PrefixMap;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::FxHashMap;
use std::sync::{Arc, Mutex};

/// One run: the workspace's sources (the knowledge index's, imports
/// included) and the settings it runs with.
pub(crate) struct Job {
    pub(crate) generation: u64,
    pub(crate) docs: Vec<(Source, Arc<Analysis>)>,
    pub(crate) rules: Reasoning,
    /// The run is skipped when the sources have more triples than this.
    pub(crate) max_triples: u64,
    pub(crate) enc: Encoding,
    /// The client shows a diagnostic's related information
    /// (`textDocument.publishDiagnostics.relatedInformation`): a finding's
    /// proof goes there instead of into its message.
    pub(crate) related: bool,
    /// TurbOWL's findings become diagnostics (`reasoning.diagnostics`);
    /// without, the run gives none and searches no proof for them.
    pub(crate) findings: bool,
    /// The project's prefixes of each document whose syntax takes none
    /// (N-Triples, N-Quads, JSON-LD), by URI: the findings name IRIs with
    /// them after the document's own. A Turtle or TriG document has them as
    /// its `external`.
    pub(crate) project: FxHashMap<String, Arc<PrefixMap>>,
}

/// What a run gives the server: its diagnostics per document URI (the
/// state it wrote is read through [`Shared`]).
pub(crate) struct Outcome {
    pub(crate) generation: u64,
    pub(crate) diagnostics: ByUri,
}

/// The reasoning thread. It blocks on its channel while idle and ends when
/// the worker is dropped.
pub(crate) struct Worker {
    jobs: Sender<Job>,
}

impl Worker {
    /// Start the thread, writing `shared`; Err when it cannot be started.
    pub(crate) fn start(shared: Arc<Shared>) -> std::io::Result<(Worker, Receiver<Outcome>)> {
        let (jobs, job_rx) = unbounded::<Job>();
        let (out_tx, out_rx) = unbounded();
        // Always a thread: the loop waits for jobs. Embedded, the server
        // runs `run_inline` instead and never starts one.
        crate::server::worker::Spawn::Thread.run(
            "reasoning",
            // Defence in depth: TurbOWL's recursion is bounded.
            16 << 20,
            move || {
                while let Ok(mut job) = job_rx.recv() {
                    // Only the newest queued job matters.
                    while let Ok(newer) = job_rx.try_recv() {
                        job = newer;
                    }
                    if out_tx.send(run_inline(&shared, &job)).is_err() {
                        break;
                    }
                }
            },
            // A job's panic is caught in `run_inline`; the loop's own cannot
            // reach the main loop but as the channel's end.
            |_| {},
        )?;
        Ok((Worker { jobs }, out_rx))
    }

    pub(crate) fn submit(&self, job: Job) {
        let _ = self.jobs.send(job);
    }
}

/// Run `job` here and now, writing `shared`. A panic is caught: the state
/// is left with no result, the open documents say so, and the next job
/// starts afresh.
pub(crate) fn run_inline(shared: &Shared, job: &Job) -> Outcome {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(shared, job)));
    let diagnostics = ran.unwrap_or_else(|e| {
        *shared.write() = None;
        diagnostics::crashed(&job.docs, &*e)
    });
    Outcome {
        generation: job.generation,
        diagnostics,
    }
}

/// Reason over `job`'s sources, reusing the last run's reasoner, and write
/// the result into `shared`.
fn run(shared: &Shared, job: &Job) -> ByUri {
    let Some(switches) = rules(job.rules) else {
        *shared.write() = None;
        return ByUri::default();
    };
    let total: u64 = job
        .docs
        .iter()
        .map(|(_, a)| a.parsed.triples.len() as u64)
        .sum();
    if total > job.max_triples {
        // A stale result would answer for text it was not run on.
        *shared.write() = None;
        return diagnostics::skipped(&job.docs, total, job.max_triples);
    }
    // Built before the lock is taken: hovers go on reading the last result.
    let base = Base::new(job.docs.clone(), job.enc);
    let mut held = shared.write();
    let mut reasoner = held
        .take()
        .map(|r| r.reasoner)
        .unwrap_or_else(|| turbowl::Reasoner::with_switches(switches));
    reasoner.set_switches(switches);
    reasoner.set_base(base.triples());
    reasoner.reason();
    let types = inferred_types(&reasoner);
    let (diagnostics, cache) = if job.findings {
        diagnostics::of(&reasoner, &base, job.related, &job.project)
    } else {
        Default::default()
    };
    *held = Some(Reasoned {
        generation: job.generation,
        reasoner,
        base,
        types,
        cache: Mutex::new(cache),
    });
    diagnostics
}

/// Each IRI's inferred types that are IRIs, sorted.
fn inferred_types(r: &turbowl::Reasoner) -> FxHashMap<String, Vec<String>> {
    use spargebra::term::{NamedOrBlankNode, Term};
    let mut types: FxHashMap<String, Vec<String>> = FxHashMap::default();
    for t in r.inferred() {
        if t.predicate.as_str() != crate::syntax::RDF_TYPE {
            continue;
        }
        if let (NamedOrBlankNode::NamedNode(s), Term::NamedNode(c)) = (t.subject, t.object) {
            types
                .entry(s.into_string())
                .or_default()
                .push(c.into_string());
        }
    }
    for classes in types.values_mut() {
        classes.sort_unstable();
        classes.dedup();
    }
    types
}

#[cfg(test)]
mod tests {
    use super::super::base::tests::doc;
    use super::super::state::Types;
    use super::*;
    use std::time::Duration;

    const EX: &str = "http://example.org/";

    fn job(generation: u64, docs: Vec<(Source, Arc<Analysis>)>) -> Job {
        Job {
            generation,
            docs,
            rules: Reasoning::Default,
            max_triples: 1000,
            enc: Encoding::Utf16,
            related: true,
            findings: true,
            project: Default::default(),
        }
    }

    fn animals() -> (Source, Arc<Analysis>) {
        doc(
            Source::Open,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             ex:Dog rdfs:subClassOf ex:Animal .\n\
             ex:rex a ex:Dog .\n",
        )
    }

    /// Run `job` inline, its result then shown as the server shows a
    /// current run's.
    fn shown(shared: &Shared, job: &Job) -> Outcome {
        let out = run_inline(shared, job);
        shared.show(job.generation);
        out
    }

    fn proved(t: Types) -> Vec<super::super::state::Inferred> {
        match t {
            Types::Proved(v) => v,
            _ => panic!("not proved"),
        }
    }

    #[test]
    fn a_subclass_gives_the_inferred_type_with_its_proof() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let out = shown(&shared, &job(7, vec![animals()]));
        assert_eq!(out.generation, 7);
        assert!(out.diagnostics.is_empty(), "{:?}", out.diagnostics);
        assert!(shared.read_for_test().is_some());
        let types = proved(shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(10)));
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].class, format!("{EX}Animal"));
        let proof = types[0].proof.as_ref().expect("a proof");
        assert!(matches!(
            proof.proof.reason(),
            turbowl::Reason::Rule {
                rule: "cax-sco",
                ..
            }
        ));
        let mut lines: Vec<u32> = proof
            .leaves()
            .filter_map(|l| l.origin())
            .map(|o| o.line)
            .collect();
        lines.sort_unstable();
        assert_eq!(lines, [3, 4]);
        // An asserted type is not inferred; a term with none has none.
        let dog = proved(shared.inferred_types(&format!("{EX}Dog"), Duration::from_secs(10)));
        assert!(dog.is_empty());
    }

    #[test]
    fn a_run_over_the_size_limit_is_skipped_and_leaves_no_result() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        assert!(shared.read_for_test().is_some());
        let small = Job {
            max_triples: 1,
            ..job(2, vec![animals()])
        };
        let out = shown(&shared, &small);
        let notes = &out.diagnostics["file:///w/a.ttl"];
        assert_eq!(notes.len(), 1);
        let code = notes[0].code.clone();
        assert_eq!(
            code,
            Some(lsp_types::NumberOrString::String(
                diagnostics::SKIPPED_CODE.into()
            ))
        );
        assert!(notes[0].message.contains("turtle.reasoning.maxTriples"));
        assert!(shared.read_for_test().is_none());
        assert!(matches!(
            shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(1)),
            Types::Unknown
        ));
    }

    #[test]
    fn a_disjointness_contradiction_is_a_diagnostic_with_its_proof() {
        let d = doc(
            Source::Open,
            "file:///w/a.ttl",
            "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             ex:Cat owl:disjointWith ex:Dog .\n\
             ex:tom a ex:Cat ,\n  ex:Dog .\n",
        );
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let out = shown(&shared, &job(1, vec![d]));
        let list = &out.diagnostics["file:///w/a.ttl"];
        let code = Some(lsp_types::NumberOrString::String("turbowl/cax-dw".into()));
        let found: Vec<_> = list.iter().filter(|d| d.code == code).collect();
        // On the individual's statement only: the class axiom is in the proof.
        let mut lines: Vec<u32> = found.iter().map(|d| d.range.start.line).collect();
        lines.sort_unstable();
        assert_eq!(lines, [3], "{list:?}");
        assert_eq!(
            found[0].message,
            "ex:tom is both a ex:Cat and a ex:Dog, which are disjoint."
        );
        let related = found[0].related_information.as_ref().expect("the proof");
        assert_eq!(related.len(), 3, "{related:?}");
        assert_eq!(
            found[0].severity,
            Some(lsp_types::DiagnosticSeverity::ERROR)
        );
    }

    #[test]
    fn hover_waits_at_most_its_limit_and_the_search_goes_on() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        let rex = format!("{EX}rex");
        {
            // A search that cannot finish yet: the cache is in use.
            let held = shared.read_for_test();
            let cache = held.as_ref().unwrap().cache.lock().unwrap();
            let t = shared.inferred_types(&rex, Duration::from_millis(50));
            assert!(
                matches!(&t, Types::Pending(c) if c == &[format!("{EX}Animal")]),
                "pending"
            );
            drop(cache);
            drop(held);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match shared.inferred_types(&rex, Duration::from_secs(10)) {
                Types::Proved(v) => {
                    assert!(v[0].proof.is_some());
                    break;
                }
                _ if std::time::Instant::now() < deadline => {}
                _ => panic!("no proof"),
            }
        }
    }

    #[test]
    fn hover_reads_only_the_result_of_the_run_shown() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let rex = format!("{EX}rex");
        shown(&shared, &job(1, vec![animals()]));
        // A run the server dropped, superseded while it ran.
        run_inline(&shared, &job(2, vec![animals()]));
        assert!(matches!(
            shared.inferred_types(&rex, Duration::from_secs(10)),
            Types::Running
        ));
        shared.show(2);
        assert_eq!(
            proved(shared.inferred_types(&rex, Duration::from_secs(10))).len(),
            1
        );
    }

    #[test]
    fn a_proof_search_whose_thread_cannot_start_is_pending_not_run_on_the_hover() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        let t = crate::server::worker::failing_starts(|| {
            shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(10))
        });
        assert!(
            matches!(&t, Types::Pending(c) if c == &[format!("{EX}Animal")]),
            "pending"
        );
    }

    #[test]
    fn one_proof_search_at_a_time_and_a_hover_again_gets_its_answer() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let more = doc(
            Source::Disk,
            "file:///w/b.ttl",
            "@prefix ex: <http://example.org/> .\nex:fido a ex:Dog .\n",
        );
        shown(&shared, &job(1, vec![animals(), more]));
        let (rex, fido) = (format!("{EX}rex"), format!("{EX}fido"));
        let searches = || shared.searches.load(std::sync::atomic::Ordering::Relaxed);
        let short = Duration::from_millis(20);
        {
            // The search cannot finish yet: the cache is in use.
            let held = shared.read_for_test();
            let cache = held.as_ref().unwrap().cache.lock().unwrap();
            assert!(matches!(
                shared.inferred_types(&rex, short),
                Types::Pending(_)
            ));
            assert_eq!(searches(), 1);
            // Hovering again waits on the same search; another term does
            // not start one beside it.
            assert!(matches!(
                shared.inferred_types(&rex, short),
                Types::Pending(_)
            ));
            assert!(matches!(
                shared.inferred_types(&fido, short),
                Types::Pending(_)
            ));
            assert_eq!(searches(), 1);
            drop(cache);
            drop(held);
        }
        // The search ends: its answer is the next hover's.
        let long = Duration::from_secs(10);
        assert!(proved(shared.inferred_types(&rex, long))[0].proof.is_some());
        assert_eq!(searches(), 1);
        assert!(
            proved(shared.inferred_types(&fido, long))[0]
                .proof
                .is_some()
        );
        assert!(proved(shared.inferred_types(&rex, long))[0].proof.is_some());
        assert_eq!(searches(), 3);
    }

    #[test]
    fn inline_every_hover_has_its_proofs_at_once() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Inline));
        shown(&shared, &job(1, vec![animals()]));
        for _ in 0..2 {
            let t = proved(shared.inferred_types(&format!("{EX}rex"), Duration::ZERO));
            assert!(t[0].proof.is_some());
        }
    }

    #[test]
    fn hover_says_a_run_is_writing_the_state() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        let held = shared.write();
        let t = shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(1));
        assert!(matches!(t, Types::Running));
        drop(held);
    }

    #[test]
    fn the_thread_runs_the_newest_job_and_writes_the_state() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let (worker, rx) = Worker::start(shared.clone()).unwrap();
        worker.submit(job(1, vec![animals()]));
        let out = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(out.generation, 1);
        assert!(shared.read_for_test().is_some());
        // An edit: the same reasoner runs again over the new base.
        let more = doc(
            Source::Disk,
            "file:///w/b.ttl",
            "@prefix ex: <http://example.org/> .\nex:fido a ex:Dog .\n",
        );
        worker.submit(job(2, vec![animals(), more]));
        let out = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(out.generation, 2);
        shared.show(2);
        let fido = proved(shared.inferred_types(&format!("{EX}fido"), Duration::from_secs(10)));
        assert_eq!(fido[0].class, format!("{EX}Animal"));
        let leaf_uris: Vec<&str> = fido[0]
            .proof
            .as_ref()
            .unwrap()
            .leaves()
            .filter_map(|l| l.origin())
            .map(|o| o.uri.as_str())
            .collect();
        assert!(leaf_uris.contains(&"file:///w/b.ttl"), "{leaf_uris:?}");
        drop(worker);
        assert!(rx.recv_timeout(Duration::from_secs(10)).is_err());
    }

    #[test]
    fn complete_rules_and_off() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let complete = Job {
            rules: Reasoning::Complete,
            ..job(1, vec![animals()])
        };
        shown(&shared, &complete);
        let rex = proved(shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(10)));
        assert!(rex.iter().any(|t| t.class == format!("{EX}Animal")));
        let off = Job {
            rules: Reasoning::Off,
            ..job(2, vec![animals()])
        };
        assert!(shown(&shared, &off).diagnostics.is_empty());
        assert!(shared.read_for_test().is_none());
        let settings = crate::settings::Settings::default();
        assert!(Shared::when_on(&settings, crate::server::worker::Spawn::Thread).is_none());
    }
}
