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
//! either, until the next run's result is shown ([`Types::Running`]). A proof is searched on a
//! thread of its own under the read lock, and the hover waits for it at
//! most a given time; past it the search goes on, its findings go into the
//! cache, and the next hover has them at once.

use super::base::{Base, Origin};
use super::turtle::Items;
use crate::server::worker::Spawn;
use crate::settings::{Reasoning, Settings};
use crate::syntax::RDF_TYPE;
use rustc_hash::FxHashMap;
use spargebra::term::{NamedNode, Triple};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError, RwLock, RwLockWriteGuard, TryLockError};
use std::time::Duration;

/// How long a hover waits for proofs.
pub(crate) const HOVER_WAIT: Duration = Duration::from_millis(200);

/// The reasoning state of a server with reasoning on.
pub(crate) struct Shared {
    /// How a proof search runs: on a thread of its own, or inline (the
    /// browser, which has no threads, and waits for it).
    spawn: Spawn,
    held: RwLock<Option<Reasoned>>,
    /// The generation of the run whose findings the server shows; only
    /// that run's result is read.
    shown: AtomicU64,
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

/// What reasoning says of a term's types.
pub(crate) enum Types {
    /// No result to read: no run has finished, or the last one was skipped
    /// or failed.
    Unknown,
    /// A run is writing the state, or the result there is of a run
    /// superseded while it ran: the next run's is coming.
    Running,
    /// Each inferred type, with its proof.
    Proved(Vec<Inferred>),
    /// The inferred types, whose proofs were not found in time. The search
    /// goes on, so asking again soon gives them.
    Pending(Vec<String>),
}

/// An inferred `rdf:type` of a term.
pub(crate) struct Inferred {
    /// The class IRI.
    pub(crate) class: String,
    /// `None` when the explainer found none (it finds one for every
    /// inferred triple, so this is a bug in it).
    pub(crate) proof: Option<Proof>,
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
                turbowl::Reason::Base => base.origin(&triple(&s.statement)?),
                turbowl::Reason::Rule { .. } => None,
            })
            .collect();
        let written = Items::new(&proof, &origins, base);
        Proof { proof, written }
    }
}

/// A proof's statement as an RDF triple, when it is one.
fn triple(s: &turbowl::Statement) -> Option<Triple> {
    use spargebra::term::{NamedOrBlankNode, Term};
    let subject: NamedOrBlankNode = match &s.subject {
        Term::NamedNode(n) => n.clone().into(),
        Term::BlankNode(b) => b.clone().into(),
        _ => return None,
    };
    let Term::NamedNode(p) = &s.predicate else {
        return None;
    };
    Some(Triple::new(subject, p.clone(), s.object.clone()))
}

impl Shared {
    pub(crate) fn new(spawn: Spawn) -> Shared {
        Shared {
            spawn,
            held: RwLock::new(None),
            shown: AtomicU64::new(0),
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
    /// search runs here, without a limit; when its thread cannot start, the
    /// types are [`Types::Pending`].
    pub(crate) fn inferred_types(self: &Arc<Self>, iri: &str, wait: Duration) -> Types {
        let classes = match self.read(|r| r.types.get(iri).cloned().unwrap_or_default()) {
            Err(t) => return t,
            Ok(c) => c,
        };
        if classes.is_empty() {
            return Types::Proved(Vec::new());
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let shared = Arc::clone(self);
        let subject = iri.to_string();
        let started = self.spawn.run(
            "reasoning-explain",
            // Defence in depth, as the reasoning thread's.
            16 << 20,
            move || shared.prove(&subject),
            move |proved| {
                let _ = tx.send(proved);
            },
        );
        // No thread to spare: not the search here, which may take long.
        if started.is_err() {
            return Types::Pending(classes);
        }
        // Inline the search has run already, and the browser has no clock
        // for `recv_timeout` to read.
        let answer = match rx.try_recv() {
            Ok(answer) => Ok(answer),
            Err(_) => rx.recv_timeout(wait),
        };
        match answer {
            Ok(Ok(types)) => types,
            // A bug in the search: no proofs rather than no hover.
            Ok(Err(_)) => Types::Unknown,
            Err(_) => Types::Pending(classes),
        }
    }

    /// The proofs of `iri`'s inferred types in the state as it is now.
    fn prove(&self, iri: &str) -> Types {
        let rdf_type = NamedNode::new_unchecked(RDF_TYPE);
        let subject = NamedNode::new_unchecked(iri);
        let read = |r: &Reasoned| {
            let Some(classes) = r.types.get(iri) else {
                return Types::Proved(Vec::new());
            };
            let mut cache = r.cache.lock().unwrap_or_else(PoisonError::into_inner);
            let Ok(explainer) = r.reasoner.explainer_with(std::mem::take(&mut *cache)) else {
                return Types::Unknown;
            };
            let proved = classes
                .iter()
                .map(|c| {
                    let t = Triple::new(
                        subject.clone(),
                        rdf_type.clone(),
                        NamedNode::new_unchecked(c),
                    );
                    Inferred {
                        class: c.clone(),
                        proof: explainer
                            .explain(t.as_ref())
                            .map(|p| Proof::new(p, &r.base)),
                    }
                })
                .collect();
            *cache = explainer.into_cache();
            Types::Proved(proved)
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
