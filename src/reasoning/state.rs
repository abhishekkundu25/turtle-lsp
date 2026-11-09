//! The state the reasoning thread and hovers share: the reasoner after its
//! last run, that run's base, each IRI's inferred types and the
//! explainer's cache.
//!
//! Only the reasoning thread writes it, and it holds the write lock for a
//! whole run (the reasoner reuses its last run's work, so the run changes
//! it in place). A reader never waits for a run: it tries the lock and, when
//! a run holds it, says so ([`Types::Running`]). A result carries its run's
//! generation, and the server marks the run whose findings it shows
//! ([`Shared::show`]): a reader reads no other run's result, so a run
//! superseded while it ran (its findings dropped) shows nothing in hover
//! either, until the next run's result is shown ([`Types::Running`]).
//!
//! A proof is searched on a thread of its own under the read lock, one
//! search at a time (they take turns on the explainer's cache), and the
//! hover waits for it at most a given time. Past it the search goes on: a
//! hover on the same term waits for that search and has its answer, a hover
//! on another term says its proofs are still being worked out. A search
//! itself stops at its time limit ([`Limits::hover`]), shared out among the
//! term's types. Its answer, stopped or not, is kept for the later hovers
//! of the term until the next search, and what it found is in the cache
//! for the next.

use super::base::{Base, Origin};
use super::turtle::Items;
use crate::server::worker::Spawn;
use crate::settings::{Reasoning, Settings};
use crate::syntax::RDF_TYPE;
use rustc_hash::FxHashMap;
use spargebra::term::{NamedNode, Triple};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockWriteGuard, TryLockError};
use std::time::Duration;

/// How long a hover waits for proofs.
pub(crate) const HOVER_WAIT: Duration = Duration::from_millis(200);

/// How long proof searches may run: a search stops at its limit and keeps
/// what it found. Without a limit, a long class chain in a small file takes
/// minutes and gigabytes to explain.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// All the proofs of one hover's term.
    pub(crate) hover: Duration,
    /// All the explanations of one run's findings.
    pub(crate) findings: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            hover: Duration::from_secs(2),
            findings: Duration::from_secs(1),
        }
    }
}

/// One limit for searches made one after another (a term's types, a run's
/// findings): each gets an equal share of the time left, and what a quick
/// one leaves goes to the next, so one costly search stops only itself.
pub(crate) struct Budget {
    end: crate::clock::Instant,
}

impl Budget {
    pub(crate) fn new(limit: Duration) -> Budget {
        Budget {
            end: crate::clock::Instant::now() + limit,
        }
    }

    /// A cancel handle for the next of `left` searches, which trips at its
    /// share of the time left, by the server's clock (the page's in the
    /// browser, which has no threads to trip it); `None` once the time is
    /// up.
    pub(crate) fn next(&self, left: usize) -> Option<turbowl::Cancel> {
        let now = crate::clock::Instant::now();
        let rest = self.end.saturating_duration_since(now);
        if rest.is_zero() {
            return None;
        }
        let deadline = now + rest / u32::try_from(left.max(1)).unwrap_or(u32::MAX);
        Some(turbowl::Cancel::with_check(move || {
            crate::clock::Instant::now() >= deadline
        }))
    }
}

/// The reasoning state of a server with reasoning on.
pub(crate) struct Shared {
    /// How a proof search runs: on a thread of its own, or inline (the
    /// browser, which has no threads, and waits for it).
    spawn: Spawn,
    held: RwLock<Option<Reasoned>>,
    /// The generation of the run whose findings the server shows; only
    /// that run's result is read.
    shown: AtomicU64,
    /// The proof search started last, going on or done.
    search: Mutex<Option<Search>>,
    /// How long proof searches may run.
    pub(super) limits: Limits,
    /// Tests: how many proof searches have started.
    #[cfg(test)]
    pub(super) searches: std::sync::atomic::AtomicUsize,
}

/// The result of the last run that finished.
pub(super) struct Reasoned {
    /// The generation of the job the run was.
    pub(super) generation: u64,
    pub(super) reasoner: turbowl::Reasoner,
    pub(super) base: Base,
    /// Each IRI's inferred types that are IRIs, sorted.
    pub(super) types: FxHashMap<String, Vec<String>>,
    /// The explainer's indexes and what its searches found, for this
    /// reasoner: an explanation takes it and gives it back.
    pub(super) cache: Mutex<turbowl::ExplainCache>,
}

/// A proof search, for the hovers on its term to wait for.
struct Search {
    /// The term, and the generation of the result it reads.
    iri: String,
    generation: u64,
    /// Its answer, once there, until a hover takes it.
    answer: crossbeam_channel::Receiver<Result<Types, String>>,
    /// The search has ended: its answer has been sent.
    done: Arc<AtomicBool>,
    /// Its answer once a hover has taken it, for the later hovers of the
    /// term: the result it was searched in cannot change, and a search
    /// stopped at its limit would only stop there again.
    kept: Option<Arc<Vec<Inferred>>>,
}

/// What reasoning says of a term's types.
pub(crate) enum Types {
    /// No result to read: no run has finished, or the last one was skipped
    /// or failed.
    Unknown,
    /// A run is writing the state, or the result there is of a run
    /// superseded while it ran: the next run's is coming.
    Running,
    /// Each inferred type, with its proof. Shared: a search's answer is
    /// kept for the later hovers of its term.
    Proved(Arc<Vec<Inferred>>),
    /// The inferred types, whose proofs were not found in time. The search
    /// goes on, so asking again soon gives them.
    Pending(Vec<String>),
}

/// An inferred `rdf:type` of a term.
pub(crate) struct Inferred {
    /// The class IRI.
    pub(crate) class: String,
    pub(crate) proof: Result<Proof, NoProof>,
}

/// Why an inferred type has no proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoProof {
    /// The explainer found none. It finds one for every inferred triple, so
    /// this is a bug in it.
    NotFound,
    /// The search reached its time limit ([`Limits::hover`]) first.
    Stopped,
}

/// A proof, with where each base triple it rests on is written, and how
/// they are written (`turtle.rs`).
pub(crate) struct Proof {
    pub(super) proof: turbowl::Proof,
    pub(super) written: Items,
}

impl Proof {
    pub(super) fn new(proof: turbowl::Proof, base: &Base) -> Proof {
        // Per step: where it is written, for a base step in the base.
        let origins: Vec<Option<Origin>> = proof
            .steps()
            .iter()
            .map(|s| match s.reason {
                turbowl::Reason::Base => base.origin(triple(&s.statement)?),
                turbowl::Reason::Rule { .. } => None,
            })
            .collect();
        let written = Items::new(&proof, &origins, base);
        Proof { proof, written }
    }
}

/// A proof's statement as an RDF triple, when it is one.
fn triple(s: &turbowl::Statement) -> Option<oxrdf::TripleRef<'_>> {
    use oxrdf::NamedOrBlankNodeRef;
    use spargebra::term::Term;
    let subject: NamedOrBlankNodeRef = match &s.subject {
        Term::NamedNode(n) => n.into(),
        Term::BlankNode(b) => b.into(),
        _ => return None,
    };
    let Term::NamedNode(p) = &s.predicate else {
        return None;
    };
    Some(oxrdf::TripleRef::new(subject, p, &s.object))
}

impl Shared {
    pub(crate) fn new(spawn: Spawn) -> Shared {
        Shared::with_limits(spawn, Limits::default())
    }

    /// A state whose proof searches run at most `limits`.
    pub(crate) fn with_limits(spawn: Spawn, limits: Limits) -> Shared {
        Shared {
            spawn,
            held: RwLock::new(None),
            shown: AtomicU64::new(0),
            search: Mutex::new(None),
            limits,
            #[cfg(test)]
            searches: Default::default(),
        }
    }

    /// The server shows the findings of the run of `generation`: its
    /// result is the one to read.
    pub(crate) fn show(&self, generation: u64) {
        self.shown.store(generation, Ordering::Release);
    }

    /// A state when `settings` turn reasoning on; `None` when it is off, so
    /// that nothing is built.
    pub(crate) fn when_on(settings: &Settings, spawn: Spawn) -> Option<Arc<Shared>> {
        (settings.reasoning != Reasoning::Off).then(|| Arc::new(Shared::new(spawn)))
    }

    /// The inferred types of `iri` (an IRI, as written in full), with their
    /// proofs when they are found within `wait`. Inline (no threads), the
    /// search runs here, within its time limit. They are [`Types::Pending`] when
    /// the search's thread cannot start, or a search for another term is
    /// going on.
    pub(crate) fn inferred_types(self: &Arc<Self>, iri: &str, wait: Duration) -> Types {
        let read = self.read(|r| (r.generation, r.types.get(iri).cloned().unwrap_or_default()));
        let (generation, classes) = match read {
            Err(t) => return t,
            Ok(c) => c,
        };
        if classes.is_empty() {
            return Types::Proved(Arc::default());
        }
        let mut search = self.search.lock().unwrap_or_else(PoisonError::into_inner);
        let answer = match search.as_ref() {
            // That search's answer, taken already.
            Some(Search {
                iri: i,
                generation: g,
                kept: Some(kept),
                ..
            }) if i == iri && *g == generation => return Types::Proved(Arc::clone(kept)),
            // That search's answer, or wait for it.
            Some(s)
                if s.iri == iri
                    && s.generation == generation
                    && (!s.answer.is_empty() || !s.done.load(Ordering::Acquire)) =>
            {
                s.answer.clone()
            }
            // Not beside it.
            Some(s) if !s.done.load(Ordering::Acquire) => return Types::Pending(classes),
            _ => match self.search(iri, generation) {
                Some(started) => {
                    let answer = started.answer.clone();
                    *search = Some(started);
                    answer
                }
                // No thread to spare: not the search here, which may take long.
                None => return Types::Pending(classes),
            },
        };
        drop(search);
        // Inline the search has run already, and the browser has no clock
        // for `recv_timeout` to read.
        let answer = match answer.try_recv() {
            Ok(answer) => Ok(answer),
            Err(_) => answer.recv_timeout(wait),
        };
        match answer {
            Ok(Ok(Types::Proved(v))) => {
                let mut search = self.search.lock().unwrap_or_else(PoisonError::into_inner);
                if let Some(s) = search
                    .as_mut()
                    .filter(|s| s.iri == iri && s.generation == generation)
                {
                    s.kept = Some(Arc::clone(&v));
                }
                Types::Proved(v)
            }
            Ok(Ok(types)) => types,
            // A bug in the search: no proofs rather than no hover.
            Ok(Err(_)) => Types::Unknown,
            Err(_) => Types::Pending(classes),
        }
    }

    /// Start a search for the proofs of `iri`'s types in the result of
    /// `generation`; `None` when its thread cannot start.
    fn search(self: &Arc<Self>, iri: &str, generation: u64) -> Option<Search> {
        let (tx, answer) = crossbeam_channel::bounded(1);
        let done = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(self);
        let subject = iri.to_string();
        let ended = Arc::clone(&done);
        #[cfg(test)]
        self.searches.fetch_add(1, Ordering::Relaxed);
        self.spawn
            .run(
                "reasoning-explain",
                // Defence in depth, as the reasoning thread's.
                16 << 20,
                move || shared.prove(&subject),
                // Done before the answer is sent: a hover that reads the
                // answer and returns must not leave the next hover (of
                // another term) seeing this search as still going on.
                move |proved| {
                    ended.store(true, Ordering::Release);
                    let _ = tx.send(proved);
                },
            )
            .ok()?;
        Some(Search {
            iri: iri.to_string(),
            generation,
            answer,
            done,
            kept: None,
        })
    }

    /// The proofs of `iri`'s inferred types in the state as it is now.
    fn prove(&self, iri: &str) -> Types {
        let rdf_type = NamedNode::new_unchecked(RDF_TYPE);
        let subject = NamedNode::new_unchecked(iri);
        let read = |r: &Reasoned| {
            let Some(classes) = r.types.get(iri) else {
                return Types::Proved(Arc::default());
            };
            let mut cache = r.cache.lock().unwrap_or_else(PoisonError::into_inner);
            let Ok(explainer) = r.reasoner.explainer_with(std::mem::take(&mut *cache)) else {
                return Types::Unknown;
            };
            // One limit for all the term's types, shared out among them: a
            // type whose proof takes too long is listed without it.
            let budget = Budget::new(self.limits.hover);
            let proved = classes
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    let t = Triple::new(
                        subject.clone(),
                        rdf_type.clone(),
                        NamedNode::new_unchecked(c),
                    );
                    let proof = match budget
                        .next(classes.len() - i)
                        .map(|cancel| explainer.explain_cancellable(t.as_ref(), &cancel))
                    {
                        Some(Ok(Some(p))) => Ok(Proof::new(p, &r.base)),
                        Some(Ok(None)) => Err(NoProof::NotFound),
                        None | Some(Err(turbowl::Error::Cancelled)) => Err(NoProof::Stopped),
                        Some(Err(_)) => Err(NoProof::NotFound),
                    };
                    Inferred {
                        class: c.clone(),
                        proof,
                    }
                })
                .collect();
            *cache = explainer.into_cache();
            Types::Proved(Arc::new(proved))
        };
        self.read(read).unwrap_or_else(|t| t)
    }

    /// `f` of the result shown, without waiting: Err with what to say when
    /// there is none to read.
    fn read<T>(&self, f: impl FnOnce(&Reasoned) -> T) -> Result<T, Types> {
        let held = match self.held.try_read() {
            Ok(held) => held,
            // A run that panicked leaves no result (see `write`).
            Err(TryLockError::Poisoned(e)) => e.into_inner(),
            Err(TryLockError::WouldBlock) => return Err(Types::Running),
        };
        match held.as_ref() {
            None => Err(Types::Unknown),
            Some(r) if r.generation != self.shown.load(Ordering::Acquire) => Err(Types::Running),
            Some(r) => Ok(f(r)),
        }
    }

    #[cfg(test)]
    pub(super) fn read_for_test(&self) -> std::sync::RwLockReadGuard<'_, Option<Reasoned>> {
        self.held.read().unwrap()
    }

    /// The state, for the reasoning thread to write. A run takes the result
    /// out before it changes it, so a run that panicked has left none, and
    /// the lock is usable again.
    pub(super) fn write(&self) -> RwLockWriteGuard<'_, Option<Reasoned>> {
        let held = self.held.write().unwrap_or_else(PoisonError::into_inner);
        self.held.clear_poison();
        held
    }
}
