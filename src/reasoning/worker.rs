//! The reasoning thread: one job at a time, the newest queued one first, as
//! SHACL validation's ([`crate::features::shacl_validate::Worker`]).
//!
//! A run stops when the server trips its job's [`Cancel`] (newer work
//! superseded it) or at its time limit ([`Limits::run`]): TurbOWL checks
//! before each round, and the run again as it reads the inferred types. A
//! run reasons on a reasoner of its own and hands its result to the state
//! only when it finishes ([`Shared::publish`]), so hover reads the result
//! shown while a run goes on, and a stopped run changes nothing. When the
//! triples, the rules and the limit are those of the result shown, the run
//! shares that result's reasoner and reasons no round.

use super::diagnostics::{self, ByUri};
use super::state::{Limits, Reasoned, Shared};
use super::{Base, rules};
use crate::analysis::{Analysis, Encoding};
use crate::knowledge::Source;
use crate::settings::Reasoning;
use crate::syntax::PrefixMap;
use crossbeam_channel::{Receiver, Sender, unbounded};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One run: the workspace's sources (the knowledge index's, imports
/// included) and the settings it runs with.
pub(crate) struct Job {
    pub(crate) generation: u64,
    pub(crate) docs: Vec<(Source, Arc<Analysis>)>,
    pub(crate) rules: Reasoning,
    /// The run is skipped when the sources have more triples than this, and
    /// reads at most this many inferred triples for the inferred types.
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
    /// Stops the run: the server trips it when newer work supersedes it.
    pub(crate) cancel: Cancel,
}

/// The handle that stops a job's run from another thread. Clones share it;
/// once tripped it stays tripped.
#[derive(Clone, Debug, Default)]
pub(crate) struct Cancel(turbowl::Cancel);

impl Cancel {
    /// Stop the run at its next check.
    pub(crate) fn cancel(&self) {
        self.0.cancel();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// What a run gives the server: its diagnostics per document URI. A
/// finished run's result waits in [`Shared`] until the server shows it
/// ([`Shared::show`]) or drops the run ([`Shared::discard`]).
pub(crate) struct Outcome {
    pub(crate) generation: u64,
    pub(crate) diagnostics: ByUri,
    /// Why the run stopped before it finished, with no diagnostics; `None`
    /// when it finished, was skipped or crashed.
    pub(crate) stopped: Option<Stopped>,
}

/// Why a run stopped before it finished. It left the state as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stopped {
    /// Newer work superseded it (its job's [`Cancel`]).
    Superseded,
    /// It reached its time limit, `limit`. `kept` is the generation of the
    /// result shown, which hover goes on reading; `None` when none is
    /// shown.
    TimeLimit { limit: Duration, kept: Option<u64> },
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

/// Run `job` here and now, handing its result to `shared`, within the
/// state's time limit ([`Limits::run`]). A panic is caught: the run leaves
/// no result, the open documents say so, and the next job starts afresh.
pub(crate) fn run_inline(shared: &Shared, job: &Job) -> Outcome {
    let ran = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(shared, job)));
    let (diagnostics, stopped) = match ran {
        Ok(Ok(diagnostics)) => (diagnostics, None),
        Ok(Err(stopped)) => (ByUri::default(), Some(stopped)),
        Err(e) => {
            shared.publish(job.generation, None);
            (diagnostics::crashed(&job.docs, &*e), None)
        }
    };
    Outcome {
        generation: job.generation,
        diagnostics,
        stopped,
    }
}

/// When a run must stop: its job was cancelled, or its deadline passed by
/// the server's clock (the page's in the browser, which has no thread to
/// trip a handle).
#[derive(Clone)]
struct Until {
    cancel: Cancel,
    deadline: crate::clock::Instant,
}

impl Until {
    fn new(job: &Job, limit: Duration) -> Until {
        Until {
            cancel: job.cancel.clone(),
            deadline: crate::clock::Instant::now() + limit,
        }
    }

    fn passed(&self) -> bool {
        self.cancel.is_cancelled() || crate::clock::Instant::now() >= self.deadline
    }

    /// A TurbOWL handle that trips when this says so.
    fn handle(&self) -> turbowl::Cancel {
        let until = self.clone();
        turbowl::Cancel::with_check(move || until.passed())
    }
}

/// Reason over `job`'s sources and hand the result to `shared`
/// ([`Shared::publish`]); Err when the run stopped first, leaving `shared`
/// as it was.
fn run(shared: &Shared, job: &Job) -> Result<ByUri, Stopped> {
    let limits = shared.limits;
    let stopped = || {
        if job.cancel.is_cancelled() {
            Stopped::Superseded
        } else {
            Stopped::TimeLimit {
                limit: limits.run,
                kept: shared.shown_generation(),
            }
        }
    };
    let Some(switches) = rules(job.rules) else {
        shared.publish(job.generation, None);
        return Ok(ByUri::default());
    };
    let total: u64 = job
        .docs
        .iter()
        .map(|(_, a)| a.parsed.triples.len() as u64)
        .sum();
    if total > job.max_triples {
        // A stale result would answer for text it was not run on.
        shared.publish(job.generation, None);
        return Ok(diagnostics::skipped(&job.docs, total, job.max_triples));
    }
    let until = Until::new(job, limits.run);
    let base = Base::new(job.docs.clone(), job.enc);
    // The result shown, not one waiting: a newer job superseded its run.
    if let Some(last) = shared.shown().filter(|r| unchanged(r, job, &base)) {
        return rerun(shared, &last, job, base, limits);
    }
    let mut reasoner = turbowl::Reasoner::with_switches(switches);
    reasoner.set_base(base.triples());
    #[cfg(test)]
    shared
        .reasoned
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if reasoner.reason_cancellable(&until.handle()).is_err() {
        return Err(stopped());
    }
    let Some((types, cut)) = inferred_types(&reasoner, job.max_triples, &until) else {
        return Err(stopped());
    };
    // Superseded: its findings would be dropped.
    if job.cancel.is_cancelled() {
        return Err(Stopped::Superseded);
    }
    let (diagnostics, cache) = findings(&reasoner, &base, job, limits);
    let result = Reasoned {
        generation: job.generation,
        rules: job.rules,
        max_triples: job.max_triples,
        reasoner: Arc::new(reasoner),
        base,
        types: Arc::new(types),
        cut,
        cache: Mutex::new(cache),
    };
    shared.publish(job.generation, Some(result));
    Ok(diagnostics)
}

/// `r` ran with the same rules and limit over the same triples as `job`
/// would.
fn unchanged(r: &Reasoned, job: &Job, base: &Base) -> bool {
    r.rules == job.rules && r.max_triples == job.max_triples && same_triples(&r.base, base)
}

/// `job` over the triples of `last`, the result shown ([`unchanged`]): no
/// round reasoned, its findings found with `last`'s reasoner, and a result
/// that shares `last`'s reasoner and inferred types, with `base` (the new
/// places of its triples) and a cache of its own.
fn rerun(
    shared: &Shared,
    last: &Reasoned,
    job: &Job,
    base: Base,
    limits: Limits,
) -> Result<ByUri, Stopped> {
    if job.cancel.is_cancelled() {
        return Err(Stopped::Superseded);
    }
    let (diagnostics, cache) = findings(&last.reasoner, &base, job, limits);
    let result = Reasoned {
        generation: job.generation,
        rules: last.rules,
        max_triples: last.max_triples,
        reasoner: Arc::clone(&last.reasoner),
        base,
        types: Arc::clone(&last.types),
        cut: last.cut,
        cache: Mutex::new(cache),
    };
    shared.publish(job.generation, Some(result));
    Ok(diagnostics)
}

/// `a` and `b` hold the same triples (each holds a triple once).
fn same_triples(a: &Base, b: &Base) -> bool {
    let a: FxHashSet<_> = a.triples().collect();
    let mut n = 0;
    for t in b.triples() {
        if !a.contains(&t) {
            return false;
        }
        n += 1;
    }
    n == a.len()
}

/// The run's findings, with the explainer's cache their proofs filled; none
/// unless the job asks for them.
fn findings(
    reasoner: &turbowl::Reasoner,
    base: &Base,
    job: &Job,
    limits: Limits,
) -> (ByUri, turbowl::ExplainCache) {
    if !job.findings {
        return Default::default();
    }
    diagnostics::of(reasoner, base, job.related, &job.project, limits.findings)
}

/// Each IRI's inferred types that are IRIs, sorted, read from at most `max`
/// inferred triples, and whether reading stopped there; `None` when `until`
/// passed, asked every [`super::infer::CHECK_EVERY`] triples.
#[allow(clippy::type_complexity)]
fn inferred_types(
    r: &turbowl::Reasoner,
    max: u64,
    until: &Until,
) -> Option<(FxHashMap<String, Vec<String>>, bool)> {
    use super::infer::CHECK_EVERY;
    use spargebra::term::{NamedOrBlankNode, Term};
    let mut types: FxHashMap<String, Vec<String>> = FxHashMap::default();
    let mut cut = false;
    for (i, t) in r.inferred().enumerate() {
        if i % CHECK_EVERY == CHECK_EVERY - 1 && until.passed() {
            return None;
        }
        if i as u64 >= max {
            cut = true;
            break;
        }
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
    Some((types, cut))
}

#[cfg(test)]
mod tests {
    use super::super::base::tests::doc;
    use super::super::state::{Limits, NoProof, Types};
    use super::*;
    use std::sync::atomic::Ordering;

    const EX: &str = "http://example.org/";

    fn job(generation: u64, docs: Vec<(Source, Arc<Analysis>)>) -> Job {
        Job {
            generation,
            docs,
            rules: Reasoning::Default,
            max_triples: 1_000_000,
            enc: Encoding::Utf16,
            related: true,
            findings: true,
            project: Default::default(),
            cancel: Default::default(),
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

    fn proved(t: Types) -> Arc<Vec<super::super::state::Inferred>> {
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
        assert!(shared.shown().is_some());
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
        assert!(shared.shown().is_some());
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
        assert!(shared.shown().is_none());
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
            let r = shared.shown().unwrap();
            let cache = r.cache.lock().unwrap();
            let t = shared.inferred_types(&rex, Duration::from_millis(50));
            assert!(
                matches!(&t, Types::Pending(c) if c == &[format!("{EX}Animal")]),
                "pending"
            );
            drop(cache);
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            match shared.inferred_types(&rex, Duration::from_secs(10)) {
                Types::Proved(v) => {
                    assert!(v[0].proof.is_ok());
                    break;
                }
                _ if std::time::Instant::now() < deadline => {}
                _ => panic!("no proof"),
            }
        }
    }

    /// `ex:fido a ex:Dog`, in a document of its own.
    fn fido() -> (Source, Arc<Analysis>) {
        doc(
            Source::Disk,
            "file:///w/b.ttl",
            "@prefix ex: <http://example.org/> .\nex:fido a ex:Dog .\n",
        )
    }

    /// The inferred types of `ex:fido`, with a proof each.
    fn types_of_fido(shared: &Arc<Shared>) -> Vec<String> {
        let fido = proved(shared.inferred_types(&format!("{EX}fido"), Duration::from_secs(10)));
        assert!(fido.iter().all(|t| t.proof.is_ok()));
        fido.iter().map(|t| t.class.clone()).collect()
    }

    #[test]
    fn a_finished_run_is_not_read_before_the_server_shows_it() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        let out = run_inline(&shared, &job(2, vec![animals(), fido()]));
        assert_eq!(out.stopped, None);
        // Hover reads the result shown: no ex:fido in it.
        assert!(types_of_fido(&shared).is_empty());
        animal_of_rex(&shared);
        shared.show(2);
        assert_eq!(types_of_fido(&shared), [format!("{EX}Animal")]);
    }

    #[test]
    fn timing_a_dropped_run_leaves_the_result_shown_and_is_never_shown() {
        let whole = whole_run();
        let limit = whole / 10;
        let shared = Arc::new(Shared::with_limits(
            crate::server::worker::Spawn::Thread,
            Limits {
                run: limit,
                ..Limits::default()
            },
        ));
        shown(&shared, &job(1, vec![animals()]));
        // A run that finished, but an edit made it stale: the server drops
        // it, and its result goes.
        let out = run_inline(&shared, &job(2, vec![animals(), fido()]));
        assert_eq!(out.stopped, None);
        assert!(matches!(shared.fresh_for_test(), Some((2, Some(_)))));
        shared.discard(2);
        assert!(shared.fresh_for_test().is_none());
        animal_of_rex(&shared);
        assert!(types_of_fido(&shared).is_empty());
        // The next run stops at its time limit: the result kept is the one
        // shown, never the dropped run's.
        let out = run_inline(&shared, &job(3, vec![animals(), fido(), slow()]));
        assert_eq!(
            out.stopped,
            Some(Stopped::TimeLimit {
                limit,
                kept: Some(1)
            })
        );
        assert_eq!(shared.shown_generation(), Some(1));
        animal_of_rex(&shared);
        assert!(types_of_fido(&shared).is_empty());
    }

    #[test]
    fn timing_a_run_over_the_same_triples_takes_no_lock_hovers_wait_on() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        // A finding whose proof takes long to search: the run's findings
        // take up to their time limit.
        let docs = vec![animals(), chain(400)];
        shown(&shared, &job(1, docs.clone()));
        let first = shared.shown().unwrap();
        // Saved again: the same triples, and the findings found again.
        let rerun = {
            let shared = Arc::clone(&shared);
            let again = job(2, docs);
            std::thread::spawn(move || run_inline(&shared, &again))
        };
        let mut during = 0;
        while !rerun.is_finished() {
            // The result shown, never "running".
            animal_of_rex(&shared);
            during += 1;
        }
        assert!(during > 0, "the run ended before the first hover");
        let out = rerun.join().unwrap();
        assert_eq!(out.stopped, None);
        assert_eq!(shared.reasoned.load(Ordering::Relaxed), 1);
        // Its result shares the reasoner and the inferred types, and waits
        // for the server to show it.
        let Some((2, Some(fresh))) = shared.fresh_for_test() else {
            panic!("no result waits");
        };
        assert!(Arc::ptr_eq(&fresh.reasoner, &first.reasoner));
        assert!(Arc::ptr_eq(&fresh.types, &first.types));
        assert_eq!(shared.shown_generation(), Some(1));
        shared.show(2);
        assert_eq!(shared.shown_generation(), Some(2));
        animal_of_rex(&shared);
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
            let r = shared.shown().unwrap();
            let cache = r.cache.lock().unwrap();
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
        }
        // The search ends: its answer is the next hover's.
        let long = Duration::from_secs(10);
        assert!(proved(shared.inferred_types(&rex, long))[0].proof.is_ok());
        assert_eq!(searches(), 1);
        assert!(proved(shared.inferred_types(&fido, long))[0].proof.is_ok());
        assert!(proved(shared.inferred_types(&rex, long))[0].proof.is_ok());
        assert_eq!(searches(), 3);
    }

    /// `ex:x` at the start of an `rdfs:subClassOf` chain of `links` links,
    /// with a class at its end disjoint from one `ex:x` has: a hover on
    /// `ex:x` lists `links` inferred types, and their proofs, as the finding
    /// about `ex:x`, take long to search (seconds at 200 links, minutes at
    /// 400, without a limit).
    fn chain(links: usize) -> (Source, Arc<Analysis>) {
        let mut text = String::from(
            "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             ex:x a ex:C0 , ex:D .\n",
        );
        for i in 0..links {
            text.push_str(&format!("ex:C{i} rdfs:subClassOf ex:C{} .\n", i + 1));
        }
        text.push_str(&format!("ex:C{links} owl:disjointWith ex:D .\n"));
        doc(Source::Open, "file:///w/chain.ttl", &text)
    }

    const SHORT: Limits = Limits {
        run: Duration::from_secs(600),
        hover: Duration::from_millis(100),
        findings: Duration::from_millis(100),
    };

    #[test]
    fn timing_a_hover_search_stops_at_its_time_limit_and_still_lists_every_type() {
        for spawn in [
            crate::server::worker::Spawn::Thread,
            crate::server::worker::Spawn::Inline,
        ] {
            let shared = Arc::new(Shared::with_limits(spawn, SHORT));
            shown(&shared, &job(1, vec![chain(400)]));
            let start = std::time::Instant::now();
            let types = proved(shared.inferred_types(&format!("{EX}x"), Duration::from_secs(120)));
            // A generous bound: without the limit this takes minutes.
            assert!(
                start.elapsed() < Duration::from_secs(20),
                "{:?}",
                start.elapsed()
            );
            assert_eq!(types.len(), 400);
            assert!(
                types
                    .iter()
                    .any(|t| matches!(t.proof, Err(NoProof::Stopped))),
                "no type was stopped"
            );
        }
    }

    #[test]
    fn timing_under_the_limit_a_small_file_has_every_proof() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        let types = proved(shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(10)));
        assert!(types.iter().all(|t| t.proof.is_ok()));
    }

    /// The finding about `ex:x` in [`chain`]`(links)`, run under `limits`.
    fn chain_finding(links: usize, limits: Limits) -> lsp_types::Diagnostic {
        let shared = Shared::with_limits(crate::server::worker::Spawn::Thread, limits);
        let start = std::time::Instant::now();
        let mut out = run_inline(&shared, &job(1, vec![chain(links)]));
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{:?}",
            start.elapsed()
        );
        let found = out
            .diagnostics
            .remove("file:///w/chain.ttl")
            .unwrap_or_default();
        let mut disjoint = found.into_iter().filter(|d| d.message.contains("disjoint"));
        let d = disjoint.next().expect("the disjointness finding");
        assert!(disjoint.next().is_none());
        d
    }

    #[test]
    fn timing_findings_past_the_time_limit_keep_their_message_without_a_proof() {
        let d = chain_finding(400, SHORT);
        assert!(d.related_information.is_none(), "{d:#?}");
    }

    #[test]
    fn timing_under_the_limit_a_small_finding_has_its_proof() {
        let d = chain_finding(3, Limits::default());
        assert!(d.related_information.is_some_and(|r| !r.is_empty()));
    }

    #[test]
    fn timing_a_stopped_hover_is_the_answer_again_without_a_new_search() {
        for spawn in [
            crate::server::worker::Spawn::Thread,
            crate::server::worker::Spawn::Inline,
        ] {
            let shared = Arc::new(Shared::with_limits(spawn, SHORT));
            shown(&shared, &job(1, vec![chain(400)]));
            let x = format!("{EX}x");
            let first = proved(shared.inferred_types(&x, Duration::from_secs(60)));
            assert!(
                first
                    .iter()
                    .any(|t| matches!(t.proof, Err(NoProof::Stopped)))
            );
            for _ in 0..3 {
                let again = proved(shared.inferred_types(&x, Duration::from_secs(60)));
                assert!(Arc::ptr_eq(&first, &again));
            }
            assert_eq!(
                shared.searches.load(std::sync::atomic::Ordering::Relaxed),
                1
            );
        }
    }

    #[test]
    fn timing_a_costly_type_stops_only_itself() {
        // ex:x a ex:C400, each ex:C<i+1> a subclass of ex:C<i>: the farthest
        // class, ex:C0, sorts first and its proof is the longest.
        let mut text = String::from(
            "@prefix ex: <http://example.org/> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             ex:x a ex:C400 .\n",
        );
        for i in 0..400 {
            text.push_str(&format!("ex:C{} rdfs:subClassOf ex:C{i} .\n", i + 1));
        }
        let shared = Arc::new(Shared::with_limits(
            crate::server::worker::Spawn::Thread,
            // The limit users get: a far shorter one leaves no time for
            // the types after a hundred stopped ones on a slow machine.
            Limits::default(),
        ));
        shown(
            &shared,
            &job(1, vec![doc(Source::Open, "file:///w/down.ttl", &text)]),
        );
        let start = std::time::Instant::now();
        let types = proved(shared.inferred_types(&format!("{EX}x"), Duration::from_secs(60)));
        let near = types
            .iter()
            .find(|t| t.class == format!("{EX}C399"))
            .expect("ex:C399");
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "{:?}",
            start.elapsed()
        );
        assert!(near.proof.is_ok(), "the direct superclass has no proof");
    }

    /// A list of 32,000 members (`ex:K0` to `ex:K31999`) after `head`, with
    /// `tail` after it: TurbOWL before 0.2.2 took 17 to 55 s and gigabytes
    /// to build a proof through it, past any time limit.
    fn long_list(head: &str, tail: &str) -> Job {
        let members: Vec<String> = (0..32_000).map(|i| format!("ex:K{i}")).collect();
        let text = format!(
            "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             {head} ( {} ) .\n{tail}\n",
            members.join(" ")
        );
        let mut job = job(1, vec![doc(Source::Open, "file:///w/list.ttl", &text)]);
        job.max_triples = 1_000_000;
        job
    }

    #[test]
    fn timing_a_hover_through_a_long_list_keeps_its_limit() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(
            &shared,
            &long_list("ex:U owl:unionOf", "ex:x a ex:K31999 ."),
        );
        let start = std::time::Instant::now();
        let types = proved(shared.inferred_types(&format!("{EX}x"), Duration::from_secs(60)));
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        let union = types
            .iter()
            .find(|t| t.class == format!("{EX}U"))
            .expect("ex:U");
        assert!(union.proof.is_ok());
    }

    #[test]
    fn timing_a_finding_through_a_long_list_keeps_its_limit() {
        let shared = Shared::new(crate::server::worker::Spawn::Thread);
        let start = std::time::Instant::now();
        let mut out = run_inline(
            &shared,
            &long_list(
                "[] a owl:AllDisjointClasses ; owl:members",
                "ex:x a ex:K0 , ex:K1 .",
            ),
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        let found = out
            .diagnostics
            .remove("file:///w/list.ttl")
            .unwrap_or_default();
        // Within the limit now, with its proof.
        assert!(
            found.iter().any(|d| d
                .related_information
                .as_ref()
                .is_some_and(|r| !r.is_empty())),
            "{found:#?}"
        );
    }

    #[test]
    fn timing_findings_that_fit_in_the_limit_all_keep_their_proofs() {
        // 100 findings, each through a 4,000-member list. Their proofs take
        // some time T on this machine (about 0.4 s on an M-series Mac, more
        // on CI): under a limit they all fit in, with room, each keeps its
        // proof, though a cold first search would not fit an equal share.
        let members: Vec<String> = (0..4_000).map(|i| format!("ex:K{i}")).collect();
        let mut text = format!(
            "@prefix ex: <http://example.org/> .\n\
             @prefix owl: <http://www.w3.org/2002/07/owl#> .\n\
             [] a owl:AllDisjointClasses ; owl:members ( {} ) .\n",
            members.join(" ")
        );
        for j in 0..100 {
            text.push_str(&format!("ex:i{j} a ex:K{} , ex:K{} .\n", 2 * j, 2 * j + 1));
        }
        let mut job = job(1, vec![doc(Source::Open, "file:///w/many.ttl", &text)]);
        job.max_triples = 1_000_000;
        let unlimited = Limits {
            run: Duration::from_secs(600),
            findings: Duration::from_secs(600),
            ..Limits::default()
        };
        let start = std::time::Instant::now();
        run_inline(
            &Shared::with_limits(crate::server::worker::Spawn::Thread, unlimited),
            &job,
        );
        let room = start.elapsed() * 2 + super::super::state::Budget::RESERVE * 100;
        let limits = Limits {
            findings: room,
            ..Limits::default()
        };
        let shared = Shared::with_limits(crate::server::worker::Spawn::Thread, limits);
        let mut out = run_inline(&shared, &job);
        let found = out
            .diagnostics
            .remove("file:///w/many.ttl")
            .unwrap_or_default();
        assert_eq!(found.len(), 100);
        let unproved = found
            .iter()
            .filter(|d| {
                !d.related_information
                    .as_ref()
                    .is_some_and(|r| !r.is_empty())
            })
            .count();
        assert_eq!(unproved, 0);
    }

    #[test]
    fn inline_every_hover_has_its_proofs_at_once() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Inline));
        shown(&shared, &job(1, vec![animals()]));
        for _ in 0..2 {
            let t = proved(shared.inferred_types(&format!("{EX}rex"), Duration::ZERO));
            assert!(t[0].proof.is_ok());
        }
    }

    #[test]
    fn hover_says_a_run_is_going_on_while_a_first_result_waits_to_be_shown() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let rex = format!("{EX}rex");
        assert!(matches!(
            shared.inferred_types(&rex, Duration::from_secs(1)),
            Types::Unknown
        ));
        run_inline(&shared, &job(1, vec![animals()]));
        assert!(matches!(
            shared.inferred_types(&rex, Duration::from_secs(1)),
            Types::Running
        ));
        shared.show(1);
        animal_of_rex(&shared);
    }

    #[test]
    fn the_thread_runs_the_newest_job_and_writes_the_state() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let (worker, rx) = Worker::start(shared.clone()).unwrap();
        worker.submit(job(1, vec![animals()]));
        let out = rx.recv_timeout(Duration::from_secs(30)).unwrap();
        assert_eq!(out.generation, 1);
        assert!(matches!(shared.fresh_for_test(), Some((1, Some(_)))));
        // An edit: a new run over the new base.
        worker.submit(job(2, vec![animals(), fido()]));
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
        assert!(shared.shown().is_none());
        // On by default; off builds nothing.
        let settings = crate::settings::Settings::default();
        assert!(Shared::when_on(&settings, crate::server::worker::Spawn::Thread).is_some());
        let off = crate::settings::Settings {
            reasoning: Reasoning::Off,
            ..settings
        };
        assert!(Shared::when_on(&off, crate::server::worker::Spawn::Thread).is_none());
    }

    /// A long `rdfs:subClassOf` chain with members at its start: TurbOWL
    /// takes hundreds of milliseconds over it, in many rounds.
    fn slow() -> (Source, Arc<Analysis>) {
        let mut text = String::from(
            "@prefix ex: <http://example.org/> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n",
        );
        for i in 0..700 {
            text.push_str(&format!("ex:C{i} rdfs:subClassOf ex:C{} .\n", i + 1));
        }
        for j in 0..100 {
            text.push_str(&format!("ex:m{j} a ex:C0 .\n"));
        }
        doc(Source::Open, "file:///w/slow.ttl", &text)
    }

    /// How long a run over [`animals`] and [`slow`] takes here, with no
    /// time limit.
    fn whole_run() -> Duration {
        let unlimited = Limits {
            run: Duration::from_secs(600),
            ..Limits::default()
        };
        let shared = Shared::with_limits(crate::server::worker::Spawn::Thread, unlimited);
        let start = std::time::Instant::now();
        let out = run_inline(&shared, &job(1, vec![animals(), slow()]));
        let whole = start.elapsed();
        assert_eq!(out.stopped, None);
        assert!(
            whole >= Duration::from_millis(200),
            "too quick to measure: {whole:?}"
        );
        whole
    }

    fn animal_of_rex(shared: &Arc<Shared>) {
        let rex = proved(shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(10)));
        assert_eq!(rex.len(), 1);
        assert_eq!(rex[0].class, format!("{EX}Animal"));
        assert!(rex[0].proof.is_ok());
    }

    #[test]
    fn timing_a_run_stops_at_its_time_limit_and_hover_keeps_the_last_result() {
        let whole = whole_run();
        let limit = whole / 10;
        let shared = Arc::new(Shared::with_limits(
            crate::server::worker::Spawn::Thread,
            Limits {
                run: limit,
                ..Limits::default()
            },
        ));
        shown(&shared, &job(1, vec![animals()]));
        let start = std::time::Instant::now();
        let out = run_inline(&shared, &job(2, vec![animals(), slow()]));
        let took = start.elapsed();
        assert_eq!(
            out.stopped,
            Some(Stopped::TimeLimit {
                limit,
                kept: Some(1)
            })
        );
        assert!(out.diagnostics.is_empty());
        assert!(
            took < whole / 2,
            "the limit stopped it after {took:?} of a {whole:?} run"
        );
        // The result shown stays, and hover reads it.
        assert_eq!(shared.shown_generation(), Some(1));
        animal_of_rex(&shared);
        // With no result before it, a stopped run leaves none.
        let shared = Arc::new(Shared::with_limits(
            crate::server::worker::Spawn::Inline,
            Limits {
                run: limit,
                ..Limits::default()
            },
        ));
        let out = run_inline(&shared, &job(1, vec![animals(), slow()]));
        assert_eq!(out.stopped, Some(Stopped::TimeLimit { limit, kept: None }));
        assert_eq!(shared.shown_generation(), None);
        assert!(shared.fresh_for_test().is_none());
    }

    #[test]
    fn timing_a_cancel_stops_the_run_and_hover_reads_the_last_result_meanwhile() {
        let whole = whole_run();
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        shown(&shared, &job(1, vec![animals()]));
        let next = job(2, vec![animals(), slow()]);
        let cancel = next.cancel.clone();
        let start = std::time::Instant::now();
        let running = {
            let shared = Arc::clone(&shared);
            std::thread::spawn(move || run_inline(&shared, &next))
        };
        std::thread::sleep(whole / 10);
        // While it runs, hover reads the last result, not "running".
        animal_of_rex(&shared);
        cancel.cancel();
        let out = running.join().unwrap();
        let took = start.elapsed();
        assert_eq!(out.stopped, Some(Stopped::Superseded));
        assert!(
            took < whole / 2,
            "the cancel stopped it after {took:?} of a {whole:?} run"
        );
        assert_eq!(shared.shown_generation(), Some(1));
        assert!(shared.fresh_for_test().is_none());
        animal_of_rex(&shared);
    }

    #[test]
    fn the_inferred_types_are_read_up_to_max_triples_and_hover_is_told() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        // Six triples, and more inferred.
        let tight = Job {
            max_triples: 6,
            ..job(1, vec![chain(3)])
        };
        let out = shown(&shared, &tight);
        assert_eq!(out.stopped, None);
        assert_eq!(shared.cut(), Some(6));
        let x = proved(shared.inferred_types(&format!("{EX}x"), Duration::from_secs(10)));
        assert!(x.len() <= 3, "{}", x.len());
        // The findings do not read the inferred types: they are all there.
        assert!(
            out.diagnostics["file:///w/chain.ttl"]
                .iter()
                .any(|d| d.message.contains("disjoint"))
        );
        shown(&shared, &job(2, vec![chain(3)]));
        assert_eq!(shared.cut(), None);
        let x = proved(shared.inferred_types(&format!("{EX}x"), Duration::from_secs(10)));
        assert_eq!(x.len(), 3);
    }

    #[test]
    fn a_run_over_the_same_triples_reasons_no_round_and_takes_their_new_places() {
        let shared = Arc::new(Shared::new(crate::server::worker::Spawn::Thread));
        let reasoned = || shared.reasoned.load(Ordering::Relaxed);
        shown(&shared, &job(1, vec![animals()]));
        assert_eq!(reasoned(), 1);
        // A blank line first: the same triples, each a line lower.
        let moved = doc(
            Source::Open,
            "file:///w/a.ttl",
            "\n@prefix ex: <http://example.org/> .\n\
             @prefix rdfs: <http://www.w3.org/2000/01/rdf-schema#> .\n\
             ex:Dog rdfs:subClassOf ex:Animal .\n\
             ex:rex a ex:Dog .\n",
        );
        shown(&shared, &job(2, vec![moved.clone()]));
        assert_eq!(reasoned(), 1);
        let rex = proved(shared.inferred_types(&format!("{EX}rex"), Duration::from_secs(10)));
        let mut lines: Vec<u32> = rex[0]
            .proof
            .as_ref()
            .unwrap()
            .leaves()
            .filter_map(|l| l.origin())
            .map(|o| o.line)
            .collect();
        lines.sort_unstable();
        assert_eq!(lines, [4, 5]);
        // Other rules, another limit or other triples reason again.
        let complete = Job {
            rules: Reasoning::Complete,
            ..job(3, vec![moved.clone()])
        };
        shown(&shared, &complete);
        assert_eq!(reasoned(), 2);
        let lower = Job {
            max_triples: 500_000,
            rules: Reasoning::Complete,
            ..job(4, vec![moved])
        };
        shown(&shared, &lower);
        assert_eq!(reasoned(), 3);
        shown(&shared, &job(5, vec![animals(), chain(3)]));
        assert_eq!(reasoned(), 4);
        // A cancelled job changes nothing, though it reasons no round.
        let cancelled = job(6, vec![animals(), chain(3)]);
        cancelled.cancel.cancel();
        assert_eq!(
            run_inline(&shared, &cancelled).stopped,
            Some(Stopped::Superseded)
        );
        assert_eq!(shared.shown_generation(), Some(5));
        assert!(shared.fresh_for_test().is_none());
    }
}
