//! Hovers held until their proofs are found.
//!
//! A hover on a resource waits a short time for the proofs of its inferred
//! types (`reasoning::HOVER_WAIT`); past it, the proof search goes on in the
//! background. Rather than answer then that the proofs are still being
//! worked out, which an editor shows until the user hovers again, the
//! server holds the request: a thread waits for the search to end, and the
//! main loop answers the hover again, now with the proofs, or with what the
//! search found by its time limit. A hover held behind another term's search
//! is held again for its own, until its deadline; past it, it is answered
//! as it is.
//!
//! `$/cancelRequest` answers a held hover with `RequestCancelled`, as an
//! editor does when the cursor moves away. Only on threads: inline (the
//! browser build) the search runs within the hover, which never waits.

use crate::clock::Instant;
use crate::server::worker::spawn_guarded;
use crossbeam_channel::{Receiver, Sender, unbounded};
use lsp_server::RequestId;
use lsp_types::Position;
use rustc_hash::FxHashMap;
use std::time::Duration;

/// The stack of a thread that only waits on a channel.
const WAITER_STACK: usize = 64 << 10;

/// A held hover: what it asked, and until when it may be held.
pub(super) struct Held {
    pub(super) uri: String,
    pub(super) pos: Position,
    pub(super) deadline: Instant,
}

/// The hovers held, by request id, and the channel on which their waiting
/// threads wake the main loop.
pub(super) struct Hovers {
    held: FxHashMap<RequestId, Held>,
    tx: Sender<RequestId>,
    rx: Receiver<RequestId>,
}

impl Default for Hovers {
    fn default() -> Self {
        let (tx, rx) = unbounded();
        Hovers {
            held: FxHashMap::default(),
            tx,
            rx,
        }
    }
}

impl Hovers {
    /// The deadline of a hover asked now, whose proof searches may each run
    /// `limit`: one for a search going on beside it, one for its own, and
    /// a margin for the answers to come back.
    pub(super) fn deadline(limit: Duration) -> Instant {
        Instant::now() + limit * 2 + Duration::from_millis(500)
    }

    /// Hold `held` as request `id` until `ended` disconnects (its search
    /// ended) or its deadline passes, whichever comes first. False when it
    /// cannot be held: its deadline has passed, or no thread could start to
    /// wait.
    pub(super) fn hold(&mut self, id: RequestId, held: Held, ended: Receiver<()>) -> bool {
        let left = held.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        let (tx, woken) = (self.tx.clone(), id.clone());
        let waited = spawn_guarded(
            "hover-wait",
            WAITER_STACK,
            move || {
                // Disconnected when the search ends; nothing is ever sent.
                let _ = ended.recv_timeout(left);
            },
            move |_| {
                let _ = tx.send(woken);
            },
        );
        if waited.is_err() {
            return false;
        }
        self.held.insert(id, held);
        true
    }

    /// Where the waiting threads say a held hover can be answered.
    pub(super) fn woken(&self) -> &Receiver<RequestId> {
        &self.rx
    }

    /// The held hover `id`, no longer held; `None` when it was answered
    /// already (cancelled).
    pub(super) fn take(&mut self, id: &RequestId) -> Option<Held> {
        self.held.remove(id)
    }

    /// The ids of every hover held.
    pub(super) fn ids(&self) -> impl Iterator<Item = &RequestId> {
        self.held.keys()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(deadline: Instant) -> Held {
        Held {
            uri: "file:///a.ttl".into(),
            pos: Position::new(0, 0),
            deadline,
        }
    }

    #[test]
    fn a_held_hover_wakes_when_its_search_ends_and_is_taken_once() {
        let mut h = Hovers::default();
        let (end, ended) = crossbeam_channel::bounded::<()>(0);
        assert!(h.hold(
            7.into(),
            held(Hovers::deadline(Duration::from_secs(5))),
            ended
        ));
        assert!(
            h.woken().try_recv().is_err(),
            "woken before its search ended"
        );
        drop(end);
        let id = h.woken().recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(id, RequestId::from(7));
        assert!(h.take(&id).is_some());
        assert!(h.take(&id).is_none(), "taken twice");
    }

    #[test]
    fn a_held_hover_wakes_at_its_deadline_when_its_search_goes_on() {
        let mut h = Hovers::default();
        let (_end, ended) = crossbeam_channel::bounded::<()>(0);
        let deadline = Instant::now() + Duration::from_millis(50);
        assert!(h.hold(1.into(), held(deadline), ended));
        let id = h.woken().recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(id, RequestId::from(1));
        assert!(Instant::now() >= deadline);
    }

    #[test]
    fn a_hover_past_its_deadline_is_not_held() {
        let mut h = Hovers::default();
        let (_end, ended) = crossbeam_channel::bounded::<()>(0);
        assert!(!h.hold(1.into(), held(Instant::now()), ended));
        assert_eq!(h.ids().count(), 0);
    }
}
